//! Application state and the update reducer.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::action::{self, Action};
use crate::model::{self, NodeId, NodeKind, SortDir, SortKey, Tree};
use crate::scan::ScanOutcome;

/// Which screen the app is currently showing.
pub enum Screen {
    Loading,
    Loaded(Loaded),
    Error(String),
}

/// Input mode: normal navigation, or typing into the filter box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Filter,
}

/// State for the loaded, interactive tree.
pub struct Loaded {
    pub tree: Tree,
    /// Flattened list of visible node ids (the table's rows). Cached; rebuilt
    /// only when expansion/sort/filter changes — never on plain cursor movement.
    pub visible: Vec<NodeId>,
    pub table_state: TableState,
    /// Rows of tree visible on screen, updated each render; drives paging.
    pub viewport_rows: usize,
    pub duration: Duration,
    pub inaccurate: bool,
    /// Whether the language-breakdown detail panel is shown.
    pub show_detail: bool,
    /// Active name filter (empty = no filter).
    pub filter: String,
}

/// Top-level application state.
pub struct App {
    pub root: PathBuf,
    pub root_label: String,
    pub sort_key: SortKey,
    pub sort_dir: SortDir,
    pub screen: Screen,
    pub mode: Mode,
    pub show_help: bool,
    pub spinner: usize,
    pub elapsed: Duration,
    pub should_quit: bool,
    /// A file the user asked to open in their editor, drained by the event loop
    /// (which owns the terminal) after each key press.
    pub pending_open: Option<PathBuf>,
}

impl App {
    pub fn new(root: PathBuf, root_label: String) -> Self {
        Self {
            root,
            root_label,
            sort_key: SortKey::Lines,
            sort_dir: SortDir::Desc,
            screen: Screen::Loading,
            mode: Mode::Normal,
            show_help: false,
            spinner: 0,
            elapsed: Duration::ZERO,
            should_quit: false,
            pending_open: None,
        }
    }

    /// Transition from `Loading` to `Loaded` once the scan completes.
    pub fn on_scan(&mut self, outcome: ScanOutcome) {
        let mut tree = outcome.tree;
        model::view::sort(&mut tree, self.sort_key, self.sort_dir);
        let visible = model::view::flatten_visible(&tree);
        let mut table_state = TableState::default();
        if !visible.is_empty() {
            table_state.select(Some(0));
        }
        self.screen = Screen::Loaded(Loaded {
            tree,
            visible,
            table_state,
            viewport_rows: 1,
            duration: outcome.duration,
            inaccurate: outcome.inaccurate,
            show_detail: false,
            filter: String::new(),
        });
    }

    pub fn on_scan_failed(&mut self, message: impl Into<String>) {
        self.screen = Screen::Error(message.into());
    }

    /// Route a key press: help overlay and filter editing intercept input;
    /// otherwise it maps to a navigation [`Action`].
    pub fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.show_help {
            // Any of these dismiss the overlay; everything else is swallowed.
            if matches!(key.code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
                self.show_help = false;
            }
            return;
        }

        if self.mode == Mode::Filter {
            self.handle_filter_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('d') if !ctrl => self.toggle_detail(),
            KeyCode::Tab => self.toggle_detail(),
            KeyCode::Esc => self.clear_filter(),
            _ => self.update(action::map_key(key)),
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.set_filter(String::new());
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                if let Screen::Loaded(loaded) = &self.screen {
                    let mut filter = loaded.filter.clone();
                    filter.pop();
                    self.set_filter(filter);
                }
            }
            KeyCode::Char(c) => {
                if let Screen::Loaded(loaded) = &self.screen {
                    let mut filter = loaded.filter.clone();
                    filter.push(c);
                    self.set_filter(filter);
                }
            }
            _ => {}
        }
    }

    fn toggle_detail(&mut self) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.show_detail = !loaded.show_detail;
        }
    }

    /// Clear an active filter (Esc in normal mode); a no-op otherwise.
    fn clear_filter(&mut self) {
        if matches!(&self.screen, Screen::Loaded(l) if !l.filter.is_empty()) {
            self.set_filter(String::new());
        }
    }

    fn set_filter(&mut self, filter: String) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.filter = filter;
            loaded.rebuild();
        }
    }

    /// Apply an action to the current state.
    pub fn update(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Quit => self.should_quit = true,
            Action::CycleSort => {
                self.sort_key = self.sort_key.next();
                self.apply_sort();
            }
            Action::ReverseSort => {
                self.sort_dir = self.sort_dir.flip();
                self.apply_sort();
            }
            Action::Open => self.open_selected(),
            other => {
                if let Screen::Loaded(loaded) = &mut self.screen {
                    loaded.handle(other);
                }
            }
        }
    }

    fn apply_sort(&mut self) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            model::view::sort(&mut loaded.tree, self.sort_key, self.sort_dir);
            loaded.rebuild();
        }
    }

    /// Enter on the selection: expand/descend a directory, or queue the selected
    /// file to open in the user's editor. The event loop owns the terminal and
    /// performs the actual suspend-and-launch.
    fn open_selected(&mut self) {
        let Screen::Loaded(loaded) = &mut self.screen else {
            return;
        };
        let Some(id) = loaded.selected_id() else {
            return;
        };
        if loaded.tree.nodes[id].is_dir() {
            loaded.expand_or_descend();
            return;
        }
        let rel_path = loaded.tree.nodes[id].rel_path.clone();
        self.pending_open = Some(self.root.join(rel_path));
    }
}

