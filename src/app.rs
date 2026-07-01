//! Application state and the update reducer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Rect};
use ratatui::widgets::{Row, TableState};
use ratatui_image::picker::Picker;

use crate::action::{self, Action};
use crate::collect::LayerResult;
use crate::model::{
    self, ChurnData, CodeData, Layer, Lens, NodeId, NodeKind, SortDir, StatusData, SubKey, Tree,
};
use crate::scan::ScanOutcome;
use crate::ui::codeview::CodeView;
use crate::ui::preview::Preview;
use crate::ui::reader::{Handoff, Reader, ReaderExit};

/// Rows the tree selection moves per wheel notch.
const WHEEL_TREE_ROWS: i64 = 1;
/// Lines a scrollable view (preview / reader) moves per wheel notch.
const WHEEL_PREVIEW_LINES: i32 = 3;

/// Which pane receives Normal-mode navigation keys and is drawn focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Tree,
    Preview,
}

/// The pane rectangles from the last render, for mouse hit-testing. Only the
/// focusable panes are tracked; a point over the detail pane or chrome hits
/// neither and leaves focus unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneRects {
    pub tree: Rect,
    pub preview: Option<Rect>,
}

impl PaneRects {
    /// Which focusable pane (if any) contains the point.
    pub fn hit(&self, col: u16, row: u16) -> Option<Focus> {
        if self.preview.is_some_and(|r| contains(r, col, row)) {
            Some(Focus::Preview)
        } else if contains(self.tree, col, row) {
            Some(Focus::Tree)
        } else {
            None
        }
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

/// Cached, fully-built tree-table render inputs (rows + resolved columns) for a
/// given content revision, width, and computing state. Reused on pure
/// navigation frames: selection and scroll changes don't alter row *content*
/// (ratatui applies the row highlight from `table_state` at render time), so the
/// rows are rebuilt only when content changes (`rebuild_rev`), the pane is
/// resized (`width`), or the active lens's computing state flips (the `…`
/// placeholder). Built and populated by `ui::tree_view::render`.
pub struct RowCache {
    pub(crate) rows: Vec<Row<'static>>,
    pub(crate) header: Row<'static>,
    pub(crate) widths: Vec<Constraint>,
    pub(crate) width: u16,
    pub(crate) rev: u64,
    pub(crate) computing: bool,
}

/// Which screen the app is currently showing.
pub enum Screen {
    Loading,
    // Boxed: `Loaded` is much larger than the other variants.
    Loaded(Box<Loaded>),
    /// The full-screen file reader; owns the suspended tree, restored on exit.
    Reader(Box<Reader>),
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
    /// only when expansion/sort/filter/lens changes — never on plain movement.
    pub visible: Vec<NodeId>,
    pub table_state: TableState,
    /// Rows of tree visible on screen, updated each render; drives paging.
    pub viewport_rows: usize,
    pub duration: Duration,
    /// Whether the detail panel is shown.
    pub show_detail: bool,
    /// Whether the preview pane is enabled (it still folds away when the
    /// terminal is too narrow or short — see the renderer's thresholds).
    pub show_preview: bool,
    /// The node whose preview is currently cached, to skip reloading on every
    /// frame; refreshed when the selection changes.
    pub preview_for: Option<NodeId>,
    /// Cached preview content for the selected node.
    pub preview: Preview,
    /// Scroll state for the text preview; rebuilt when the selection changes.
    pub preview_view: CodeView,
    /// Which pane has keyboard focus (and the highlighted border).
    pub focus: Focus,
    /// Pane rectangles from the last render, for mouse hit-testing.
    pub panes: PaneRects,
    /// Active name filter (empty = no filter).
    pub filter: String,
    /// Hide rows whose value is zero under the active lens.
    pub hide_zeros: bool,
    /// Nodes the user has explicitly **excluded** from the aggregate statistics,
    /// keyed by `rel_path` so the choice survives rescans (ids are not stable
    /// across walks). Exclusion is inherited by the whole subtree unless a
    /// descendant is re-included via `included`. Excluded subtrees stay visible
    /// but are subtracted from every ancestor total; see
    /// [`Loaded::effective_value`].
    pub excluded: HashSet<PathBuf>,
    /// Nodes the user has explicitly **re-included** as exceptions carved out of
    /// an otherwise-excluded subtree, also keyed by `rel_path`. Tracking
    /// exclusion and inclusion as independent, directory-level intents — rather
    /// than lowering "exclude this directory except that file" into "exclude
    /// each of the directory's current files" — is lossless: files that appear
    /// later under an excluded directory stay excluded, and only the named
    /// exceptions keep counting.
    pub included: HashSet<PathBuf>,
    /// Resolved node ids where the effective exclusion state *flips* going down
    /// the tree: `exclude_boundaries` are excluded nodes whose inherited state
    /// is included, `include_boundaries` the reverse. Derived from
    /// `excluded`/`included` by [`Loaded::refresh_exclusion`] and cached so the
    /// read-time arithmetic in [`Loaded::effective_value`] never re-resolves
    /// paths per query. Redundant markers (matching their inherited state) are
    /// omitted, so these hold only the genuine transitions.
    exclude_boundaries: HashSet<NodeId>,
    include_boundaries: HashSet<NodeId>,
    /// The active lens (which metric drives the view).
    pub active_lens: Lens,
    /// The sort sub-key (scoped to the active lens).
    pub sort_key: SubKey,
    pub sort_dir: SortDir,
    /// Whether the (computed) code layer reported a parsing ambiguity.
    pub inaccurate: bool,
    /// Bumped on every content rebuild; part of the row-cache key so navigation
    /// frames reuse the built rows but any content change rebuilds them.
    pub rebuild_rev: u64,
    /// Cached tree-table render inputs reused across navigation frames; see
    /// [`RowCache`].
    pub row_cache: Option<RowCache>,
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
    /// A file the user asked to edit in `$EDITOR` (Shift+Enter / `e`), drained by
    /// the event loop (which owns the terminal) after each key press. Viewing now
    /// opens the in-TUI reader instead of shelling out to `$PAGER`.
    pub pending_edit: Option<PathBuf>,
    /// A lens whose data must be computed; drained by the event loop, which
    /// spawns the background collector.
    pub pending_compute: Option<Lens>,
    /// A requested mouse-capture change (the release-capture toggle); drained by
    /// the event loop, which owns the terminal. `Some(false)` releases capture
    /// for native selection, `Some(true)` re-grabs it.
    pub pending_capture: Option<bool>,
    /// Whether the root is inside a git repository (gates the git lenses).
    pub repo: bool,
    /// Terminal image-protocol picker for the preview pane, probed once at
    /// startup. `None` in tests and when the terminal can't be queried.
    pub picker: Option<Picker>,
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
            pending_edit: None,
            pending_compute: None,
            pending_capture: None,
            repo: false,
            picker: None,
        }
    }

