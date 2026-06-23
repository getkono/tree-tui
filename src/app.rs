//! Application state and the update reducer.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::action::{self, Action};
use crate::collect::LayerResult;
use crate::model::{
    self, ChurnData, CodeData, Layer, Lens, NodeId, NodeKind, SortDir, StatusData, SubKey, Tree,
};
use crate::scan::ScanOutcome;

/// Which screen the app is currently showing.
pub enum Screen {
    Loading,
    // Boxed: `Loaded` is much larger than the other variants.
    Loaded(Box<Loaded>),
    Error(String),
}

/// Input mode: normal navigation, or typing into the filter box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Filter,
}

/// How a queued file open should hand off to an external program: `View` runs
/// the pager (`$PAGER`), `Edit` runs the editor (`$VISUAL`/`$EDITOR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    View,
    Edit,
}

/// State for the loaded, interactive tree.
pub struct Loaded {
    pub tree: Tree,
    /// Flattened list of visible node ids (the table's rows). Cached; rebuilt
    /// only when expansion/sort/filter/lens changes — never on plain movement.
    pub visible: Vec<NodeId>,
    pub table_state: TableState,
    /// Rows of tree visible on screen, updated each render; drives paging.
    pub viewport_rows: usize,
    pub duration: Duration,
    /// Whether the detail panel is shown.
    pub show_detail: bool,
    /// Active name filter (empty = no filter).
    pub filter: String,
    /// Hide rows whose value is zero under the active lens.
    pub hide_zeros: bool,
    /// The active lens (which metric drives the view).
    pub active_lens: Lens,
    /// The sort sub-key (scoped to the active lens).
    pub sort_key: SubKey,
    pub sort_dir: SortDir,
    /// Whether the (computed) code layer reported a parsing ambiguity.
    pub inaccurate: bool,
    // Lazily-computed, cached per-lens metric layers.
    pub code: Layer<CodeData>,
    pub churn: Layer<ChurnData>,
    pub status: Layer<StatusData>,
}

/// Top-level application state.
pub struct App {
    pub root: PathBuf,
    pub root_label: String,
    pub screen: Screen,
    pub mode: Mode,
    pub show_help: bool,
    pub spinner: usize,
    pub elapsed: Duration,
    pub should_quit: bool,
    /// A file the user asked to open externally (view in `$PAGER` or edit in
    /// `$EDITOR`), drained by the event loop (which owns the terminal) after each
    /// key press.
    pub pending_open: Option<(PathBuf, OpenMode)>,
    /// A lens whose data must be computed; drained by the event loop, which
    /// spawns the background collector.
    pub pending_compute: Option<Lens>,
    /// Whether the root is inside a git repository (gates the git lenses).
    pub repo: bool,
}

impl App {
    pub fn new(root: PathBuf, root_label: String) -> Self {
        Self {
            root,
            root_label,
            screen: Screen::Loading,
            mode: Mode::Normal,
            show_help: false,
            spinner: 0,
            elapsed: Duration::ZERO,
            should_quit: false,
            pending_open: None,
            pending_compute: None,
            repo: false,
        }
    }

    /// Whether the app has work in flight (initial walk or a computing lens),
    /// which keeps the spinner ticking.
    pub fn is_busy(&self) -> bool {
        match &self.screen {
            Screen::Loading => true,
            Screen::Loaded(loaded) => {
                loaded.code.is_computing()
                    || loaded.churn.is_computing()
                    || loaded.status.is_computing()
            }
            Screen::Error(_) => false,
        }
    }

    /// Transition from `Loading` to `Loaded` once the walk completes, and request
    /// the default lens's data.
    pub fn on_scan(&mut self, outcome: ScanOutcome) {
        self.repo = outcome.repo;
        let mut loaded = Loaded::new(outcome.tree, outcome.duration);
        loaded.apply_sort();
        // The default lens (code) reads a layer; kick off its computation.
        loaded.mark_computing(loaded.active_lens);
        self.pending_compute = Some(loaded.active_lens);
        self.screen = Screen::Loaded(Box::new(loaded));
    }

    pub fn on_scan_failed(&mut self, message: impl Into<String>) {
        self.screen = Screen::Error(message.into());
    }

