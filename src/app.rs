//! Application state and the update reducer.

use std::path::PathBuf;
use std::time::Duration;

use ratatui::widgets::TableState;

use crate::action::Action;
use crate::model::{self, NodeId, NodeKind, SortDir, SortKey, Tree};
use crate::scan::ScanOutcome;

/// Which screen the app is currently showing.
pub enum Screen {
    Loading,
    Loaded(Loaded),
    Error(String),
}

/// State for the loaded, interactive tree.
pub struct Loaded {
    pub tree: Tree,
    /// Flattened list of visible node ids (the table's rows). Cached; rebuilt
    /// only when expansion/sort changes — never on plain cursor movement.
    pub visible: Vec<NodeId>,
    pub table_state: TableState,
    /// Rows of tree visible on screen, updated each render; drives paging.
    pub viewport_rows: usize,
    pub duration: Duration,
    pub inaccurate: bool,
}

/// Top-level application state.
pub struct App {
    pub root: PathBuf,
    pub root_label: String,
    pub sort_key: SortKey,
    pub sort_dir: SortDir,
    pub screen: Screen,
    pub spinner: usize,
    pub elapsed: Duration,
    pub should_quit: bool,
}

impl App {
    pub fn new(root: PathBuf, root_label: String) -> Self {
        Self {
            root,
            root_label,
            sort_key: SortKey::Lines,
            sort_dir: SortDir::Desc,
            screen: Screen::Loading,
            spinner: 0,
            elapsed: Duration::ZERO,
            should_quit: false,
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
        });
    }

    pub fn on_scan_failed(&mut self, message: impl Into<String>) {
        self.screen = Screen::Error(message.into());
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
        self.visible = model::view::flatten_visible(&self.tree);
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
