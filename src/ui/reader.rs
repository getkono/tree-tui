//! The full-screen, read-only file reader.
//!
//! Opened with `Enter` on a file, it replaces `$PAGER` with an in-TUI view built
//! on the `karet-fileview` widget: tree-sitter-highlighted code, inline images,
//! a hex dump for binaries, or a placeholder for PDFs / oversized files — with
//! scroll, search, goto-line, and a line-number gutter. Editing stays delegated
//! to `$EDITOR` (`e`), and `$PAGER` survives as an explicit escape hatch (`P`).
//!
//! The reader owns the suspended [`Loaded`] tree and hands it back on exit, so
//! all tree state (expansion, selection, sort, lens, cached layers) is preserved.
//!
//! `FileViewState` hides its scroll position, so the reader keeps a shadow [`top`]
//! (the viewport's first visible row) in lockstep with it — that mirror is what
//! the title bar, edit-at-line, and half-page scrolling read.
//!
//! [`top`]: Reader::top

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use karet_core::{Decoration, DecorationKind, LineCol, Range, ThemeRole};
use karet_fileview::{FileDoc, FileView, FileViewState, Limits};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::preview::read_bounded;
use super::theme;
use crate::app::Loaded;

/// Largest file the reader loads in full. Well above the preview's 256 KB cap,
/// but bounded so a stray multi-gigabyte file can't exhaust memory.
const MAX_READER_BYTES: u64 = 4 * 1024 * 1024;
/// Above this many lines, render without syntax highlighting so opening stays
/// instant (highlighting is `O(file)` and would otherwise block the loop).
const HIGHLIGHT_LINE_BUDGET: usize = 20_000;

/// Footer key hints when no prompt is active.
const HINTS: &str = "j/k scroll · /n search · :goto · y copy · e edit · P pager · q back";

/// Size and highlight budgets for the full-file reader.
fn reader_limits() -> Limits {
    Limits::new(MAX_READER_BYTES, HIGHLIGHT_LINE_BUDGET)
}

/// A transient footer prompt accumulating typed input.
pub enum Prompt {
    None,
    /// `:` goto-line — the typed digits so far.
    Goto(String),
    /// `/` search — the typed query so far.
    Search(String),
}

/// A compiled search and its matches (zero-based line indices, ascending).
pub struct Search {
    pub matches: Vec<usize>,
    pub current: usize,
}

/// An external handoff requested from inside the reader, drained by the event
/// loop (which owns the terminal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// Edit the file in `$EDITOR`, at `line` when the editor supports `+LINE`.
    EditAtLine { path: PathBuf, line: usize },
    /// View the file in `$PAGER`.
    Pager { path: PathBuf },
}

/// Whether a handled key keeps the reader open or returns to the tree.
pub enum ReaderExit {
    Stay,
    ToTree,
}

/// Full-screen reader state.
pub struct Reader {
    /// The suspended interactive tree, handed back on exit.
    pub loaded: Box<Loaded>,
    /// Absolute path of the file being read.
    pub path: PathBuf,
    /// File size in bytes, for the title bar.
    pub bytes: u64,
    /// The prepared document rendered in the body.
    doc: FileDoc,
    /// Scroll (and image-reserve) state for the view.
    state: FileViewState,
    /// The file's text (text/markdown only), for search and the `y` yank.
    text: Option<String>,
    /// Shadow of the viewport's first visible row, kept in lockstep with
    /// [`state`](Self::state) so the title, edit-at-line, and half-page scroll can
    /// read the position `FileViewState` otherwise hides.
    top: u32,
    /// Body height captured at the last render, for half-page + centering.
    page_rows: u16,
    pub search: Option<Search>,
    /// Search-match line highlights, passed to the view each frame.
    decorations: Vec<Decoration>,
    pub prompt: Prompt,
    /// A queued external handoff, drained by the event loop.
    pub pending_handoff: Option<Handoff>,
    /// Digits accumulated for a `<n>G` goto-line.
    count: String,
    /// Whether a `g` is waiting for the second `g` of `gg`.
    pending_g: bool,
}