impl Loaded {
    /// The id of the currently selected node, if any.
    pub fn selected_id(&self) -> Option<NodeId> {
        self.table_state
            .selected()
            .and_then(|i| self.visible.get(i).copied())
    }

    /// Recompute the visible list, keeping the selection on the same node when
    /// possible and otherwise clamping into range.
    fn rebuild(&mut self) {
        let previous_id = self.selected_id();
        let previous_index = self.table_state.selected();
        self.visible = if self.filter.is_empty() {
            model::view::flatten_visible(&self.tree)
        } else {
            model::view::flatten_filtered(&self.tree, &self.filter)
        };
        if self.visible.is_empty() {
            self.table_state.select(None);
            return;
        }
        let index = previous_id
            .and_then(|id| self.visible.iter().position(|&n| n == id))
            .or(previous_index)
            .unwrap_or(0)
            .min(self.visible.len() - 1);
        self.table_state.select(Some(index));
    }

    fn handle(&mut self, action: Action) {
        match action {
            Action::Down => self.move_by(1),
            Action::Up => self.move_by(-1),
            Action::First => self.move_to(0),
            Action::Last => self.move_to(self.visible.len().saturating_sub(1)),
            Action::PageDown => self.move_by(self.viewport_rows.max(1) as i64),
            Action::PageUp => self.move_by(-(self.viewport_rows.max(1) as i64)),
            Action::Expand => self.expand_or_descend(),
            Action::Collapse => self.collapse_or_parent(),
            Action::Toggle => self.toggle(),
            Action::ExpandAll => self.set_all_expanded(true),
            Action::CollapseAll => self.collapse_all(),
            _ => {}
        }
    }

    fn move_by(&mut self, delta: i64) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as i64 - 1;
        let current = self.table_state.selected().unwrap_or(0) as i64;
        let next = (current + delta).clamp(0, last);
        self.table_state.select(Some(next as usize));
    }

    fn move_to(&mut self, index: usize) {
        if self.visible.is_empty() {
            return;
        }
        self.table_state
            .select(Some(index.min(self.visible.len() - 1)));
    }

    fn expand_or_descend(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if !self.tree.nodes[id].is_dir() {
            return;
        }
        if !self.tree.nodes[id].expanded {
            self.tree.nodes[id].expanded = true;
            self.rebuild();
        } else if let Some(&first) = self.tree.nodes[id].children.first()
            && let Some(pos) = self.visible.iter().position(|&n| n == first)
        {
            self.table_state.select(Some(pos));
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if self.tree.nodes[id].is_dir() && self.tree.nodes[id].expanded {
            self.tree.nodes[id].expanded = false;
            self.rebuild();
        } else if let Some(parent) = self.tree.nodes[id].parent
            && parent != self.tree.root
            && let Some(pos) = self.visible.iter().position(|&n| n == parent)
        {
            self.table_state.select(Some(pos));
        }
    }

    fn toggle(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if self.tree.nodes[id].is_dir() {
            self.tree.nodes[id].expanded = !self.tree.nodes[id].expanded;
            self.rebuild();
        }
    }

    fn set_all_expanded(&mut self, expanded: bool) {
        for node in &mut self.tree.nodes {
            if node.kind == NodeKind::Dir {
                node.expanded = expanded;
            }
        }
        self.tree.nodes[self.tree.root].expanded = true;
        self.rebuild();
    }

    fn collapse_all(&mut self) {
        let root = self.tree.root;
        for (id, node) in self.tree.nodes.iter_mut().enumerate() {
            if node.kind == NodeKind::Dir {
                node.expanded = id == root;
            }
        }
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::model::build_tree;
    use crate::scan::ScanOutcome;
    use std::path::{Path, PathBuf};
    use tokei::{Language, LanguageType, Languages, Report};

    /// A loaded app for `/proj` with `src/main.rs` and a top-level `README.md`.
    fn sample_app() -> App {
        let mut rust = Language::new();
        rust.reports = vec![Report::new(PathBuf::from("/proj/src/main.rs"))];
        let mut md = Language::new();
        md.reports = vec![Report::new(PathBuf::from("/proj/README.md"))];
        let mut languages = Languages::new();
        languages.insert(LanguageType::Rust, rust);
        languages.insert(LanguageType::Markdown, md);

        let tree = build_tree(&languages, Path::new("/proj"), "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            inaccurate: false,
        });
        app
    }

    fn select(app: &mut App, name: &str) {
        let Screen::Loaded(loaded) = &mut app.screen else {
            panic!("expected a loaded screen");
        };
        let index = loaded
            .visible
            .iter()
            .position(|&id| loaded.tree.nodes[id].name == name)
            .unwrap_or_else(|| panic!("{name} is not visible"));
        loaded.table_state.select(Some(index));
    }

    fn is_visible(app: &App, name: &str) -> bool {
        let Screen::Loaded(loaded) = &app.screen else {
            return false;
        };
        loaded
            .visible
            .iter()
            .any(|&id| loaded.tree.nodes[id].name == name)
    }

    #[test]
    fn enter_on_a_file_queues_it_for_the_editor() {
        let mut app = sample_app();
        select(&mut app, "README.md");
        app.update(Action::Open);
        assert_eq!(app.pending_open, Some(PathBuf::from("/proj/README.md")));
    }

    #[test]
    fn enter_on_a_directory_expands_without_queuing() {
        let mut app = sample_app();
        // `src` starts collapsed, so its child is not yet visible.
        assert!(!is_visible(&app, "main.rs"));
        select(&mut app, "src");
        app.update(Action::Open);
        assert!(app.pending_open.is_none());
        assert!(is_visible(&app, "main.rs"));
    }
}
