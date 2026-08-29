//! The "render any file" view: one classify-and-dispatch layer over karet.
//!
//! `karet-fileview` ships only the read-only *primitives* — a hex dump, a terminal
//! image renderer, and a placeholder — plus `classify`. Composing them with the
//! read-only editor and the tree-sitter stack into a single widget that renders
//! whatever file is selected is this module's job.
//!
//! Two types split the cost. [`FileDoc::prepare`] is the expensive step — classify
//! the bytes, then decode / parse / highlight into an owned payload — and runs once
//! per opened file. [`FileView`] is the cheap per-frame renderer that dispatches
//! that payload to the right primitive, carrying its scroll position in
//! [`FileViewState`].
//!
//! Images render through the Kitty graphics protocol when the terminal speaks it
//! and truecolor half-blocks otherwise. The Kitty path paints nothing into the
//! ratatui buffer: it *reserves* a rect and leaves [`flush_kitty_image`] to
//! transmit the pixels after `terminal.draw` (see `crate::event`).

#[cfg(feature = "raster")]
use std::io;
#[cfg(feature = "raster")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(any(feature = "images", feature = "pdf"))]
use karet_core::ThemeRole;
use karet_core::{Decoration, FoldRegions};
use karet_editor::{Editor, EditorState, Fold};
use karet_filetype::{FileKind, classify_with_guard};
use karet_fileview::HexView;
use karet_fileview::image;
use karet_fileview::image::GraphicsProtocol;
#[cfg(feature = "images")]
use karet_fileview::image::ImageWidget;
use karet_fileview::viewer::Placeholder;
use karet_syntax::{Highlights, LayeredHighlighter, SemanticBlocker, SemanticBlocks};
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_treesitter::{LayeredParser, language_id_from_path, language_name_from_path};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
#[cfg(any(feature = "images", feature = "pdf"))]
use ratatui::style::Style;
use ratatui::widgets::{StatefulWidget, Widget};

/// How many leading bytes to sample for file-type classification.
const HEAD_BYTES: usize = 8192;

/// The bytes shown per hex row (matches `karet_fileview::HexView`).
const HEX_ROW_WIDTH: usize = 16;

/// The scale at which PDF pages are rasterized. Rendered larger than a typical
/// pane so the Kitty protocol downscales (sharp) rather than upscales (blurry)
/// into the reserved cell box; 2.0 ≈ 144 DPI for a native 72-DPI page.
#[cfg(feature = "pdf")]
const PDF_RENDER_SCALE: f32 = 2.0;

/// Size and highlighting budgets for [`FileDoc::prepare`].
///
/// The two budgets are independent knobs tuned per context: the inline preview
/// uses a small `max_bytes` and a low `highlight_line_budget` for an instant open,
/// the full-screen reader a much larger pair.
///
/// Nothing in karet enforces the line budget any more — it is ours to apply, and
/// [`prepare`](FileDoc::prepare) is what applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Files whose length exceeds this are [`FileKind::TooLarge`] (a placeholder).
    pub max_bytes: u64,
    /// Text with more lines than this is rendered without syntax highlighting so
    /// it opens instantly; the buffer itself is unaffected.
    pub highlight_line_budget: usize,
}

impl Default for Limits {
    /// A general-purpose default: `karet_filetype::SIZE_GUARD` (10 MiB) and a
    /// 20 000-line highlight budget.
    fn default() -> Self {
        Self::new(karet_filetype::SIZE_GUARD, 20_000)
    }
}

impl Limits {
    /// Construct limits from an explicit byte ceiling and highlight line budget.
    #[must_use]
    pub fn new(max_bytes: u64, highlight_line_budget: usize) -> Self {
        Self {
            max_bytes,
            highlight_line_budget,
        }
    }
}