    /// Apply a freshly-computed lens layer (from a background collector).
    pub fn on_layer(&mut self, result: LayerResult) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.apply_layer(result);
        }
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
            Action::Open => self.open_selected(),
            Action::Edit => self.edit_selected(),
            Action::CycleLens => self.cycle_lens(),
            Action::JumpLens(n) => self.jump_lens(n),
            other => {
                if let Screen::Loaded(loaded) = &mut self.screen {
                    loaded.handle(other);
                }
            }
        }
    }

    fn cycle_lens(&mut self) {
        if let Some(lens) = self.next_available_lens() {
            self.set_lens(lens);
        }
    }

    fn jump_lens(&mut self, n: u8) {
        let idx = (n as usize).wrapping_sub(1);
        if let Some(&lens) = Lens::ALL.get(idx)
            && lens.is_available(self.repo)
        {
            self.set_lens(lens);
        }
    }

    /// The next lens after the active one that has data to show, or `None` if no
    /// other lens is available.
    fn next_available_lens(&self) -> Option<Lens> {
        let Screen::Loaded(loaded) = &self.screen else {
            return None;
        };
        let mut lens = loaded.active_lens;
        for _ in 0..Lens::ALL.len() {
            lens = lens.next();
            if lens != loaded.active_lens && lens.is_available(self.repo) {
                return Some(lens);
            }
        }
        None
    }

    /// Switch the active lens, requesting its layer if not yet computed.
    fn set_lens(&mut self, lens: Lens) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.set_lens(lens);
            if lens.has_layer() && loaded.layer_not_computed(lens) {
                loaded.mark_computing(lens);
                self.pending_compute = Some(lens);
            }
        }
    }

    /// Enter on the selection: expand/descend a directory, or queue the selected
    /// file to view in the user's pager.
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
        self.pending_open = Some((self.root.join(rel_path), OpenMode::View));
    }

    /// Shift+Enter / `e` on the selection: queue the selected file to edit in the
    /// user's editor. A no-op on directories.
    fn edit_selected(&mut self) {
        if let Some(path) = self.selected_file_path() {
            self.pending_open = Some((path, OpenMode::Edit));
        }
    }

    /// The absolute path of the selected node when it is a file, else `None`
    /// (no side effects).
    fn selected_file_path(&self) -> Option<PathBuf> {
        let Screen::Loaded(loaded) = &self.screen else {
            return None;
        };
        let id = loaded.selected_id()?;
        if loaded.tree.nodes[id].is_dir() {
            return None;
        }
        let rel_path = loaded.tree.nodes[id].rel_path.clone();
        Some(self.root.join(rel_path))
    }
}

impl Loaded {
    fn new(tree: Tree, duration: Duration) -> Self {
        Self {
            tree,
            visible: Vec::new(),
            table_state: TableState::default(),
            viewport_rows: 1,
            duration,
            show_detail: false,
            filter: String::new(),
            hide_zeros: false,
            active_lens: Lens::Code,
            sort_key: Lens::Code.default_sub_key(),
            sort_dir: SortDir::Desc,
            inaccurate: false,
            code: Layer::NotComputed,
            churn: Layer::NotComputed,
            status: Layer::NotComputed,
        }
    }

    /// The id of the currently selected node, if any.
    pub fn selected_id(&self) -> Option<NodeId> {
        self.table_state
            .selected()
            .and_then(|i| self.visible.get(i).copied())
    }

    /// The value of `key` for node `id`, reading the node fields or the relevant
    /// cached layer (zero until that layer is `Ready`).
    pub fn value(&self, key: SubKey, id: NodeId) -> u128 {
        match key {
            SubKey::Bytes => self.tree.nodes[id].bytes as u128,
            SubKey::Files => self.tree.nodes[id].files as u128,
            SubKey::Name => 0,
            SubKey::Lines => self.code_at(id).map_or(0, |c| c.num.lines()) as u128,
            SubKey::Code => self.code_at(id).map_or(0, |c| c.num.code) as u128,
            SubKey::Comments => self.code_at(id).map_or(0, |c| c.num.comments) as u128,
            SubKey::Blanks => self.code_at(id).map_or(0, |c| c.num.blanks) as u128,
            SubKey::Added => self.churn_at(id).map_or(0, |c| c.added) as u128,
            SubKey::Deleted => self.churn_at(id).map_or(0, |c| c.deleted) as u128,
            SubKey::Churn => self.churn_at(id).map_or(0, |c| c.churn()) as u128,
            SubKey::Commits => self.churn_at(id).map_or(0, |c| c.commits) as u128,
            SubKey::StatusAdded => self.status_at(id).map_or(0, |s| s.added) as u128,
            SubKey::StatusModified => self.status_at(id).map_or(0, |s| s.modified) as u128,
            SubKey::StatusDeleted => self.status_at(id).map_or(0, |s| s.deleted) as u128,
            SubKey::StatusTotal => self.status_at(id).map_or(0, |s| s.total()) as u128,
        }
    }