impl Reader {
    /// Open `path` (a file) in the reader, taking ownership of the tree state.
    pub fn open(loaded: Box<Loaded>, path: PathBuf) -> Self {
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let limits = reader_limits();
        // Read a bounded prefix; an oversized file yields a head sample and
        // `prepare` classifies it `TooLarge` from `len` (a placeholder).
        let bytes = read_bounded(&path, limits.max_bytes).unwrap_or_default();
        let doc = FileDoc::prepare(&path, &bytes, len, &limits);
        let text = doc
            .language()
            .is_some()
            .then(|| String::from_utf8_lossy(&bytes).into_owned());
        Self {
            loaded,
            path,
            bytes: len,
            doc,
            state: FileViewState::new(),
            text,
            top: 0,
            page_rows: 1,
            search: None,
            decorations: Vec::new(),
            prompt: Prompt::None,
            pending_handoff: None,
            count: String::new(),
            pending_g: false,
        }
    }

    /// Handle a key. Returns whether to stay or return to the tree.
    pub fn handle_key(&mut self, key: KeyEvent) -> ReaderExit {
        // An active prompt swallows input until Enter/Esc.
        match self.prompt {
            Prompt::Goto(_) => return self.handle_goto_key(key),
            Prompt::Search(_) => return self.handle_search_key(key),
            Prompt::None => {}
        }

        // `gg` jumps to the top: a pending first `g` awaits the second.
        if std::mem::take(&mut self.pending_g) && key.code == KeyCode::Char('g') {
            self.goto_top();
            return ReaderExit::Stay;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return ReaderExit::ToTree,
            KeyCode::Char('j') | KeyCode::Down => self.scroll(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll(-1),
            KeyCode::Char('d') if ctrl => self.half_page(1),
            KeyCode::Char('u') if ctrl => self.half_page(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.page(1),
            KeyCode::PageUp | KeyCode::Char('b') => self.page(-1),
            KeyCode::Char('G') | KeyCode::End => self.goto_bottom_or_count(),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('/') => self.prompt = Prompt::Search(String::new()),
            KeyCode::Char(':') => self.prompt = Prompt::Goto(String::new()),
            KeyCode::Char('n') => self.search_step(1),
            KeyCode::Char('N') => self.search_step(-1),
            KeyCode::Char(c @ '0'..='9') => {
                self.count.push(c);
                return ReaderExit::Stay; // keep accumulating digits
            }
            KeyCode::Char('y') => self.yank(),
            KeyCode::Char('e') => self.request_edit(),
            KeyCode::Char('P') => self.request_pager(),
            _ => {}
        }
        self.count.clear();
        ReaderExit::Stay
    }

    /// The last scrollable row (text line or hex row), clamped to at least 0.
    fn max_top(&self) -> u32 {
        (self.doc.row_count() as u32).saturating_sub(1)
    }

    /// Move the viewport's top to `line` (clamped to the document), keeping
    /// [`state`](Self::state) and the [`top`](Self::top) shadow in sync.
    fn set_top(&mut self, line: u32) {
        let line = line.min(self.max_top());
        if line >= self.top {
            self.state.scroll_down(line - self.top);
        } else {
            self.state.scroll_up(self.top - line);
        }
        self.top = line;
    }

    /// Scroll the view by `delta` rows. `pub(crate)` so the event loop can route
    /// mouse-wheel scrolls here, the same as in the tree/preview.
    pub(crate) fn scroll(&mut self, delta: i32) {
        let next = (i64::from(self.top) + i64::from(delta)).max(0);
        self.set_top(next as u32);
    }

    fn page(&mut self, dir: i32) {
        let step = i32::from(self.page_rows.max(1));
        self.scroll(step * dir);
    }

    fn half_page(&mut self, dir: i32) {
        let step = i32::from((self.page_rows / 2).max(1));
        self.scroll(step * dir);
    }

    fn goto_top(&mut self) {
        self.set_top(0);
    }

    fn goto_bottom(&mut self) {
        self.set_top(self.max_top());
    }

    /// Center the viewport on a 1-based line.
    fn goto_line(&mut self, one_based: usize) {
        let line = one_based.saturating_sub(1) as u32;
        let half = u32::from(self.page_rows) / 2;
        self.set_top(line.saturating_sub(half));
    }

    fn goto_bottom_or_count(&mut self) {
        match self.count.parse::<usize>().ok() {
            Some(n) => self.goto_line(n),
            None => self.goto_bottom(),
        }
    }

    fn yank(&mut self) {
        if let Some(text) = &self.text
            && !text.is_empty()
        {
            crate::clipboard::osc52_copy(text);
        }
    }

    fn request_edit(&mut self) {
        self.pending_handoff = Some(Handoff::EditAtLine {
            path: self.path.clone(),
            line: self.top as usize + 1,
        });
    }

    fn request_pager(&mut self) {
        self.pending_handoff = Some(Handoff::Pager {
            path: self.path.clone(),
        });
    }

    fn handle_goto_key(&mut self, key: KeyEvent) -> ReaderExit {
        match key.code {
            KeyCode::Esc => self.prompt = Prompt::None,
            KeyCode::Enter => {
                let target = match &self.prompt {
                    Prompt::Goto(buf) => buf.parse::<usize>().ok(),
                    _ => None,
                };
                self.prompt = Prompt::None;
                if let Some(n) = target {
                    self.goto_line(n);
                }
            }
            KeyCode::Backspace => {
                if let Prompt::Goto(buf) = &mut self.prompt {
                    buf.pop();
                }
            }
            KeyCode::Char(c @ '0'..='9') => {
                if let Prompt::Goto(buf) = &mut self.prompt {
                    buf.push(c);
                }
            }
            _ => {}
        }
        ReaderExit::Stay
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> ReaderExit {
        match key.code {
            KeyCode::Esc => self.prompt = Prompt::None,
            KeyCode::Enter => {
                let query = match &self.prompt {
                    Prompt::Search(buf) => buf.clone(),
                    _ => String::new(),
                };
                self.prompt = Prompt::None;
                self.run_search(&query);
            }
            KeyCode::Backspace => {
                if let Prompt::Search(buf) = &mut self.prompt {
                    buf.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Prompt::Search(buf) = &mut self.prompt {
                    buf.push(c);
                }
            }
            _ => {}
        }
        ReaderExit::Stay
    }

    /// Zero-based indices of the text lines containing `needle` (already
    /// lowercased). Empty for non-text documents.
    fn matching_lines(&self, needle: &str) -> Vec<usize> {
        let Some(text) = &self.text else {
            return Vec::new();
        };
        text.lines()
            .enumerate()
            .filter(|(_, line)| line.to_lowercase().contains(needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// Run a case-insensitive search, jumping to the first match at or after the
    /// current top line.
    fn run_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            self.decorations.clear();
            return;
        }
        let needle = query.to_lowercase();
        let matches = self.matching_lines(&needle);
        if matches.is_empty() {
            self.decorations.clear();
            self.search = Some(Search {
                matches,
                current: 0,
            });
            return;
        }
        let current = matches
            .iter()
            .position(|&l| l as u32 >= self.top)
            .unwrap_or(0);
        let line = matches[current];
        self.search = Some(Search { matches, current });
        self.rebuild_decorations();
        self.goto_line(line + 1);
    }

    /// Advance to the next (`dir > 0`) or previous match, wrapping.
    fn search_step(&mut self, dir: i32) {
        let line = {
            let Some(search) = &mut self.search else {
                return;
            };
            if search.matches.is_empty() {
                return;
            }
            let len = search.matches.len() as i32;
            search.current = (search.current as i32 + dir).rem_euclid(len) as usize;
            search.matches[search.current]
        };
        self.goto_line(line + 1);
    }

    /// Rebuild the whole-line match highlights from the current search.
    fn rebuild_decorations(&mut self) {
        self.decorations = self
            .search
            .as_ref()
            .map(|search| {
                search
                    .matches
                    .iter()
                    .filter_map(|&line| {
                        let line = line as u32;
                        Range::new(LineCol::new(line, 0), LineCol::new(line + 1, 0))
                            .ok()
                            .map(|range| Decoration {
                                range,
                                kind: DecorationKind::LineBackground,
                                role: Some(ThemeRole::SearchMatch),
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
}

/// Render the full-screen reader into `area`.
pub fn render(frame: &mut Frame, reader: &mut Reader, area: Rect) {
    let [title_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    reader.page_rows = body_area.height;
    render_title(frame, reader, title_area);
    // Disjoint borrows: `doc`/`decorations` (shared) and `state` (mutable).
    frame.render_stateful_widget(
        FileView::new(&reader.doc).decorations(&reader.decorations),
        body_area,
        &mut reader.state,
    );
    render_footer(frame, reader, footer_area);
}

fn render_title(frame: &mut Frame, reader: &Reader, area: Rect) {
    let mut left = vec![Span::styled(
        format!(" {}", reader.path.display()),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(lang) = reader.doc.language() {
        left.push(Span::styled(
            format!("  {lang}"),
            Style::default().fg(theme::MUTED),
        ));
    }

    // The line position is meaningful only for the scrollable text branch.
    let position = if reader.doc.language().is_some() {
        format!("ln {}/{}", reader.top + 1, reader.doc.row_count().max(1))
    } else {
        String::new()
    };
    let right = Line::from(Span::styled(
        format!("{}  {position} ", theme::human_bytes(reader.bytes)),
        Style::default().fg(theme::MUTED),
    ));

    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Paragraph::new(Line::from(left)), left_area);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
}

fn render_footer(frame: &mut Frame, reader: &Reader, area: Rect) {
    let left = match &reader.prompt {
        Prompt::Goto(buf) => Line::from(Span::styled(
            format!(" :{buf}"),
            Style::default().fg(theme::ACCENT),
        )),
        Prompt::Search(buf) => Line::from(Span::styled(
            format!(" /{buf}"),
            Style::default().fg(theme::ACCENT),
        )),
        Prompt::None => Line::from(Span::styled(
            format!(" {HINTS}"),
            Style::default().fg(theme::MUTED),
        )),
    };

    let right = match &reader.search {
        Some(search) if !search.matches.is_empty() => {
            format!("match {}/{} ", search.current + 1, search.matches.len())
        }
        Some(_) => "no matches ".to_string(),
        None => String::new(),
    };
    let right = Line::from(Span::styled(right, Style::default().fg(theme::MUTED)));

    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A reader over synthetic text, bypassing the filesystem.
    fn reader_with(lines: usize) -> Reader {
        let source: String = (0..lines).map(|i| format!("line {i} content\n")).collect();
        let bytes = source.into_bytes();
        let doc = FileDoc::prepare(
            Path::new("/proj/src/main.rs"),
            &bytes,
            bytes.len() as u64,
            &reader_limits(),
        );
        let text = doc
            .language()
            .is_some()
            .then(|| String::from_utf8_lossy(&bytes).into_owned());
        Reader {
            loaded: Box::new(loaded_stub()),
            path: PathBuf::from("/proj/src/main.rs"),
            bytes: bytes.len() as u64,
            doc,
            state: FileViewState::new(),
            text,
            top: 0,
            page_rows: 1,
            search: None,
            decorations: Vec::new(),
            prompt: Prompt::None,
            pending_handoff: None,
            count: String::new(),
            pending_g: false,
        }
    }

    /// A throwaway `Loaded` so a `Reader` can be built in tests.
    fn loaded_stub() -> Loaded {
        use crate::model::build_skeleton;
        let tree = build_skeleton(&[(PathBuf::from("src/main.rs"), 1)], &[], "proj".into());
        let mut app = crate::app::App::new(PathBuf::from("/proj"), "proj".into());
        app.on_scan(crate::scan::ScanOutcome {
            tree,
            duration: std::time::Duration::ZERO,
            repo: false,
            head: None,
        });
        let crate::app::Screen::Loaded(loaded) = app.screen else {
            unreachable!()
        };
        *loaded
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Drive the view through one render so its viewport is known, then read the
    /// (shadowed) top line.
    fn rendered_top(reader: &mut Reader) -> usize {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| render(frame, reader, frame.area()))
            .unwrap();
        reader.top as usize
    }

    #[test]
    fn q_and_esc_return_to_the_tree() {
        let mut reader = reader_with(10);
        assert!(matches!(
            reader.handle_key(key(KeyCode::Char('q'))),
            ReaderExit::ToTree
        ));
        assert!(matches!(
            reader.handle_key(key(KeyCode::Esc)),
            ReaderExit::ToTree
        ));
    }

    #[test]
    fn goto_prompt_jumps_to_a_line() {
        let mut reader = reader_with(500);
        rendered_top(&mut reader);
        reader.handle_key(key(KeyCode::Char(':')));
        for c in "200".chars() {
            reader.handle_key(key(KeyCode::Char(c)));
        }
        reader.handle_key(key(KeyCode::Enter));
        let top = rendered_top(&mut reader);
        // line 200 (1-based) centered in a ~10-row body lands near index 194.
        assert!(top > 180 && top < 200, "unexpected top {top}");
    }

    #[test]
    fn count_prefix_goto_with_capital_g() {
        let mut reader = reader_with(500);
        rendered_top(&mut reader);
        for c in "100".chars() {
            reader.handle_key(key(KeyCode::Char(c)));
        }
        reader.handle_key(key(KeyCode::Char('G')));
        let top = rendered_top(&mut reader);
        assert!(top > 80 && top < 100, "unexpected top {top}");
        // the count is consumed
        assert!(reader.count.is_empty());
    }

    #[test]
    fn esc_closes_the_prompt_without_leaving() {
        let mut reader = reader_with(10);
        reader.handle_key(key(KeyCode::Char(':')));
        assert!(matches!(
            reader.handle_key(key(KeyCode::Esc)),
            ReaderExit::Stay
        ));
        assert!(matches!(reader.prompt, Prompt::None));
    }

    #[test]
    fn search_finds_navigates_and_wraps() {
        let mut reader = reader_with(10); // "line 0 content" .. "line 9 content"
        rendered_top(&mut reader);
        reader.handle_key(key(KeyCode::Char('/')));
        for c in "line 3".chars() {
            reader.handle_key(key(KeyCode::Char(c)));
        }
        reader.handle_key(key(KeyCode::Enter));
        let search = reader.search.as_ref().unwrap();
        assert_eq!(search.matches, vec![3]); // only "line 3 content" matches
        assert_eq!(search.current, 0);

        // n wraps around a single match.
        reader.handle_key(key(KeyCode::Char('n')));
        assert_eq!(reader.search.as_ref().unwrap().current, 0);
    }

    #[test]
    fn search_with_no_match_is_recorded_but_empty() {
        let mut reader = reader_with(10);
        rendered_top(&mut reader);
        reader.handle_key(key(KeyCode::Char('/')));
        for c in "zzz".chars() {
            reader.handle_key(key(KeyCode::Char(c)));
        }
        reader.handle_key(key(KeyCode::Enter));
        assert!(reader.search.as_ref().unwrap().matches.is_empty());
    }

    #[test]
    fn edit_requests_the_current_top_line() {
        let mut reader = reader_with(500);
        rendered_top(&mut reader);
        reader.handle_key(key(KeyCode::Char('G'))); // jump to the bottom
        let top = rendered_top(&mut reader);
        reader.handle_key(key(KeyCode::Char('e')));
        assert_eq!(
            reader.pending_handoff,
            Some(Handoff::EditAtLine {
                path: PathBuf::from("/proj/src/main.rs"),
                line: top + 1,
            })
        );
    }

    #[test]
    fn pager_handoff_is_offered() {
        let mut reader = reader_with(1);
        reader.handle_key(key(KeyCode::Char('P')));
        assert_eq!(
            reader.pending_handoff,
            Some(Handoff::Pager {
                path: PathBuf::from("/proj/src/main.rs"),
            })
        );
    }

    #[test]
    fn renders_without_panicking() {
        let mut reader = reader_with(50);
        rendered_top(&mut reader); // a normal-size draw
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        // A tiny terminal must not panic on layout underflow.
        let mut tiny = Terminal::new(TestBackend::new(20, 4)).unwrap();
        tiny.draw(|frame| render(frame, &mut reader, frame.area()))
            .unwrap();
    }
}