/// The prepared, owned payload for each renderable branch.
enum Content {
    /// Text/Markdown: a read-only buffer plus its (possibly empty) syntax model.
    Text {
        buffer: TextBuffer,
        highlights: Highlights,
        blocks: SemanticBlocks,
        folds: FoldRegions,
        language: &'static str,
    },
    /// A decoded raster image.
    #[cfg(feature = "images")]
    Image(image::Image),
    /// A parsed PDF; pages are rasterized on demand while rendering.
    #[cfg(feature = "pdf")]
    Document {
        doc: karet_pdf::Document,
        page_count: usize,
    },
    /// Raw bytes shown as a hex dump.
    Binary(Vec<u8>),
    /// Nothing to render inline (too-large, undecodable, or a format whose engine
    /// we don't ship).
    Placeholder,
}

/// A file classified and prepared for read-only rendering.
///
/// Build one with [`prepare`](Self::prepare) — the expensive step, run once per
/// opened file — and render it with [`FileView`]. The document owns everything a
/// frame needs, so per-frame rendering allocates almost nothing.
pub struct FileDoc {
    kind: FileKind,
    content: Content,
    path: PathBuf,
    dims: Option<(u32, u32)>,
    len: u64,
}

impl FileDoc {
    /// Prepare `path`'s content for rendering: classify the bytes against `limits`,
    /// then decode an image / parse a PDF / parse+highlight text / keep raw bytes
    /// for a hex dump / fall back to a placeholder.
    ///
    /// `bytes` should be the file's content with `len` its true size — except when
    /// `len > limits.max_bytes`, where the file classifies [`FileKind::TooLarge`]
    /// without inspecting `bytes` beyond a leading sample, so a caller may pass
    /// only a head slice for a very large file.
    #[must_use]
    pub fn prepare(path: &Path, bytes: &[u8], len: u64, limits: &Limits) -> Self {
        let head = &bytes[..bytes.len().min(HEAD_BYTES)];
        let kind = classify_with_guard(path, head, len, limits.max_bytes);
        let content = match kind {
            FileKind::Text | FileKind::Markdown => prepare_text(path, bytes, limits),
            #[cfg(feature = "images")]
            FileKind::Image => match image::decode(bytes) {
                Ok(img) => Content::Image(img),
                Err(_) => Content::Placeholder,
            },
            #[cfg(feature = "pdf")]
            FileKind::Pdf => match karet_pdf::Document::load(bytes.to_vec()) {
                Ok(doc) => {
                    let page_count = doc.page_count();
                    Content::Document { doc, page_count }
                }
                Err(_) => Content::Placeholder,
            },
            FileKind::Binary => Content::Binary(bytes.to_vec()),
            // TooLarge, CBOR, DOCX, notebooks, images/PDFs with their feature off,
            // and any future `#[non_exhaustive]` kind → a placeholder naming it.
            _ => Content::Placeholder,
        };
        // Annotate an undecodable-image placeholder with the pixel dimensions.
        #[cfg(feature = "images")]
        let dims = (matches!(kind, FileKind::Image) && matches!(content, Content::Placeholder))
            .then(|| image::dimensions(bytes))
            .flatten();
        #[cfg(not(feature = "images"))]
        let dims = None;
        Self {
            kind,
            content,
            path: path.to_path_buf(),
            dims,
            len,
        }
    }

    /// The display language name for a text file (e.g. `"Rust"`), or `None` for
    /// every non-text branch. Doubles as the "is this scrollable text?" probe.
    #[must_use]
    pub fn language(&self) -> Option<&'static str> {
        match &self.content {
            Content::Text { language, .. } => Some(language),
            _ => None,
        }
    }

    /// The number of scrollable units — text lines or hex rows — or `0` for the
    /// image / document / placeholder branches.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match &self.content {
            Content::Text { buffer, .. } => buffer.line_count(),
            Content::Binary(bytes) => bytes.len().div_ceil(HEX_ROW_WIDTH),
            _ => 0,
        }
    }

    /// The page count for a PDF, or `None` for every other kind.
    #[must_use]
    pub fn page_count(&self) -> Option<usize> {
        match &self.content {
            #[cfg(feature = "pdf")]
            Content::Document { page_count, .. } => Some(*page_count),
            _ => None,
        }
    }

    /// The document's fold regions (empty for a non-text branch, or when the file
    /// exceeded the highlight budget).
    #[must_use]
    pub fn fold_regions(&self) -> Option<&FoldRegions> {
        match &self.content {
            Content::Text { folds, .. } => Some(folds),
            _ => None,
        }
    }
}