    /// The active lens's headline value for node `id` (the bar / zero-test value).
    pub fn primary_value(&self, id: NodeId) -> u128 {
        self.value(self.active_lens.primary().key, id)
    }

    /// The concatenated display name for a row (a chain of sole sub-directories,
    /// e.g. `src/main/java`); just the node's own name otherwise.
    pub fn display_name(&self, id: NodeId) -> String {
        model::view::row_name(&self.tree, id)
    }

    /// A row's indentation level, counted in displayed ancestors so a chained
    /// child sits one level under its concatenated row.
    pub fn display_depth(&self, id: NodeId) -> usize {
        model::view::display_depth(&self.tree, id)
    }

    /// The code layer entry for a node, if the code layer is computed.
    pub fn code_at(&self, id: NodeId) -> Option<&CodeData> {
        self.code.ready().map(|values| &values[id])
    }

    /// The churn layer entry for a node, if the churn layer is computed.
    pub fn churn_at(&self, id: NodeId) -> Option<&ChurnData> {
        self.churn.ready().map(|values| &values[id])
    }

    /// The status layer entry for a node, if the status layer is computed.
    pub fn status_at(&self, id: NodeId) -> Option<&StatusData> {
        self.status.ready().map(|values| &values[id])
    }

    /// Whether the active lens's layer is still being computed.
    pub fn active_computing(&self) -> bool {
        match self.active_lens {
            Lens::Code => self.code.is_computing(),
            Lens::Churn => self.churn.is_computing(),
            Lens::Status => self.status.is_computing(),
            Lens::Size => false,
        }
    }

    fn layer_not_computed(&self, lens: Lens) -> bool {
        match lens {
            Lens::Code => matches!(self.code, Layer::NotComputed),
            Lens::Churn => matches!(self.churn, Layer::NotComputed),
            Lens::Status => matches!(self.status, Layer::NotComputed),
            Lens::Size => false,
        }
    }

    fn mark_computing(&mut self, lens: Lens) {
        match lens {
            Lens::Code => self.code = Layer::Computing,
            Lens::Churn => self.churn = Layer::Computing,
            Lens::Status => self.status = Layer::Computing,
            Lens::Size => {}
        }
    }

    fn apply_layer(&mut self, result: LayerResult) {
        let lens = result.lens();
        match result {
            LayerResult::Code { files, inaccurate } => {
                self.code = Layer::Ready(model::aggregate_code(&self.tree, &files));
                self.inaccurate = inaccurate;
            }
            LayerResult::Churn(files) => {
                self.churn = Layer::Ready(model::aggregate(&self.tree, &files));
            }
            LayerResult::Status(files) => {
                self.status = Layer::Ready(model::aggregate(&self.tree, &files));
            }
        }
        // Re-sort if the data we just got drives the current view.
        if lens == self.active_lens {
            self.apply_sort();
        }
    }

    fn set_lens(&mut self, lens: Lens) {
        self.active_lens = lens;
        self.sort_key = lens.default_sub_key();
        self.apply_sort();
    }

    /// Re-order siblings by the current sort key/direction, then rebuild the
    /// visible list.
    fn apply_sort(&mut self) {
        if self.sort_key == SubKey::Name {
            model::view::sort_by_name(&mut self.tree, self.sort_dir);
        } else {
            let values: Vec<u128> = (0..self.tree.nodes.len())
                .map(|id| self.value(self.sort_key, id))
                .collect();
            model::view::sort_by_values(&mut self.tree, &values, self.sort_dir);
        }
        self.rebuild();
    }

