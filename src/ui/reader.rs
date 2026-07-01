//! The full-screen, read-only file reader.
//!
//! Opened with `Enter` on a file, it replaces `$PAGER` with an in-TUI,
//! syntax-highlighted view: scroll, search, goto-line, horizontal pan, and a
//! line-number gutter (all via the shared [`CodeView`]). Editing stays delegated
//! to `$EDITOR` (`e`), and `$PAGER` survives as an explicit escape hatch (`P`)
//! for binary or oversized files.
//!
//! The reader owns the suspended [`Loaded`] tree and hands it back on exit, so
//! all tree state (expansion, selection, sort, lens, cached layers) is preserved.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use super::codeview::CodeView;
use super::{preview, syntax, theme};
use crate::app::Loaded;

/// Largest file the reader loads in full. Well above the preview's 256 KB cap,
/// but bounded so a stray multi-gigabyte file can't exhaust memory.
const MAX_READER_BYTES: u64 = 4 * 1024 * 1024;
/// Above this many lines, render without syntax highlighting so opening stays
/// instant (highlighting is `O(file)` and would otherwise block the loop).
const HIGHLIGHT_LINE_BUDGET: usize = 20_000;

/// Footer key hints when no prompt is active.
const HINTS: &str = "j/k scroll · /n search · :goto · h/l pan · y copy · e edit · P pager · q back";

/// What the reader is showing.
pub enum Content {
    /// Highlighted (or, for very large files, plain) scrollable text.
    Text(CodeView),
    /// A decoded image filling the body.
    Image(Box<StatefulProtocol>),
    /// A note (binary / too large / unreadable / empty) — `P` opens `$PAGER`.
    Info(String),
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
    /// Detected language label for the title bar, if known.
    pub lang: Option<String>,
    /// File size in bytes, for the title bar.
    pub bytes: u64,
    pub content: Content,
    pub prompt: Prompt,
    pub search: Option<Search>,
    /// A queued external handoff, drained by the event loop.
    pub pending_handoff: Option<Handoff>,
    /// Digits accumulated for a `<n>G` goto-line.
    count: String,
    /// Whether a `g` is waiting for the second `g` of `gg`.
    pending_g: bool,
}