    /// Whether the app has work in flight (initial walk or a computing lens),
    /// which keeps the spinner ticking.
    pub fn is_busy(&self) -> bool {
        match &self.screen {
            Screen::Loading => true,
            Screen::Loaded(loaded) => loaded.any_computing(),
            Screen::Reader(reader) => reader.loaded.any_computing(),
            Screen::Error(_) => false,
        }
    }

    /// The interactive tree state, whether shown directly or behind the reader.
    fn loaded_mut(&mut self) -> Option<&mut Loaded> {
        match &mut self.screen {
            Screen::Loaded(loaded) => Some(loaded),
            Screen::Reader(reader) => Some(&mut reader.loaded),
            _ => None,
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

    /// Re-apply a fresh walk after a filesystem change.
    ///
    /// A no-op when the visible skeleton (paths + sizes) is unchanged, so
    /// spurious events — builds, git internals, and other gitignored/hidden
    /// paths absent from the walk — cost only a cheap re-walk. Otherwise the
    /// tree is rebuilt (preserving expansion and selection by path) and the
    /// cached metric layers are invalidated, with the active lens re-requested.
    pub fn on_rescan(&mut self, outcome: ScanOutcome) {
        let Some(loaded) = self.loaded_mut() else {
            // Still loading: treat this as the initial scan.
            self.on_scan(outcome);
            return;
        };
        if loaded.same_skeleton(&outcome.tree) {
            return;
        }
        loaded.reload(outcome.tree, outcome.duration);
        let active = loaded.active_lens;
        let needs_layer = active.has_layer();
        if needs_layer {
            loaded.mark_computing(active);
        }
        self.repo = outcome.repo;
        if needs_layer {
            self.pending_compute = Some(active);
        }
    }

    /// Apply a freshly-computed lens layer (from a background collector).
    pub fn on_layer(&mut self, result: LayerResult) {
        if let Some(loaded) = self.loaded_mut() {
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

        // The full-screen reader owns all input while it is open.
        if let Screen::Reader(reader) = &mut self.screen {
            if matches!(reader.handle_key(key), ReaderExit::ToTree) {
                self.close_reader();
            }
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

    fn toggle_preview(&mut self) {
        let root = self.root.clone();
        let picker = self.picker.as_ref();
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.show_preview = !loaded.show_preview;
            // Paint the first preview immediately on enable; later selection
            // changes are debounced off the navigation hot path (event loop).
            if loaded.show_preview {
                loaded.ensure_preview(&root, picker);
            }
        }
    }

    /// The selected node whose preview is on screen but not yet cached — the
    /// target the event loop debounces a refresh toward. `None` when the preview
    /// is hidden, folded away, or already current; used to keep the syntax
    /// highlight off the per-frame navigation path.
    pub fn preview_target_id(&self) -> Option<NodeId> {
        let Screen::Loaded(loaded) = &self.screen else {
            return None;
        };
        if loaded.show_preview && loaded.panes.preview.is_some() {
            let sel = loaded.selected_id();
            if sel != loaded.preview_for {
                return sel;
            }
        }
        None
    }

    /// Load the preview for the current selection. Driven by the event loop's
    /// debounce timer so the file read + highlight never runs on a nav frame.
    pub fn refresh_preview(&mut self) {
        let root = self.root.clone();
        let picker = self.picker.as_ref();
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.ensure_preview(&root, picker);
        }
    }

    /// Move keyboard focus between the tree and the preview pane. Only focuses
    /// the preview when it is actually on screen.
    fn cycle_focus(&mut self) {
        if let Screen::Loaded(loaded) = &mut self.screen {
            loaded.focus = match loaded.focus {
                Focus::Tree if loaded.panes.preview.is_some() => Focus::Preview,
                _ => Focus::Tree,
            };
        }
    }

    /// Copy the focused pane's content to the system clipboard via OSC 52: the
    /// visible preview text, or the selected node's path when the tree is focused.
    fn yank(&mut self) {
        let Screen::Loaded(loaded) = &self.screen else {
            return;
        };
        let text = match loaded.focus {
            Focus::Preview => loaded.preview_view.visible_text(),
            Focus::Tree => loaded
                .selected_id()
                .map(|id| loaded.tree.nodes[id].rel_path.display().to_string())
                .unwrap_or_default(),
        };
        if !text.is_empty() {
            crate::clipboard::osc52_copy(&text);
        }
    }

    /// Queue a mouse-capture flip for the event loop: releasing capture lets the
    /// terminal's native click-drag selection work; toggling again re-grabs it.
    fn toggle_mouse_capture(&mut self) {
        self.pending_capture = Some(!crate::tui::mouse_captured());
    }

    /// A wheel scroll at a screen position. The wheel acts on the pane under the
    /// cursor and focuses it (interact-to-focus). `delta` is signed steps
    /// (positive = down); the event loop passes ±1 per wheel event, moving 1
    /// tree row / 3 preview lines per notch. Every target clamps, so a scroll
    /// can't run off the edge.
    pub fn handle_scroll(&mut self, col: u16, row: u16, delta: i32) {
        match &mut self.screen {
            Screen::Loaded(loaded) => match loaded.panes.hit(col, row) {
                Some(Focus::Preview) => {
                    loaded.focus = Focus::Preview;
                    loaded.preview_view.scroll_by(delta * WHEEL_PREVIEW_LINES);
                }
                Some(Focus::Tree) => {
                    loaded.focus = Focus::Tree;
                    loaded.move_by(delta as i64 * WHEEL_TREE_ROWS);
                }
                None => {}
            },
            // The full-screen reader scrolls its view with the wheel too.
            Screen::Reader(reader) => reader.scroll(delta * WHEEL_PREVIEW_LINES),
            _ => {}
        }
    }

    /// A left click focuses the pane under the cursor (interact-to-focus). An
    /// idle hover never changes focus — only deliberate interaction does. In the
    /// tree it also moves the selection to the clicked row; clicking the
    /// already-selected row activates it (like Enter: expand/descend a dir, open
    /// a file). A click on the border, header, or blank rows only focuses.
    pub fn handle_click(&mut self, col: u16, row: u16) {
        let mut activate = false;
        if let Screen::Loaded(loaded) = &mut self.screen
            && let Some(focus) = loaded.panes.hit(col, row)
        {
            loaded.focus = focus;
            if focus == Focus::Tree
                && let Some(index) = loaded.row_at(row)
            {
                if loaded.table_state.selected() == Some(index) {
                    activate = true;
                } else {
                    loaded.move_to(index);
                }
            }
        }
        // The `loaded` borrow above has ended; activation re-borrows `self`.
        if activate {
            self.open_selected();
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
            Action::TogglePreview => self.toggle_preview(),
            Action::CycleLens => self.cycle_lens(),
            Action::JumpLens(n) => self.jump_lens(n),
            Action::CycleFocus => self.cycle_focus(),
            Action::Yank => self.yank(),
            Action::ToggleMouseCapture => self.toggle_mouse_capture(),
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

    /// Enter on the selection: expand/descend a directory, or open the selected
    /// file in the full-screen reader.
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
        self.open_reader(self.root.join(rel_path));
    }

    /// Replace the loaded tree with a full-screen reader over `path`, moving the
    /// boxed tree state into the reader so exiting restores it whole.
    fn open_reader(&mut self, path: PathBuf) {
        let Screen::Loaded(loaded) = std::mem::replace(&mut self.screen, Screen::Loading) else {
            return; // unreachable: only called from the loaded screen
        };
        let reader = Reader::open(loaded, path, self.picker.as_ref());
        self.screen = Screen::Reader(Box::new(reader));
    }

    /// Close the reader, restoring the tree it was holding.
    fn close_reader(&mut self) {
        if let Screen::Reader(reader) = std::mem::replace(&mut self.screen, Screen::Loading) {
            self.screen = Screen::Loaded(reader.loaded);
        }
    }

    /// Take a pending external handoff (`$EDITOR`/`$PAGER`) requested from inside
    /// the reader, for the event loop to run while the TUI is suspended.
    pub fn take_reader_handoff(&mut self) -> Option<Handoff> {
        match &mut self.screen {
            Screen::Reader(reader) => reader.pending_handoff.take(),
            _ => None,
        }
    }

    /// Shift+Enter / `e` on the selection: queue the selected file to edit in the
    /// user's editor. A no-op on directories.
    fn edit_selected(&mut self) {
        if let Some(path) = self.selected_file_path() {
            self.pending_edit = Some(path);
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
            show_preview: true,
            preview_for: None,
            preview: Preview::Empty,
            preview_view: CodeView::default(),
            focus: Focus::Tree,
            panes: PaneRects::default(),
            filter: String::new(),
            hide_zeros: false,
            excluded: HashSet::new(),
            included: HashSet::new(),
            exclude_boundaries: HashSet::new(),
            include_boundaries: HashSet::new(),
            active_lens: Lens::Code,
            sort_key: Lens::Code.default_sub_key(),
            sort_dir: SortDir::Desc,
            inaccurate: false,
            rebuild_rev: 0,
            row_cache: None,
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

    /// Load the preview for the current selection if it isn't already cached.
    /// Called by the renderer only while the preview pane is visible, so the
    /// bounded file read happens lazily and at most once per selection.
    pub fn ensure_preview(&mut self, root: &Path, picker: Option<&Picker>) {
        let id = self.selected_id();
        if id == self.preview_for {
            return;
        }
        self.preview_for = id;
        self.preview = match id {
            None => Preview::Empty,
            Some(id) if self.tree.nodes[id].is_dir() => {
                Preview::Info(format!("{} — directory", self.display_name(id)))
            }
            Some(id) => {
                let path = root.join(&self.tree.nodes[id].rel_path);
                crate::ui::preview::load(&path, picker)
            }
        };
        // Reset the preview's scroll state for the new content.
        self.preview_view = match &self.preview {
            Preview::Text(lines) => CodeView::new(lines.clone()),
            _ => CodeView::default(),
        };
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
        self.effective_value(self.active_lens.primary().key, id)
    }

    /// The value of `key` for node `id` with excluded subtrees subtracted and
    /// re-included exceptions added back.
    ///
    /// Aggregation is a pure bottom-up sum, so an excluded subtree's
    /// contribution to `id` equals that subtree root's stored value. Because
    /// exclusion and inclusion boundaries strictly alternate down any path,
    /// subtracting each `exclude_boundaries` subtree and adding back each
    /// `include_boundaries` subtree telescopes to exactly the included leaves —
    /// no re-aggregation and no retained per-file maps. A boundary nested inside
    /// another makes the running subtotal dip below zero mid-sum (an excluded
    /// directory's value is subtracted before its re-included child is added),
    /// so we accumulate signed and clamp only at the end. An *excluded* `id`'s
    /// own row still shows its full total (it's only dimmed).
    pub fn effective_value(&self, key: SubKey, id: NodeId) -> u128 {
        if self.is_excluded(id) {
            return self.value(key, id);
        }
        let mut total = self.value(key, id) as i128;
        for &boundary in &self.exclude_boundaries {
            if self.is_ancestor(id, boundary) {
                total -= self.value(key, boundary) as i128;
            }
        }
        for &boundary in &self.include_boundaries {
            if self.is_ancestor(id, boundary) {
                total += self.value(key, boundary) as i128;
            }
        }
        total.max(0) as u128
    }

    /// Whether `id` is effectively excluded: the nearest exclusion/inclusion
    /// boundary on the path from `id` up to the root is an exclusion (with no
    /// boundary at all, the default is included). Drives the dimmed/struck-
    /// through styling of the subtree.
    pub fn is_excluded(&self, id: NodeId) -> bool {
        if self.exclude_boundaries.is_empty() && self.include_boundaries.is_empty() {
            return false;
        }
        let mut cur = Some(id);
        while let Some(node) = cur {
            if self.exclude_boundaries.contains(&node) {
                return true;
            }
            if self.include_boundaries.contains(&node) {
                return false;
            }
            cur = self.tree.nodes[node].parent;
        }
        false
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    fn is_ancestor(&self, ancestor: NodeId, descendant: NodeId) -> bool {
        let mut cur = self.tree.nodes[descendant].parent;
        while let Some(node) = cur {
            if node == ancestor {
                return true;
            }
            cur = self.tree.nodes[node].parent;
        }
        false
    }

    /// The effective exclusion state at `start` (walk up, nearest resolved
    /// marker wins; default included), evaluated against explicit resolved
    /// marker id-sets. Used by [`Loaded::refresh_exclusion`] to classify each
    /// marker as a real boundary or a redundant no-op.
    fn resolved_excluded(
        &self,
        start: Option<NodeId>,
        excluded: &HashSet<NodeId>,
        included: &HashSet<NodeId>,
    ) -> bool {
        let mut cur = start;
        while let Some(node) = cur {
            if excluded.contains(&node) {
                return true;
            }
            if included.contains(&node) {
                return false;
            }
            cur = self.tree.nodes[node].parent;
        }
        false
    }

    /// Recompute the boundary caches from the `excluded`/`included` rel-paths
    /// against the current arena: resolve each path to its id (dropping any not
    /// in this walk), then keep only the markers that actually flip the
    /// inherited state — an excluded node under an included parent, or an
    /// included node under an excluded parent. Redundant markers are ignored
    /// (they stay in the path sets so they survive rescans, but contribute
    /// nothing). Call after mutating `excluded`/`included` or swapping the tree.
    fn refresh_exclusion(&mut self) {
        let excluded_ids: HashSet<NodeId> = self
            .excluded
            .iter()
            .filter_map(|path| self.tree.index.get(path).copied())
            .collect();
        let included_ids: HashSet<NodeId> = self
            .included
            .iter()
            .filter_map(|path| self.tree.index.get(path).copied())
            .collect();
        self.exclude_boundaries = excluded_ids
            .iter()
            .copied()
            .filter(|&id| {
                !self.resolved_excluded(self.tree.nodes[id].parent, &excluded_ids, &included_ids)
            })
            .collect();
        self.include_boundaries = included_ids
            .iter()
            .copied()
            .filter(|&id| {
                self.resolved_excluded(self.tree.nodes[id].parent, &excluded_ids, &included_ids)
            })
            .collect();
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

    /// Whether any lens layer is still being computed (keeps the spinner alive).
    pub fn any_computing(&self) -> bool {
        self.code.is_computing() || self.churn.is_computing() || self.status.is_computing()
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

    /// Whether `other` has the same paths and per-node sizes as the current
    /// tree — i.e. nothing the walk can see has changed.
    fn same_skeleton(&self, other: &Tree) -> bool {
        if self.tree.nodes.len() != other.nodes.len() || self.tree.index.len() != other.index.len()
        {
            return false;
        }
        self.tree.index.iter().all(|(path, &id)| {
            other
                .index
                .get(path)
                .is_some_and(|&oid| other.nodes[oid].bytes == self.tree.nodes[id].bytes)
        })
    }

    /// Swap in a freshly-walked `tree`, preserving expansion and selection by
    /// path (node ids are not stable across walks) and invalidating the cached
    /// metric layers.
    fn reload(&mut self, tree: Tree, duration: Duration) {
        let expanded: HashSet<PathBuf> = self
            .tree
            .nodes
            .iter()
            .filter(|n| n.is_dir() && n.expanded)
            .map(|n| n.rel_path.clone())
            .collect();
        let selected_path = self
            .selected_id()
            .map(|id| self.tree.nodes[id].rel_path.clone());

        self.tree = tree;
        self.duration = duration;
        self.inaccurate = false;
        self.code = Layer::NotComputed;
        self.churn = Layer::NotComputed;
        self.status = Layer::NotComputed;

        // Re-apply expansion (the root is already expanded by build_skeleton).
        for node in &mut self.tree.nodes {
            if node.is_dir() && expanded.contains(&node.rel_path) {
                node.expanded = true;
            }
        }

        // Re-resolve exclusion roots against the new arena (paths persist).
        self.refresh_exclusion();

        self.apply_sort(); // rebuilds the visible list

        // Restore the selection on the same path when it still exists.
        if let Some(path) = selected_path
            && let Some(&id) = self.tree.index.get(&path)
            && let Some(pos) = self.visible.iter().position(|&n| n == id)
        {
            self.table_state.select(Some(pos));
        }
    }

    /// Re-order siblings by the current sort key/direction, then rebuild the
    /// visible list.
    fn apply_sort(&mut self) {
        if self.sort_key == SubKey::Name {
            model::view::sort_by_name(&mut self.tree, self.sort_dir);
        } else {
            let values: Vec<u128> = (0..self.tree.nodes.len())
                .map(|id| self.effective_value(self.sort_key, id))
                .collect();
            model::view::sort_by_values(&mut self.tree, &values, self.sort_dir);
        }
        self.rebuild();
    }

    /// Recompute the visible list (honoring filter + declutter), keeping the
    /// selection on the same node when possible.
    fn rebuild(&mut self) {
        // Content is changing: invalidate the cached table rows (the renderer
        // keys its row cache on this counter).
        self.rebuild_rev = self.rebuild_rev.wrapping_add(1);
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
        // When the preview pane is focused, navigation scrolls it instead of
        // the tree; unhandled actions fall through to the tree below.
        if self.focus == Focus::Preview && self.handle_preview_action(action) {
            return;
        }
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
            Action::ToggleExclude => {
                if let Some(id) = self.selected_id() {
                    let path = self.tree.nodes[id].rel_path.clone();
                    // Flip the row's *effective* state. We only keep an explicit
                    // marker when it overrides what the node would otherwise
                    // inherit; when the target state already matches the parent,
                    // clearing both markers is enough (e.g. re-including a file
                    // whose directory is not excluded restores the plain
                    // default, and re-excluding it needs no marker either).
                    let want_excluded = !self.is_excluded(id);
                    let parent = self.tree.nodes[id].parent;
                    let inherited_excluded = parent.is_some_and(|p| self.is_excluded(p));
                    self.excluded.remove(&path);
                    self.included.remove(&path);
                    if want_excluded != inherited_excluded {
                        if want_excluded {
                            self.excluded.insert(path);
                        } else {
                            self.included.insert(path);
                        }
                    }
                    self.refresh_exclusion();
                    self.apply_sort();
                }
            }
            _ => {}
        }
    }

    /// Route a navigation action to the focused preview pane. Returns whether it
    /// was consumed (movement keys scroll the preview; anything else falls
    /// through to the tree).
    fn handle_preview_action(&mut self, action: Action) -> bool {
        match action {
            Action::Down => self.preview_view.scroll_by(1),
            Action::Up => self.preview_view.scroll_by(-1),
            Action::PageDown => self.preview_view.page(1),
            Action::PageUp => self.preview_view.page(-1),
            Action::First => self.preview_view.goto_top(),
            Action::Last => self.preview_view.goto_bottom(),
            Action::Collapse => self.preview_view.scroll_h(-4),
            Action::Expand => self.preview_view.scroll_h(4),
            _ => return false,
        }
        true
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

    /// Map a screen `row` to a visible-list index, or `None` if it lands on the
    /// border/header/blank rows. Data rows begin at `panes.tree.y + 2` (the top
    /// border + the header) and span `viewport_rows`; the scroll offset comes
    /// from ratatui's `table_state` (read-only — the table still owns scroll).
    fn row_at(&self, row: u16) -> Option<usize> {
        let first = self.panes.tree.y.saturating_add(2);
        if row < first {
            return None; // border or header
        }
        let in_view = (row - first) as usize;
        if in_view >= self.viewport_rows {
            return None; // below the last drawn data row
        }
        let index = self.table_state.offset() + in_view;
        (index < self.visible.len()).then_some(index)
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

    /// Give the loaded screen a tree pane on the left and a preview on the right.
    fn set_panes(app: &mut App) {
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.panes = PaneRects {
                tree: Rect::new(0, 0, 40, 20),
                preview: Some(Rect::new(80, 0, 40, 20)),
            };
        }
    }

    /// Set the visible-rows count the renderer normally records each frame, so
    /// `row_at` accepts clicks below the first data row in unit tests.
    fn set_viewport(app: &mut App, rows: usize) {
        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.viewport_rows = rows;
        }
    }

    fn focus(app: &App) -> Focus {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        loaded.focus
    }

    fn selected_index(app: &App) -> usize {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        loaded.table_state.selected().unwrap()
    }

    fn rebuild_rev(app: &App) -> u64 {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        loaded.rebuild_rev
    }

    /// The root node's effective value for `key` (totals after exclusions).
    fn root_effective(app: &App, key: SubKey) -> u128 {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        loaded.effective_value(key, loaded.tree.root)
    }

    /// Whether the node named `name` is currently marked excluded.
    fn is_excluded(app: &App, name: &str) -> bool {
        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        let id = loaded
            .tree
            .nodes
            .iter()
            .position(|n| n.name == name)
            .unwrap_or_else(|| panic!("no node named {name}"));
        loaded.is_excluded(id)
    }

    #[test]
    fn excluding_a_directory_subtracts_its_subtree_from_totals() {
        // /proj = src/main.rs (1000 bytes) + README.md (200 bytes) → 1200, 2 files.
        let mut app = sample_app();
        assert_eq!(root_effective(&app, SubKey::Bytes), 1200);
        assert_eq!(root_effective(&app, SubKey::Files), 2);

        // Excluding src drops its subtree (main.rs) from the root totals, and the
        // whole subtree reads as excluded for styling.
        select(&mut app, "src");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 200);
        assert_eq!(root_effective(&app, SubKey::Files), 1);
        assert!(is_excluded(&app, "src"));
        assert!(is_excluded(&app, "main.rs"), "subtree inherits exclusion");
        assert!(!is_excluded(&app, "README.md"));

        // Re-toggling restores the full totals.
        select(&mut app, "src");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 1200);
        assert_eq!(root_effective(&app, SubKey::Files), 2);
        assert!(!is_excluded(&app, "src"));
    }

    /// A loaded app with two files under `src` plus a top-level README, for
    /// exercising re-inclusion exceptions carved out of an excluded directory.
    /// `/proj` = src/main.rs (1000) + src/lib.rs (500) + README.md (200).
    fn exclusion_app() -> App {
        let files = vec![
            (PathBuf::from("src/main.rs"), 1000),
            (PathBuf::from("src/lib.rs"), 500),
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
        app.pending_compute = None;
        app
    }

    #[test]
    fn re_including_a_file_keeps_only_that_exception_out_of_the_exclusion() {
        let mut app = exclusion_app();
        assert_eq!(root_effective(&app, SubKey::Bytes), 1700);
        assert_eq!(root_effective(&app, SubKey::Files), 3);

        // Exclude the whole src directory: only README.md is left counted.
        select(&mut app, "src");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 200);
        assert_eq!(root_effective(&app, SubKey::Files), 1);

        // Re-include main.rs as an exception. src stays excluded (lib.rs with
        // it), but main.rs counts again: 200 + 1000 bytes across 2 files.
        app.update(Action::Expand); // reveal main.rs / lib.rs under src
        select(&mut app, "main.rs");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 1200);
        assert_eq!(root_effective(&app, SubKey::Files), 2);
        assert!(is_excluded(&app, "src"), "the directory stays excluded");
        assert!(is_excluded(&app, "lib.rs"), "siblings stay excluded");
        assert!(
            !is_excluded(&app, "main.rs"),
            "only the exception is included"
        );

        // Toggling main.rs again folds it back under src's exclusion.
        select(&mut app, "main.rs");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 200);
        assert_eq!(root_effective(&app, SubKey::Files), 1);
        assert!(is_excluded(&app, "main.rs"));
    }

    #[test]
    fn un_excluding_a_directory_folds_a_lingering_exception_back_in() {
        let mut app = exclusion_app();
        select(&mut app, "src");
        app.update(Action::ToggleExclude);
        app.update(Action::Expand);
        select(&mut app, "main.rs");
        app.update(Action::ToggleExclude); // re-include main.rs as an exception
        assert_eq!(root_effective(&app, SubKey::Bytes), 1200);

        // Re-include src itself. The whole subtree is counted again and the
        // now-redundant main.rs exception is inert (it matches its parent state).
        select(&mut app, "src");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 1700);
        assert_eq!(root_effective(&app, SubKey::Files), 3);
        assert!(!is_excluded(&app, "src"));
        assert!(!is_excluded(&app, "lib.rs"));
        assert!(!is_excluded(&app, "main.rs"));
    }

    #[test]
    fn exclusion_and_inclusion_alternate_down_the_tree() {
        // /proj
        //   a/               (excluded)
        //     keep.rs   100   (excluded via a)
        //     b/             (re-included exception)
        //       inc.rs  10    (included via b)
        //       c/           (excluded again)
        //         deep.rs 1000 (excluded via c)
        let files = vec![
            (PathBuf::from("a/keep.rs"), 100),
            (PathBuf::from("a/b/inc.rs"), 10),
            (PathBuf::from("a/b/c/deep.rs"), 1000),
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
        app.update(Action::ExpandAll);

        assert_eq!(root_effective(&app, SubKey::Bytes), 1110);

        // Exclude a → nothing under it counts.
        select(&mut app, "a");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 0);

        // Re-include a/b → inc.rs and (for now) deep.rs come back.
        select(&mut app, "b");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 1010);

        // Exclude a/b/c → deep.rs drops out again, leaving only inc.rs. The two
        // nested exclusions (a, then c) must not double-count the shared subtree.
        select(&mut app, "c");
        app.update(Action::ToggleExclude);
        assert_eq!(root_effective(&app, SubKey::Bytes), 10);
        assert!(is_excluded(&app, "a"));
        assert!(is_excluded(&app, "keep.rs"));
        assert!(!is_excluded(&app, "b"));
        assert!(!is_excluded(&app, "inc.rs"));
        assert!(is_excluded(&app, "c"));
        assert!(is_excluded(&app, "deep.rs"));
    }

    #[test]
    fn pane_hit_test_maps_point_to_focus() {
        let panes = PaneRects {
            tree: Rect::new(0, 0, 40, 20),
            preview: Some(Rect::new(80, 0, 40, 20)),
        };
        assert_eq!(panes.hit(5, 5), Some(Focus::Tree));
        assert_eq!(panes.hit(90, 5), Some(Focus::Preview));
        assert_eq!(panes.hit(60, 5), None); // the gap between the panes
        assert_eq!(panes.hit(5, 50), None); // below both panes
    }

    #[test]
    fn click_focuses_the_pane_under_the_cursor() {
        let mut app = sample_app();
        set_panes(&mut app);
        app.handle_click(90, 5);
        assert_eq!(focus(&app), Focus::Preview);
        app.handle_click(5, 5);
        assert_eq!(focus(&app), Focus::Tree);
        // A click over neither pane leaves focus unchanged.
        app.handle_click(90, 5);
        app.handle_click(60, 5);
        assert_eq!(focus(&app), Focus::Preview);
    }

    #[test]
    fn click_selects_the_clicked_tree_row() {
        let mut app = sample_app();
        set_panes(&mut app);
        set_viewport(&mut app, 17);
        select(&mut app, "src");
        app.update(Action::Expand); // ≥3 visible rows
        app.update(Action::First); // selection (and offset) at the top
        // Data rows start at screen row 2 (top border + header); the third data
        // row is screen row 4 → visible index 2.
        app.handle_click(5, 4);
        assert_eq!(selected_index(&app), 2);
        assert_eq!(focus(&app), Focus::Tree);
    }

    #[test]
    fn click_on_the_header_or_border_only_focuses() {
        let mut app = sample_app();
        set_panes(&mut app);
        set_viewport(&mut app, 17);
        app.update(Action::First);
        let before = selected_index(&app);
        app.handle_click(5, 0); // top border
        assert_eq!(focus(&app), Focus::Tree);
        assert_eq!(selected_index(&app), before);
        app.handle_click(5, 1); // header row
        assert_eq!(selected_index(&app), before);
    }

    #[test]
    fn clicking_the_selected_directory_row_activates_it() {
        let mut app = sample_app();
        set_panes(&mut app);
        set_viewport(&mut app, 17);
        select(&mut app, "src");
        assert!(!is_visible(&app, "main.rs"));
        // Clicking the already-selected row activates it (like Enter) → expands.
        let row = 2 + selected_index(&app) as u16;
        app.handle_click(5, row);
        assert!(is_visible(&app, "main.rs"));
    }

    #[test]
    fn clicking_the_selected_file_row_opens_the_reader() {
        let mut app = sample_app();
        set_panes(&mut app);
        set_viewport(&mut app, 17);
        select(&mut app, "README.md");
        // Clicking the already-selected file row activates it → opens the reader.
        let row = 2 + selected_index(&app) as u16;
        app.handle_click(5, row);
        assert!(matches!(app.screen, Screen::Reader(_)));
    }

    #[test]
    fn content_changes_bump_the_rebuild_rev_but_navigation_does_not() {
        let mut app = sample_app();
        select(&mut app, "src");
        let base = rebuild_rev(&app);

        // Plain navigation never rebuilds content, so the rev is stable (the
        // renderer reuses its cached rows on these frames).
        app.update(Action::Down);
        app.update(Action::Up);
        assert_eq!(rebuild_rev(&app), base, "navigation must not bump the rev");

        // Expansion, sorting, and filtering each change content → rebuild.
        select(&mut app, "src");
        app.update(Action::Expand);
        let after_expand = rebuild_rev(&app);
        assert!(after_expand > base, "expansion should bump the rev");
        app.update(Action::CycleSort);
        let after_sort = rebuild_rev(&app);
        assert!(after_sort > after_expand, "sorting should bump the rev");
        app.set_filter("rs".into());
        assert!(
            rebuild_rev(&app) > after_sort,
            "filtering should bump the rev"
        );
    }

    #[test]
    fn clicking_a_new_tree_row_selects_without_activating() {
        let mut app = sample_app();
        set_panes(&mut app);
        set_viewport(&mut app, 17);
        select(&mut app, "src");
        // Click README.md's row (a different, not-yet-selected row): it selects
        // but does not activate, so the reader never opens.
        let readme = {
            let Screen::Loaded(loaded) = &app.screen else {
                panic!("not loaded");
            };
            loaded
                .visible
                .iter()
                .position(|&id| loaded.tree.nodes[id].name == "README.md")
                .unwrap()
        };
        app.handle_click(5, 2 + readme as u16);
        assert!(matches!(app.screen, Screen::Loaded(_)));
        assert_eq!(selected_index(&app), readme);
    }

    #[test]
    fn wheel_moves_by_notch_count_and_focuses_the_pane() {
        let mut app = sample_app();
        set_panes(&mut app);
        select(&mut app, "src");
        app.update(Action::Expand); // reveal main.rs / app.rs so there is room
        app.update(Action::First); // start at the top
        let top = selected_index(&app);

        // Over the tree, the wheel moves by the coalesced notch count: a spin of
        // N notches moves N rows, and focuses the pane it acts on.
        app.handle_scroll(5, 5, 2);
        assert_eq!(selected_index(&app), top + 2);
        assert_eq!(focus(&app), Focus::Tree);

        // A big spin clamps to the last row rather than running off the end.
        app.update(Action::Last);
        let last = selected_index(&app);
        app.update(Action::First);
        app.handle_scroll(5, 5, 99);
        assert_eq!(selected_index(&app), last);

        // Scrolling up by a big spin clamps back to the top.
        app.handle_scroll(5, 5, -99);
        assert_eq!(selected_index(&app), top);

        // Over the preview, the wheel scrolls the preview view (not the tree
        // selection) and focuses the preview.
        let now = selected_index(&app);
        app.handle_scroll(90, 5, 1);
        assert_eq!(selected_index(&app), now);
        assert_eq!(focus(&app), Focus::Preview);
    }

    #[test]
    fn preview_refresh_is_gated_until_the_selection_settles() {
        let mut app = sample_app();
        set_panes(&mut app);
        select(&mut app, "src");

        // The selection differs from the cached preview, so a refresh is due —
        // this is the signal the event loop debounces on, keeping the file read
        // + syntax highlight off the per-frame navigation path.
        assert!(app.preview_target_id().is_some());

        // Once loaded the target clears, so nav frames request no more work.
        app.refresh_preview();
        assert_eq!(app.preview_target_id(), None);

        // With the preview hidden there is never a target to refresh.
        app.update(Action::TogglePreview);
        select(&mut app, "README.md");
        assert_eq!(app.preview_target_id(), None);
    }

    #[test]
    fn navigation_scrolls_the_preview_when_it_is_focused() {
        let mut app = sample_app();
        select(&mut app, "src");
        app.update(Action::Expand);
        select(&mut app, "src");
        let before = selected_index(&app);

        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.focus = Focus::Preview;
        }
        app.update(Action::Down);
        assert_eq!(
            selected_index(&app),
            before,
            "tree selection must not move while the preview is focused"
        );

        if let Screen::Loaded(loaded) = &mut app.screen {
            loaded.focus = Focus::Tree;
        }
        app.update(Action::Down);
        assert_eq!(selected_index(&app), before + 1);
    }

    #[test]
    fn cycle_focus_requires_a_visible_preview() {
        let mut app = sample_app();
        app.update(Action::CycleFocus);
        assert_eq!(focus(&app), Focus::Tree); // no preview rect recorded yet
        set_panes(&mut app);
        app.update(Action::CycleFocus);
        assert_eq!(focus(&app), Focus::Preview);
        app.update(Action::CycleFocus);
        assert_eq!(focus(&app), Focus::Tree);
    }

    #[test]
    fn toggle_mouse_capture_queues_a_flip() {
        let mut app = sample_app();
        app.update(Action::ToggleMouseCapture);
        // Capture starts off in tests (tui::init never ran), so this requests on.
        assert_eq!(app.pending_capture, Some(true));
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
    fn enter_on_a_file_opens_the_reader() {
        let mut app = sample_app();
        select(&mut app, "README.md");
        app.update(Action::Open);
        assert!(app.pending_edit.is_none(), "should not shell out to $PAGER");
        match &app.screen {
            Screen::Reader(reader) => {
                assert_eq!(reader.path, PathBuf::from("/proj/README.md"));
            }
            _ => panic!("expected the reader screen"),
        }
    }

    #[test]
    fn closing_the_reader_restores_the_tree() {
        let mut app = sample_app();
        // Expand `src` so the restored tree must remember the expansion.
        select(&mut app, "src");
        app.update(Action::Expand);
        assert!(is_visible(&app, "main.rs"));

        select(&mut app, "README.md");
        app.update(Action::Open);
        assert!(matches!(app.screen, Screen::Reader(_)));

        // `q` in the reader returns to the tree with state intact.
        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(app.screen, Screen::Loaded(_)));
        assert!(is_visible(&app, "main.rs"), "expansion should be preserved");
    }

    #[test]
    fn edit_on_a_file_queues_it_for_editing() {
        let mut app = sample_app();
        select(&mut app, "README.md");
        app.update(Action::Edit);
        assert_eq!(app.pending_edit, Some(PathBuf::from("/proj/README.md")));
    }

    #[test]
    fn edit_on_a_directory_is_a_noop() {
        let mut app = sample_app();
        select(&mut app, "src");
        app.update(Action::Edit);
        assert!(app.pending_edit.is_none());
        // Unlike Enter, editing a directory does not expand it.
        assert!(!is_visible(&app, "main.rs"));
    }

    #[test]
    fn enter_on_a_directory_expands_without_opening_the_reader() {
        let mut app = sample_app();
        assert!(!is_visible(&app, "main.rs"));
        select(&mut app, "src");
        app.update(Action::Open);
        assert!(matches!(app.screen, Screen::Loaded(_)));
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

    #[test]
    fn on_rescan_preserves_view_and_invalidates_layers() {
        let mut app = sample_app();
        // Expand `src` and select `main.rs` within it.
        select(&mut app, "src");
        app.update(Action::Expand);
        select(&mut app, "main.rs");

        // A new walk that adds `src/new.rs` (so the skeleton differs).
        let files = vec![
            (PathBuf::from("src/main.rs"), 1000),
            (PathBuf::from("README.md"), 200),
            (PathBuf::from("src/new.rs"), 50),
        ];
        let tree = build_skeleton(&files, &[PathBuf::from("src")], "proj".into());
        app.on_rescan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });

        // Expansion preserved (main.rs still visible) and the new file appears.
        assert!(is_visible(&app, "main.rs"));
        assert!(is_visible(&app, "new.rs"));

        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        // Selection preserved by path.
        let selected = loaded.selected_id().unwrap();
        assert_eq!(loaded.tree.nodes[selected].name, "main.rs");
        // The active (code) layer was invalidated and re-requested.
        assert!(loaded.code.is_computing());
        assert_eq!(app.pending_compute, Some(Lens::Code));
    }

    #[test]
    fn on_rescan_is_a_noop_when_the_skeleton_is_unchanged() {
        let mut app = sample_app();
        // Make the code layer Ready, then clear the pending request.
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("src/main.rs"),
            CodeData {
                num: CodeNum {
                    code: 10,
                    comments: 0,
                    blanks: 0,
                },
                ..Default::default()
            },
        );
        app.on_layer(LayerResult::Code {
            files,
            inaccurate: false,
        });
        app.pending_compute = None;

        // Re-walk with an identical skeleton (same paths + sizes).
        let files = vec![
            (PathBuf::from("src/main.rs"), 1000),
            (PathBuf::from("README.md"), 200),
        ];
        let tree = build_skeleton(&files, &[PathBuf::from("src")], "proj".into());
        app.on_rescan(ScanOutcome {
            tree,
            duration: Duration::ZERO,
            repo: false,
        });

        let Screen::Loaded(loaded) = &app.screen else {
            panic!("not loaded");
        };
        // Nothing changed: the layer stays Ready and no recompute is requested.
        assert!(loaded.code_at(loaded.tree.root).is_some());
        assert!(app.pending_compute.is_none());
    }
}