    /// Recompute the visible list (honoring filter + declutter), keeping the
    /// selection on the same node when possible.
    fn rebuild(&mut self) {
        let previous_id = self.selected_id();
        let previous_index = self.table_state.selected();
        let mut visible = if self.filter.is_empty() {
            model::view::flatten_visible(&self.tree)
        } else {
            model::view::flatten_filtered(&self.tree, &self.filter)
        };
        if self.hide_zeros {
            visible.retain(|&id| self.primary_value(id) > 0);
        }
        self.visible = visible;
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
            Action::CycleSort => {
                self.sort_key = self.active_lens.next_sub_key(self.sort_key);
                self.apply_sort();
            }
            Action::ReverseSort => {
                self.sort_dir = self.sort_dir.flip();
                self.apply_sort();
            }
            Action::ToggleZeros => {
                self.hide_zeros = !self.hide_zeros;
                self.rebuild();
            }
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
        } else {
            // Descend into the chain's tail children, not the head's.
            let tail = model::view::segment_tail(&self.tree, id);
            if let Some(&first) = self.tree.nodes[tail].children.first()
                && let Some(pos) = self.visible.iter().position(|&n| n == first)
            {
                self.table_state.select(Some(pos));
            }
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(id) = self.selected_id() else {
            return;
        };
        if self.tree.nodes[id].is_dir() && self.tree.nodes[id].expanded {
            self.tree.nodes[id].expanded = false;
            self.rebuild();
        } else if let Some(parent) = self.tree.nodes[id].parent {
            // Jump to the row that owns the parent — the head of its chain, which
            // may be several path segments up from the raw parent node.
            let target = model::view::segment_head(&self.tree, parent);
            if target != self.tree.root
                && let Some(pos) = self.visible.iter().position(|&n| n == target)
            {
                self.table_state.select(Some(pos));
            }
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
    use crate::collect::LayerResult;
    use crate::model::{CodeData, CodeNum, build_skeleton};
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// A loaded app for `/proj` with `src/main.rs` and a top-level `README.md`.
    fn sample_app() -> App {
        let files = vec![
            (PathBuf::from("src/main.rs"), 1000),
            (PathBuf::from("README.md"), 200),
        ];
        let dirs = vec![PathBuf::from("src")];
        let tree = build_skeleton(&files, &dirs, "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });
        // Simulate the event loop draining the initial code request.
        app.pending_compute = None;
        app
    }

    /// A loaded app whose tree contains the sole-subdirectory chain
    /// `a/b/c/deep.rs` plus a top-level `top.rs`.
    fn chained_app() -> App {
        let files = vec![
            (PathBuf::from("a/b/c/deep.rs"), 1000),
            (PathBuf::from("top.rs"), 10),
        ];
        let dirs = vec![
            PathBuf::from("a"),
            PathBuf::from("a/b"),
            PathBuf::from("a/b/c"),
        ];
        let tree = build_skeleton(&files, &dirs, "proj".into());
        let mut app = App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });
        app.pending_compute = None;
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

    fn active_lens(app: &App) -> Lens {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        loaded.active_lens
    }

    #[test]
    fn on_scan_requests_the_default_code_lens() {
        let tree = build_skeleton(&[(PathBuf::from("a.rs"), 1)], &[], "p".into());
        let mut app = App::new(PathBuf::from("/p"), "p".into());
        app.on_scan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });
        assert_eq!(app.pending_compute, Some(Lens::Code));
        assert!(app.is_busy());
    }

    #[test]
    fn enter_on_a_file_queues_it_for_viewing() {
        let mut app = sample_app();
        select(&mut app, "README.md");
        app.update(Action::Open);
        assert_eq!(
            app.pending_open,
            Some((PathBuf::from("/proj/README.md"), OpenMode::View))
        );
    }

    #[test]
    fn edit_on_a_file_queues_it_for_editing() {
        let mut app = sample_app();
        select(&mut app, "README.md");
        app.update(Action::Edit);
        assert_eq!(
            app.pending_open,
            Some((PathBuf::from("/proj/README.md"), OpenMode::Edit))
        );
    }

    #[test]
    fn edit_on_a_directory_is_a_noop() {
        let mut app = sample_app();
        select(&mut app, "src");
        app.update(Action::Edit);
        assert!(app.pending_open.is_none());
        // Unlike Enter, editing a directory does not expand it.
        assert!(!is_visible(&app, "main.rs"));
    }