impl Reader {
    /// Open `path` (a file) in the reader, taking ownership of the tree state.
    pub fn open(loaded: Box<Loaded>, path: PathBuf, picker: Option<&Picker>) -> Self {
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (content, lang) = load(&path, bytes, picker);
        Self {
            loaded,
            path,
            lang,
            bytes,
            content,
            prompt: Prompt::None,
            search: None,
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
            if let Some(cv) = self.code_view_mut() {
                cv.goto_top();
            }
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
            KeyCode::Char('l') | KeyCode::Right => self.scroll_h(4),
            KeyCode::Char('h') | KeyCode::Left => self.scroll_h(-4),
            KeyCode::Home => self.scroll_h(i32::MIN),
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

    fn code_view(&self) -> Option<&CodeView> {
        match &self.content {
            Content::Text(cv) => Some(cv),
            _ => None,
        }
    }

    fn code_view_mut(&mut self) -> Option<&mut CodeView> {
        match &mut self.content {
            Content::Text(cv) => Some(cv),
            _ => None,
        }
    }

    /// Scroll the view by `delta` lines. `pub(crate)` so the event loop can
    /// route mouse-wheel scrolls here, the same as in the tree/preview.
    pub(crate) fn scroll(&mut self, delta: i32) {
        if let Some(cv) = self.code_view_mut() {
            cv.scroll_by(delta);
        }
    }

    fn page(&mut self, dir: i32) {
        if let Some(cv) = self.code_view_mut() {
            cv.page(dir);
        }
    }

    fn half_page(&mut self, dir: i32) {
        if let Some(cv) = self.code_view_mut() {
            let step = (cv.viewport_rows() / 2).max(1) as i32;
            cv.scroll_by(step * dir);
        }
    }

    fn scroll_h(&mut self, delta: i32) {
        if let Some(cv) = self.code_view_mut() {
            cv.scroll_h(delta);
        }
    }

    fn goto_bottom_or_count(&mut self) {
        let line = self.count.parse::<usize>().ok();
        if let Some(cv) = self.code_view_mut() {
            match line {
                Some(n) => cv.goto_line(n),
                None => cv.goto_bottom(),
            }
        }
    }

    fn yank(&mut self) {
        if let Some(cv) = self.code_view() {
            let text = cv.visible_text();
            if !text.is_empty() {
                crate::clipboard::osc52_copy(&text);
            }
        }
    }

    fn request_edit(&mut self) {
        let line = self.code_view().map_or(1, |cv| cv.top() + 1);
        self.pending_handoff = Some(Handoff::EditAtLine {
            path: self.path.clone(),
            line,
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
                if let Some(n) = target
                    && let Some(cv) = self.code_view_mut()
                {
                    cv.goto_line(n);
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

    /// Run a case-insensitive search, jumping to the first match at or after the
    /// current top line.
    fn run_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            if let Some(cv) = self.code_view_mut() {
                cv.clear_matches();
            }
            return;
        }
        let needle = query.to_lowercase();
        let matches = self
            .code_view()
            .map(|cv| cv.matching_lines(&needle))
            .unwrap_or_default();
        if matches.is_empty() {
            if let Some(cv) = self.code_view_mut() {
                cv.clear_matches();
            }
            self.search = Some(Search {
                matches,
                current: 0,
            });
            return;
        }
        let top = self.code_view().map_or(0, CodeView::top);
        let current = matches.iter().position(|&l| l >= top).unwrap_or(0);
        let line = matches[current];
        if let Some(cv) = self.code_view_mut() {
            cv.set_matches(&matches, Some(line));
            cv.goto_line(line + 1);
        }
        self.search = Some(Search { matches, current });
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
        if let Some(cv) = self.code_view_mut() {
            cv.set_current_match(Some(line));
            cv.goto_line(line + 1);
        }
    }
}

/// Load the reader content for `path`: an image, highlighted text, or a note.
fn load(path: &Path, bytes: u64, picker: Option<&Picker>) -> (Content, Option<String>) {
    if image::ImageFormat::from_path(path).is_ok() {
        let content = match preview::decode_image(path, picker) {
            Ok(protocol) => Content::Image(protocol),
            Err(message) => Content::Info(message),
        };
        return (content, None);
    }
    load_text(path, bytes)
}

fn load_text(path: &Path, bytes: u64) -> (Content, Option<String>) {
    use std::io::Read;

    if bytes > MAX_READER_BYTES {
        return (
            Content::Info(format!(
                "file too large for the reader ({}) — press P to open in $PAGER",
                theme::human_bytes(bytes)
            )),
            None,
        );
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => return (Content::Info(format!("cannot read file: {err}")), None),
    };
    let mut buf = Vec::new();
    if let Err(err) = file.take(MAX_READER_BYTES).read_to_end(&mut buf) {
        return (Content::Info(format!("cannot read file: {err}")), None);
    }
    // A NUL byte reliably signals a binary file.
    if buf.contains(&0) {
        return (
            Content::Info("binary file — press P to open in $PAGER".into()),
            None,
        );
    }
    let text = String::from_utf8_lossy(&buf);
    if text.trim().is_empty() {
        return (Content::Info("empty file".into()), None);
    }
    let lang = syntax::language_name(path, &text);
    // Highlight small files; render very large ones plain so opening is instant.
    let lines = if text.lines().count() <= HIGHLIGHT_LINE_BUDGET {
        syntax::highlight(&text, path)
    } else {
        text.lines().map(|l| Line::from(l.to_string())).collect()
    };
    (Content::Text(CodeView::new(lines)), lang)
}

/// Render the full-screen reader into `area`.
pub fn render(frame: &mut Frame, reader: &mut Reader, area: Rect) {
    let [title_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_title(frame, reader, title_area);
    match &mut reader.content {
        Content::Text(cv) => super::codeview::render(frame, cv, body_area, "", true),
        Content::Image(protocol) => {
            frame.render_stateful_widget(
                StatefulImage::<StatefulProtocol>::new(),
                body_area,
                protocol.as_mut(),
            );
        }
        Content::Info(message) => {
            let note = Paragraph::new(Line::from(Span::styled(
                message.clone(),
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::ITALIC),
            )))
            .alignment(Alignment::Center);
            frame.render_widget(note, body_area);
        }
    }
    render_footer(frame, reader, footer_area);
}

fn render_title(frame: &mut Frame, reader: &Reader, area: Rect) {
    let mut left = vec![Span::styled(
        format!(" {}", reader.path.display()),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(lang) = &reader.lang {
        left.push(Span::styled(
            format!("  {lang}"),
            Style::default().fg(theme::MUTED),
        ));
    }

    let position = match reader.content {
        Content::Text(ref cv) => format!("ln {}/{}", cv.top() + 1, cv.line_count().max(1)),
        _ => String::new(),
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
    use super::*;
    use ratatui::text::Line;

    /// A reader over synthetic text, bypassing the filesystem.
    fn reader_with(lines: usize) -> Reader {
        let text: Vec<Line<'static>> = (0..lines)
            .map(|i| Line::from(format!("line {i} content")))
            .collect();
        Reader {
            loaded: Box::new(loaded_stub()),
            path: PathBuf::from("/proj/src/main.rs"),
            lang: Some("Rust".into()),
            bytes: 1234,
            content: Content::Text(CodeView::new(text)),
            prompt: Prompt::None,
            search: None,
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

    /// Drive the view through one render so its viewport is known, then read it.
    fn rendered_top(reader: &mut Reader) -> usize {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| render(frame, reader, frame.area()))
            .unwrap();
        match &reader.content {
            Content::Text(cv) => cv.top(),
            _ => 0,
        }
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
        // line 200 (1-based) centered in a ~10-row body lands near index 199.
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
    fn binary_offers_the_pager() {
        // A reader whose content is a binary note: P queues the pager.
        let mut reader = reader_with(1);
        reader.content = Content::Info("binary file — press P to open in $PAGER".into());
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
