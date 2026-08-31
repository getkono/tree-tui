//! The side-by-side preview pane.
//!
//! Shows the selected file through the [`FileView`] widget: syntax-highlighted
//! code (tree-sitter), an inline image (Kitty graphics, or truecolor half-blocks
//! where the terminal can't), a hex dump for binaries, or a placeholder for
//! oversized / undecodable files. Content is prepared once from a bounded prefix
//! and cached on the [`Loaded`] state, refreshed only when the selection changes
//! (see `Loaded::ensure_preview`).

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::fileview::{self, FileDoc, FileView, FileViewState, Limits, Stamp};
use super::theme;
use crate::app::{Focus, Loaded};

/// Largest file we read for a text preview.
const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// Above this many lines the text is rendered without highlighting so the pane
/// opens instantly (the buffer itself is unaffected).
const MAX_TEXT_LINES: usize = 500;

/// Size and highlight budgets for the inline preview: a small prefix and a low
/// line budget for an instant, glance-sized open.
#[must_use]
pub fn preview_limits() -> Limits {
    Limits::new(MAX_TEXT_BYTES, MAX_TEXT_LINES)
}

/// Cached preview content for the selected node.
#[derive(Default)]
pub struct Preview {
    /// The prepared document, or `None` for a directory / unreadable selection.
    pub doc: Option<FileDoc>,
    /// A short note shown when there is no document (directory, read error).
    pub note: Option<String>,
    /// The previewed text (text/markdown only), kept for the `y` yank.
    pub yank: Option<String>,
    /// Scroll state for the view; reset when the selection changes.
    pub state: FileViewState,
    /// What the file looked like when this was built, for the freshness check
    /// in `Loaded::mark_preview_stale_if_changed`.
    ///
    /// `None` only where the file could not be stat'd at all (and for a
    /// directory, whose note has no content to go stale). A *read* failure
    /// still records the stamp: left `None` it would compare unequal to disk
    /// forever, so every filesystem event under the root — a `cargo build`,
    /// say — would re-open and re-fail the same unreadable file.
    pub stamp: Option<Stamp>,
}

impl Preview {
    /// A note-only preview (directory, read error): nothing scrollable to show.
    pub fn note(message: String) -> Self {
        Self {
            note: Some(message),
            ..Default::default()
        }
    }

    /// A preview over an already-prepared document (used in tests).
    #[cfg(test)]
    pub fn from_doc(doc: FileDoc) -> Self {
        Self {
            doc: Some(doc),
            note: None,
            yank: None,
            state: FileViewState::new(),
            stamp: None,
        }
    }
}

/// Build the preview for `path` (a file), reading at most a bounded prefix.
pub fn load(path: &Path) -> Preview {
    // Stat first, read second, and keep it that way. A write landing between
    // the two stores the *older* stamp against the *newer* bytes, which costs
    // one redundant reload and converges. Stat'ing from the open handle instead
    // would store a stamp describing content that was never read — and that
    // change would then be invisible forever.
    let (len, stamp) = match std::fs::metadata(path) {
        Ok(meta) => (meta.len(), Some(Stamp::from_metadata(&meta))),
        // Unstat-able: no stamp to record, and `Stamp::of` will keep answering
        // `None` too, so this compares equal and does not re-read on every event.
        Err(err) => return Preview::note(format!("cannot read file: {err}")),
    };
    let limits = preview_limits();
    // Read a bounded prefix. For an oversized file this is just a head sample;
    // `prepare` classifies it `TooLarge` from `len` without reading the body.
    let bytes = match read_bounded(path, limits.max_bytes) {
        Ok(bytes) => bytes,
        // Stat'able but unreadable (mode 000, an I/O error). The stamp is real
        // and must be kept, or this file re-opens and re-fails on every event.
        Err(err) => {
            return Preview {
                stamp,
                ..Preview::note(format!("cannot read file: {err}"))
            };
        }
    };
    let doc = FileDoc::prepare(path, &bytes, len, &limits);
    // A text/markdown document exposes a language; keep its text for `y` yank.
    let yank = doc
        .language()
        .is_some()
        .then(|| String::from_utf8_lossy(&bytes).into_owned());
    Preview {
        doc: Some(doc),
        note: None,
        yank,
        state: FileViewState::new(),
        stamp,
    }
}

/// Read at most `max` bytes of `path`. Shared with the full-screen reader.
pub(super) fn read_bounded(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(max).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Render the preview pane for the current selection into `area`.
pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    let focused = loaded.focus == Focus::Preview;
    let border = if focused { theme::ACCENT } else { theme::MUTED };
    let block = Block::bordered()
        .title(" preview ")
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(doc) = &loaded.preview.doc {
        // Disjoint borrows of `doc` (shared) and `state` (mutable).
        frame.render_stateful_widget(
            FileView::new(doc).graphics(fileview::graphics_protocol()),
            inner,
            &mut loaded.preview.state,
        );
    } else if let Some(note) = &loaded.preview.note {
        let note = Paragraph::new(Line::from(Span::styled(
            note.clone(),
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(note, inner);
    }
}