/// Build the text branch: a read-only buffer plus the syntax model, skipping the
/// expensive tree-sitter pass when the file exceeds the line budget.
fn prepare_text(path: &Path, bytes: &[u8], limits: &Limits) -> Content {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let language = language_name_from_path(path).unwrap_or("plaintext");
    let buffer = TextBuffer::from_text(&text);
    let (highlights, blocks, folds) = if buffer.line_count() <= limits.highlight_line_budget {
        analyze(path, &text)
    } else {
        Default::default()
    };
    Content::Text {
        buffer,
        highlights,
        blocks,
        folds,
        language,
    }
}

/// Parse `text` for `path`'s language and derive its highlights, semantic blocks,
/// and fold regions. Empty models when no grammar is compiled in or parsing fails.
///
/// The parse is *layered*, so a language injected into another — a fenced code
/// block in markdown, or a doc-comment's markdown in Rust — is highlighted too.
fn analyze(path: &Path, text: &str) -> (Highlights, SemanticBlocks, FoldRegions) {
    let Some(lang) = language_id_from_path(path) else {
        return Default::default();
    };
    let Ok(tree) = LayeredParser::new().parse(lang, text) else {
        return Default::default();
    };
    let highlights = LayeredHighlighter::new().highlight(&tree, text);
    let blocks = SemanticBlocker::new().analyze(tree.root(), text);
    let folds = karet_syntax::fold(tree.root());
    (highlights, blocks, folds)
}

/// The terminal's image protocol, detected once.
///
/// Detection is env-only (no stdin probe), so it cannot race crossterm's event
/// reader — the pitfall that froze input under `ratatui-image`'s query picker.
pub fn graphics_protocol() -> GraphicsProtocol {
    static PROTOCOL: OnceLock<GraphicsProtocol> = OnceLock::new();
    *PROTOCOL.get_or_init(image::detect_protocol)
}

/// The persistent per-view state: scroll position and, for the Kitty image path,
/// the rect reserved this frame (see [`flush_kitty_image`]).
///
/// The scroll helpers drive the text and hex branches at once; only the active
/// branch's scroll is read when rendering, so a caller can move the viewport
/// without knowing the document's kind.
#[derive(Clone, Debug, Default)]
pub struct FileViewState {
    /// Scroll state for the read-only text branch.
    editor: EditorState,
    /// First visible 16-byte row for the hex branch.
    hex_scroll: usize,
    /// Viewport height captured at the last render, for page scrolling.
    page: u16,
    /// The rect reserved for a Kitty image this frame, consumed by
    /// [`flush_kitty_image`]. `None` for every other branch/protocol.
    pending_image: Option<Rect>,
    /// The current 0-based page for a PDF branch.
    doc_page: usize,
    /// The most recently rasterized PDF page — `(page index, image)` — so
    /// scrolling and resizing don't re-rasterize the same page every frame.
    #[cfg(feature = "pdf")]
    rendered: Option<(usize, image::Image)>,
}

impl FileViewState {
    /// A fresh state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The viewport's first visible row — the text line or hex row at the top.
    #[must_use]
    pub fn top(&self) -> u32 {
        self.editor.scroll_line
    }

    /// Scroll down by `lines` (text lines, or hex rows). Clamped when rendered.
    pub fn scroll_down(&mut self, lines: u32) {
        self.editor.scroll_line = self.editor.scroll_line.saturating_add(lines);
        self.hex_scroll = self.hex_scroll.saturating_add(lines as usize);
    }

    /// Scroll up by `lines` (text lines, or hex rows).
    pub fn scroll_up(&mut self, lines: u32) {
        self.editor.scroll_line = self.editor.scroll_line.saturating_sub(lines);
        self.hex_scroll = self.hex_scroll.saturating_sub(lines as usize);
    }

    /// Scroll down one viewport page.
    pub fn page_down(&mut self) {
        self.scroll_down(u32::from(self.page.max(1)));
    }

    /// Scroll up one viewport page.
    pub fn page_up(&mut self) {
        self.scroll_up(u32::from(self.page.max(1)));
    }

