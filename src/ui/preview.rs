//! The side-by-side preview pane.
//!
//! Shows the selected file: syntax-highlighted text (bat-like) or an inline
//! image where the terminal supports a graphics protocol. Content is read in a
//! bounded prefix and cached on the [`Loaded`] state, refreshed only when the
//! selection changes (see `Loaded::ensure_preview`).

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use super::{codeview, syntax, theme};
use crate::app::{Focus, Loaded};

/// Largest file we read for a text preview.
const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// Largest number of lines we highlight.
const MAX_TEXT_LINES: usize = 500;
/// Largest image file we decode.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Cached preview content for the selected node.
#[derive(Default)]
pub enum Preview {
    /// Nothing selected.
    #[default]
    Empty,
    /// Syntax-highlighted text (already truncated).
    Text(Vec<Line<'static>>),
    /// A decoded image, resized to the pane on render.
    Image(Box<StatefulProtocol>),
    /// A short note: directory, binary, too large, unreadable, …
    Info(String),
}

/// Build the preview for `path` (a file), reading at most a bounded prefix.
pub fn load(path: &Path, picker: Option<&Picker>) -> Preview {
    if image::ImageFormat::from_path(path).is_ok() {
        load_image(path, picker)
    } else {
        load_text(path)
    }
}

fn load_image(path: &Path, picker: Option<&Picker>) -> Preview {
    let Some(picker) = picker else {
        return Preview::Info("image preview is not supported by this terminal".into());
    };
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_IMAGE_BYTES => {
            return Preview::Info(format!(
                "image too large to preview ({})",
                theme::human_bytes(meta.len())
            ));
        }
        Ok(_) => {}
        Err(err) => return Preview::Info(format!("cannot read image: {err}")),
    }
    let reader = match image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
        Ok(reader) => reader,
        Err(err) => return Preview::Info(format!("cannot read image: {err}")),
    };
    match reader.decode() {
        Ok(img) => Preview::Image(Box::new(picker.new_resize_protocol(img))),
        Err(err) => Preview::Info(format!("cannot decode image: {err}")),
    }
}

fn load_text(path: &Path) -> Preview {
    use std::io::Read;

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => return Preview::Info(format!("cannot read file: {err}")),
    };
    let mut buf = Vec::new();
    if let Err(err) = file.take(MAX_TEXT_BYTES).read_to_end(&mut buf) {
        return Preview::Info(format!("cannot read file: {err}"));
    }
    // A NUL byte in the prefix is a reliable "this is binary" signal.
    if buf.contains(&0) {
        return Preview::Info("binary file — no preview".into());
    }
    let text = String::from_utf8_lossy(&buf);
    let mut truncated = String::new();
    for line in text.lines().take(MAX_TEXT_LINES) {
        truncated.push_str(line);
        truncated.push('\n');
    }
    if truncated.trim().is_empty() {
        return Preview::Info("empty file".into());
    }
    Preview::Text(syntax::highlight(&truncated, path))
}

/// Render the preview pane for the current selection into `area`.
pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    let focused = loaded.focus == Focus::Preview;

    // Text scrolls through the shared code-view, which owns its own border.
    if matches!(loaded.preview, Preview::Text(_)) {
        codeview::render(frame, &mut loaded.preview_view, area, " preview ", focused);
        return;
    }

    let border = if focused { theme::ACCENT } else { theme::MUTED };
    let block = Block::bordered()
        .title(" preview ")
        .border_style(Style::default().fg(border))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &mut loaded.preview {
        Preview::Empty | Preview::Text(_) => {}
        Preview::Image(protocol) => {
            frame.render_stateful_widget(
                StatefulImage::<StatefulProtocol>::new(),
                inner,
                protocol.as_mut(),
            );
        }
        Preview::Info(message) => {
            let note = Paragraph::new(Line::from(Span::styled(
                message.clone(),
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::ITALIC),
            )))
            .alignment(Alignment::Center);
            frame.render_widget(note, inner);
        }
    }
}