    #[test]
    fn enter_on_a_directory_expands_without_queuing() {
        let mut app = sample_app();
        assert!(!is_visible(&app, "main.rs"));
        select(&mut app, "src");
        app.update(Action::Open);
        assert!(app.pending_open.is_none());
        assert!(is_visible(&app, "main.rs"));
    }

    #[test]
    fn cycle_lens_advances_to_size_and_does_not_recompute() {
        let mut app = sample_app(); // repo = false
        app.update(Action::CycleLens);
        assert_eq!(active_lens(&app), Lens::Size);
        // Size needs no layer, so nothing is requested.
        assert!(app.pending_compute.is_none());
    }

    #[test]
    fn jump_to_unavailable_git_lens_is_a_noop() {
        let mut app = sample_app(); // repo = false → churn (3) unavailable
        app.update(Action::JumpLens(3));
        assert_eq!(active_lens(&app), Lens::Code);
    }

    #[test]
    fn git_lens_is_available_and_requested_in_a_repo() {
        let mut app = sample_app();
        app.repo = true;
        app.pending_compute = None;
        app.update(Action::JumpLens(3)); // churn
        assert_eq!(active_lens(&app), Lens::Churn);
        assert_eq!(app.pending_compute, Some(Lens::Churn));
    }

    #[test]
    fn reactivating_a_computed_lens_does_not_request_again() {
        let mut app = sample_app();
        // Code is currently Computing (from on_scan). Switch to Size and back.
        app.update(Action::CycleLens); // -> Size
        app.update(Action::JumpLens(1)); // -> Code (still Computing, not NotComputed)
        assert_eq!(active_lens(&app), Lens::Code);
        assert!(app.pending_compute.is_none());
    }

    #[test]
    fn on_layer_caches_code_and_sorts_by_lines() {
        let mut app = sample_app();
        let mut files = HashMap::new();
        let main = CodeData {
            num: CodeNum {
                code: 100,
                comments: 0,
                blanks: 0,
            },
            ..Default::default()
        };
        files.insert(PathBuf::from("src/main.rs"), main);
        app.on_layer(LayerResult::Code {
            files,
            inaccurate: false,
        });
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        assert!(loaded.code_at(loaded.tree.root).is_some());
        // src (100 lines) sorts above README.md (0) under the code lens.
        let src_id = loaded.tree.index[&PathBuf::from("src")];
        assert_eq!(loaded.value(SubKey::Lines, src_id), 100);
    }

    #[test]
    fn chained_segment_expands_and_collapses_as_one_unit() {
        let mut app = chained_app();
        // `a/b/c` is a single row: b and c never appear on their own.
        assert!(is_visible(&app, "a"));
        assert!(!is_visible(&app, "b"));
        assert!(!is_visible(&app, "c"));
        assert!(!is_visible(&app, "deep.rs"));

        // The row reads as the concatenated path.
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        let a = loaded.tree.index[&PathBuf::from("a")];
        assert_eq!(loaded.display_name(a), "a/b/c");

        // Expanding the chain reveals the tail's child directly — no b/c rows.
        select(&mut app, "a");
        app.update(Action::Expand);
        assert!(is_visible(&app, "deep.rs"));
        assert!(!is_visible(&app, "b"));
        assert!(!is_visible(&app, "c"));

        // Collapsing the same row hides it again.
        select(&mut app, "a");
        app.update(Action::Collapse);
        assert!(!is_visible(&app, "deep.rs"));
    }

    #[test]
    fn collapse_from_chained_child_jumps_to_the_segment_row() {
        let mut app = chained_app();
        select(&mut app, "a");
        app.update(Action::Expand);
        select(&mut app, "deep.rs");
        // Left on a file jumps to the parent *row* — the `a/b/c` chain head.
        app.update(Action::Collapse);
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        let selected = loaded.selected_id().unwrap();
        assert_eq!(loaded.tree.nodes[selected].name, "a");
    }

    #[test]
    fn toggle_zeros_hides_zero_value_rows() {
        let mut app = sample_app(); // code not ready → all lines zero
        // Under the code lens with no data, decluttering hides everything.
        app.update(Action::ToggleZeros);
        assert!(!is_visible(&app, "src"));
        app.update(Action::ToggleZeros);
        assert!(is_visible(&app, "src"));
    }
}