    /// Jump to the top of the document.
    pub fn scroll_to_top(&mut self) {
        self.editor.scroll_line = 0;
        self.hex_scroll = 0;
    }

    /// Put `line` at the top of the viewport. Clamped when rendered.
    pub fn scroll_to_line(&mut self, line: u32) {
        self.editor.scroll_line = line;
        self.hex_scroll = line as usize;
    }

    /// The body height captured at the last render, for half-page scrolling.
    #[must_use]
    pub fn page_rows(&self) -> u16 {
        self.page.max(1)
    }

    /// Forget any Kitty reservation, for a view that is no longer on screen.
    ///
    /// [`FileView::render`] clears this at the start of every frame, so it only
    /// needs calling for a view that is *not* rendered — otherwise the last
    /// reservation would outlive the pane and paint over whatever replaced it.
    pub fn clear_pending_image(&mut self) {
        self.pending_image = None;
    }

    /// Advance to the next page of a PDF. Clamped to the last page when rendered;
    /// a no-op for every other branch.
    pub fn next_page(&mut self) {
        self.doc_page = self.doc_page.saturating_add(1);
    }

    /// Go back to the previous page of a PDF.
    pub fn prev_page(&mut self) {
        self.doc_page = self.doc_page.saturating_sub(1);
    }

    /// The current 0-based PDF page (see [`next_page`](Self::next_page)).
    #[must_use]
    pub fn doc_page(&self) -> usize {
        self.doc_page
    }
}

/// A read-only widget that renders any [`FileDoc`] — highlighted text, an image, a
/// PDF page, a hex dump, or a placeholder — dispatching on the document's kind.
///
/// Search matches (or any overlay) are supplied as [`Decoration`]s and painted on
/// the text branch. For the Kitty image path, call [`flush_kitty_image`] once
/// after `terminal.draw(...)`; the half-block path is self-contained.
pub struct FileView<'a> {
    doc: &'a FileDoc,
    theme: &'a Theme,
    protocol: GraphicsProtocol,
    decorations: &'a [Decoration],
    folds: &'a [Fold],
    word_wrap: bool,
    sticky_scroll: bool,
}

impl<'a> FileView<'a> {
    /// Start building a view over `doc`, using the shared editor theme.
    #[must_use]
    pub fn new(doc: &'a FileDoc) -> Self {
        Self {
            doc,
            theme: super::theme::editor_theme(),
            protocol: GraphicsProtocol::Halfblocks,
            decorations: &[],
            folds: &[],
            word_wrap: false,
            sticky_scroll: false,
        }
    }

    /// Select the terminal graphics protocol for images and PDF pages. Defaults to
    /// half-blocks, which render inline with no flush.
    #[must_use]
    pub fn graphics(mut self, protocol: GraphicsProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Supply decorations painted on the text branch — e.g. search matches.
    #[must_use]
    pub fn decorations(mut self, decorations: &'a [Decoration]) -> Self {
        self.decorations = decorations;
        self
    }

    /// Supply the resolved fold list for the text branch (see
    /// [`karet_editor::resolve_folds`]).
    #[must_use]
    pub fn folds(mut self, folds: &'a [Fold]) -> Self {
        self.folds = folds;
        self
    }

    /// Soft-wrap long lines instead of letting them overflow.
    #[must_use]
    pub fn word_wrap(mut self, word_wrap: bool) -> Self {
        self.word_wrap = word_wrap;
        self
    }

    /// Pin the enclosing scope headers above the viewport while scrolling.
    #[must_use]
    pub fn sticky_scroll(mut self, sticky_scroll: bool) -> Self {
        self.sticky_scroll = sticky_scroll;
        self
    }

    #[cfg(any(feature = "images", feature = "pdf"))]
    /// Reserve `area` for a Kitty placement: paint the themed background so the
    /// cells underneath are clean, and record the rect for the post-draw flush.
    fn reserve_kitty(&self, area: Rect, buf: &mut Buffer, state: &mut FileViewState) {
        buf.set_style(
            area,
            Style::default().bg(self.theme.role(ThemeRole::Background).to_ratatui()),
        );
        state.pending_image = Some(area);
    }
}

impl StatefulWidget for FileView<'_> {
    type State = FileViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut FileViewState) {
        state.page = area.height;
        state.pending_image = None;

        match &self.doc.content {
            Content::Text {
                buffer,
                highlights,
                blocks,
                ..
            } => {
                Editor::new(buffer)
                    .theme(self.theme)
                    .highlights(highlights)
                    .semantic_blocks(blocks)
                    .decorations(self.decorations)
                    .folds(self.folds)
                    .word_wrap(self.word_wrap)
                    .sticky_scroll(self.sticky_scroll)
                    .read_only(true)
                    .render(area, buf, &mut state.editor);
            }
            #[cfg(feature = "images")]
            Content::Image(img) => match self.protocol {
                GraphicsProtocol::Kitty => {
                    self.reserve_kitty(
                        image::fit_rect(area, img.width(), img.height()),
                        buf,
                        state,
                    );
                }
                GraphicsProtocol::Halfblocks => ImageWidget::new(img).render(area, buf),
            },
            #[cfg(feature = "pdf")]
            Content::Document { doc, page_count } => {
                let idx = state.doc_page.min(page_count.saturating_sub(1));
                state.doc_page = idx;
                match self.protocol {
                    GraphicsProtocol::Kitty => {
                        // Rasterize the current page unless it is already cached.
                        if !matches!(&state.rendered, Some((i, _)) if *i == idx) {
                            state.rendered =
                                doc.render_page(idx, PDF_RENDER_SCALE).ok().map(|page| {
                                    let (w, h) = (page.width(), page.height());
                                    (idx, image::Image::from_rgba(page.into_rgba(), w, h))
                                });
                        }
                        match &state.rendered {
                            // Reserve an aspect-fit sub-rect so the page isn't stretched.
                            Some((_, img)) => {
                                let rect = image::fit_rect(area, img.width(), img.height());
                                self.reserve_kitty(rect, buf, state);
                            }
                            // Rasterization failed — fall back to a neutral placeholder.
                            None => Placeholder::new(&self.doc.path, self.doc.kind, None, 0)
                                .render(area, buf),
                        }
                    }
                    // No Kitty graphics: say what's missing rather than pretending
                    // the file can't be opened at all.
                    GraphicsProtocol::Halfblocks => {
                        Placeholder::requires_kitty(&self.doc.path).render(area, buf);
                    }
                }
            }
            Content::Binary(bytes) => {
                let rows = bytes.len().div_ceil(HEX_ROW_WIDTH);
                state.hex_scroll = state.hex_scroll.min(rows.saturating_sub(1));
                HexView::new(bytes)
                    .scroll(state.hex_scroll)
                    .theme(self.theme)
                    .render(area, buf);
            }
            Content::Placeholder => {
                Placeholder::new(&self.doc.path, self.doc.kind, self.doc.dims, self.doc.len)
                    .render(area, buf);
            }
        }
    }
}

/// Transmit the Kitty image reserved by the last [`FileView`] render to `out`.
///
/// Call once per frame, **after** `terminal.draw(...)`, and use the returned flag
/// to decide whether a [`clear_kitty_images`] is owed: `false` means the frame
/// reserved nothing, so an image transmitted earlier is still on screen and has
/// to be cleared. The half-block path and every non-image branch reserve nothing.
///
/// # Errors
/// Propagates any write/flush error from `out`.
#[cfg(feature = "raster")]
pub fn flush_kitty_image(
    doc: &FileDoc,
    state: &FileViewState,
    out: &mut impl Write,
) -> io::Result<bool> {
    // The pixels live either directly on the document (a raster image) or in the
    // per-view cache (a rasterized PDF page). The explicit type keeps a
    // `raster`-only build (neither arm compiled) inferrable.
    let img: Option<&image::Image> = match &doc.content {
        #[cfg(feature = "images")]
        Content::Image(img) => Some(img),
        #[cfg(feature = "pdf")]
        Content::Document { .. } => state.rendered.as_ref().map(|(_, img)| img),
        _ => None,
    };
    let (Some(rect), Some(img)) = (state.pending_image, img) else {
        return Ok(false);
    };
    write!(out, "{}", image::kitty_delete_all())?;
    // Position the cursor at the reserved rect's top-left (VT coords are 1-based).
    write!(out, "\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
    write!(out, "{}", img.kitty_escape(rect.width, rect.height))?;
    out.flush()?;
    Ok(true)
}

/// Clear every image this process transmitted through the Kitty protocol.
///
/// Called when the pane that held an image stops showing one — a new selection, a
/// reader opening or closing, teardown — so a transmitted placement can't survive
/// on top of later frames.
#[cfg(feature = "raster")]
pub fn clear_kitty_images(out: &mut impl Write) -> io::Result<()> {
    write!(out, "{}", image::kitty_delete_all())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(doc: &FileDoc, area: Rect, state: &mut FileViewState) -> Buffer {
        let mut buf = Buffer::empty(area);
        FileView::new(doc).render(area, &mut buf, state);
        buf
    }

    fn text_of(buf: &Buffer) -> String {
        buf.content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn prepares_text_with_highlights() {
        let doc = FileDoc::prepare(Path::new("a.rs"), b"fn main() {}\n", 13, &Limits::default());
        assert_eq!(doc.kind, FileKind::Text);
        assert_eq!(doc.language(), Some("Rust"));
        assert_eq!(doc.row_count(), 2);
        let Content::Text { highlights, .. } = &doc.content else {
            panic!("expected the text branch");
        };
        assert!(
            !highlights.all().is_empty(),
            "the bundled rust grammar should yield highlight spans"
        );
    }

    #[test]
    fn line_budget_disables_highlighting() {
        let src = "fn a() {}\n".repeat(10);
        let limits = Limits::new(karet_filetype::SIZE_GUARD, 3);
        let doc = FileDoc::prepare(Path::new("a.rs"), src.as_bytes(), src.len() as u64, &limits);
        let Content::Text { highlights, .. } = &doc.content else {
            panic!("expected the text branch");
        };
        assert!(
            highlights.all().is_empty(),
            "over-budget text must render unhighlighted"
        );
    }

    #[test]
    fn markdown_highlights_injected_fences() {
        let src = "# Title\n\n```rust\nfn main() {}\n```\n";
        let doc = FileDoc::prepare(
            Path::new("a.md"),
            src.as_bytes(),
            src.len() as u64,
            &Limits::default(),
        );
        assert_eq!(doc.kind, FileKind::Markdown);
        let Content::Text { highlights, .. } = &doc.content else {
            panic!("expected the text branch");
        };
        // The layered parse reaches into the fence: spans land past the fence open.
        let fence_body = src.find("fn main").expect("fence body");
        assert!(
            highlights
                .all()
                .iter()
                .any(|span| span.span.start.0 >= fence_body),
            "expected highlights inside the injected rust fence"
        );
    }

    #[test]
    fn binary_becomes_hex() {
        let doc = FileDoc::prepare(Path::new("x.bin"), &[0u8, 1, 2, 3], 4, &Limits::default());
        assert_eq!(doc.kind, FileKind::Binary);
        assert_eq!(doc.language(), None);
        assert_eq!(doc.row_count(), 1);
        let buf = render(&doc, Rect::new(0, 0, 80, 2), &mut FileViewState::new());
        assert!(text_of(&buf).contains("00000000"), "hex offset missing");
    }

    #[test]
    fn oversized_is_a_placeholder_without_reading_the_body() {
        let limits = Limits::new(1024, 20_000);
        // Only a head sample, with a large reported len — the body is never touched.
        let doc = FileDoc::prepare(Path::new("big.rs"), b"fn", 4096, &limits);
        assert_eq!(doc.kind, FileKind::TooLarge { len: 4096 });
        assert!(matches!(doc.content, Content::Placeholder));
    }

    #[test]
    fn unshipped_formats_fall_back_to_a_named_placeholder() {
        // We don't ship the DOCX engine; the placeholder still names the file.
        let doc = FileDoc::prepare(
            Path::new("report.docx"),
            b"PK\x03\x04",
            4,
            &Limits::default(),
        );
        assert_eq!(doc.kind, FileKind::Docx);
        assert!(matches!(doc.content, Content::Placeholder));
        let buf = render(&doc, Rect::new(0, 0, 40, 8), &mut FileViewState::new());
        assert!(text_of(&buf).contains("report.docx"));
    }

    #[test]
    fn text_branch_paints_a_gutter_and_no_caret() {
        let doc = FileDoc::prepare(Path::new("a.rs"), b"fn main() {}\n", 13, &Limits::default());
        let area = Rect::new(0, 0, 30, 4);
        let buf = render(&doc, area, &mut FileViewState::new());
        let row0: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row0.contains('1'), "gutter line number missing: {row0:?}");
        let any_caret = (0..area.width).any(|x| {
            (0..area.height).any(|y| {
                buf[(x, y)]
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
        });
        assert!(!any_caret, "the read-only branch must not draw a caret");
    }

    #[test]
    fn scroll_helpers_move_the_viewport() {
        let mut state = FileViewState::new();
        state.page = 10;
        state.page_down();
        state.scroll_down(3);
        assert_eq!(state.top(), 13);
        assert_eq!(state.hex_scroll, 13);
        state.scroll_to_line(45);
        assert_eq!(state.top(), 45);
        assert_eq!(state.hex_scroll, 45);
        state.scroll_to_top();
        assert_eq!(state.top(), 0);
        assert_eq!(state.hex_scroll, 0);
    }

    /// A minimal single-page PDF (an empty US-Letter page), inline so there is no
    /// fixture file to keep in sync.
    #[cfg(feature = "pdf")]
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
trailer<</Size 4/Root 1 0 R>>\n%%EOF";

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_prepares_with_a_page_count() {
        let doc = FileDoc::prepare(
            Path::new("a.pdf"),
            MINIMAL_PDF,
            MINIMAL_PDF.len() as u64,
            &Limits::default(),
        );
        assert_eq!(doc.kind, FileKind::Pdf);
        assert_eq!(doc.page_count(), Some(1));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_reserves_a_rect_and_flushes_a_kitty_escape() {
        let doc = FileDoc::prepare(
            Path::new("a.pdf"),
            MINIMAL_PDF,
            MINIMAL_PDF.len() as u64,
            &Limits::default(),
        );
        let area = Rect::new(0, 0, 40, 20);
        let mut state = FileViewState::new();
        let mut buf = Buffer::empty(area);
        FileView::new(&doc)
            .graphics(GraphicsProtocol::Kitty)
            .render(area, &mut buf, &mut state);
        assert!(state.pending_image.is_some(), "expected a reserved rect");
        let mut out = Vec::new();
        let drew = flush_kitty_image(&doc, &state, &mut out).expect("a Vec never fails");
        assert!(drew, "a reserved page should flush");
        assert!(
            String::from_utf8_lossy(&out).contains("\x1b_G"),
            "expected a Kitty graphics escape"
        );
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_without_kitty_asks_for_the_protocol() {
        let doc = FileDoc::prepare(
            Path::new("a.pdf"),
            MINIMAL_PDF,
            MINIMAL_PDF.len() as u64,
            &Limits::default(),
        );
        // The default protocol is half-blocks, which cannot show a page.
        let mut state = FileViewState::new();
        let buf = render(&doc, Rect::new(0, 0, 60, 8), &mut state);
        assert!(text_of(&buf).contains("Kitty graphics protocol"));
        assert!(state.pending_image.is_none(), "nothing should be reserved");
    }

    #[cfg(feature = "raster")]
    #[test]
    fn a_non_image_document_flushes_nothing() {
        let doc = FileDoc::prepare(Path::new("a.rs"), b"fn main() {}\n", 13, &Limits::default());
        let mut state = FileViewState::new();
        let _ = render(&doc, Rect::new(0, 0, 30, 4), &mut state);
        let mut out = Vec::new();
        let drew = flush_kitty_image(&doc, &state, &mut out).expect("a Vec never fails");
        assert!(!drew, "the text branch reserves nothing");
        assert!(out.is_empty(), "the text branch must not flush escapes");
    }
}
