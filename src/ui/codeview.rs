//! A scrollable, syntax-highlighted code view.
//!
//! Owns the highlighted document plus its scroll offsets and renders a
//! line-number gutter, horizontal scrolling, a scrollbar, and a focus-aware
//! border. Shared by the side-by-side preview pane and (later) the full-screen
//! file reader, so both scroll identically.

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

/// A scrollable view over a highlighted document.
#[derive(Default)]
pub struct CodeView {
    /// The full highlighted document (owned, already truncated by the loader).
    lines: Vec<Line<'static>>,
    /// First visible line (vertical scroll offset).
    top: usize,
    /// Horizontal scroll offset, in display cells.
    left: usize,
    /// Inner content size from the last render (cols, rows); drives clamping
    /// and paging. Zero until the first render.
    viewport: (u16, u16),
    /// Widest line in display cells, cached for the horizontal clamp.
    max_width: usize,
}

impl CodeView {
    /// Build a view over already-highlighted `lines`.
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        let max_width = lines.iter().map(Line::width).max().unwrap_or(0);
        Self {
            lines,
            max_width,
            ..Default::default()
        }
    }

    /// Scroll vertically by `delta` lines (negative = up), clamped.
    pub fn scroll_by(&mut self, delta: i32) {
        self.top = clamp_add(self.top, delta, self.max_top());
    }

    /// Scroll vertically by `dir` pages (one viewport height each), clamped.
    pub fn page(&mut self, dir: i32) {
        let step = self.page_rows() as i32 * dir;
        self.top = clamp_add(self.top, step, self.max_top());
    }

    /// Scroll horizontally by `delta` cells (negative = left), clamped.
    pub fn scroll_h(&mut self, delta: i32) {
        self.left = clamp_add(self.left, delta, self.max_left());
    }

    pub fn goto_top(&mut self) {
        self.top = 0;
    }

    pub fn goto_bottom(&mut self) {
        self.top = self.max_top();
    }

    /// The currently visible lines as plain text, for clipboard yank.
    pub fn visible_text(&self) -> String {
        self.lines
            .iter()
            .skip(self.top)
            .take(self.page_rows())
            .map(line_to_plain)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Rows of text the viewport can show.
    fn page_rows(&self) -> usize {
        (self.viewport.1 as usize).max(1)
    }

    /// Largest valid `top` so the final screenful sits at the bottom.
    fn max_top(&self) -> usize {
        self.lines.len().saturating_sub(self.page_rows())
    }

    /// Cells the line-number gutter occupies (digits + one trailing space).
    fn gutter_width(&self) -> usize {
        let digits = self.lines.len().max(1).to_string().len();
        digits.max(2) + 1
    }

    /// Cells available for text after the gutter.
    fn text_width(&self) -> usize {
        (self.viewport.0 as usize).saturating_sub(self.gutter_width())
    }

    fn max_left(&self) -> usize {
        self.max_width.saturating_sub(self.text_width())
    }

    /// Re-clamp the offsets after a viewport resize.
    fn clamp(&mut self) {
        self.top = self.top.min(self.max_top());
        self.left = self.left.min(self.max_left());
    }
}

/// Render `view` into `area` with a focus-aware bordered block titled `title`.
pub fn render(frame: &mut Frame, view: &mut CodeView, area: Rect, title: &str, focused: bool) {
    let border = if focused { theme::ACCENT } else { theme::MUTED };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    view.viewport = (inner.width, inner.height);
    view.clamp();

    let gutter_width = view.gutter_width();
    let rows = inner.height as usize;
    let mut out: Vec<Line> = Vec::with_capacity(rows);
    for (i, line) in view.lines.iter().enumerate().skip(view.top).take(rows) {
        let number = Span::styled(
            format!("{:>width$} ", i + 1, width = gutter_width - 1),
            Style::default().fg(theme::MUTED),
        );
        let mut spans = vec![number];
        spans.extend(shift_line(line, view.left));
        out.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(out), inner);

    // Scrollbar on the block's right border; only when the text overflows.
    if view.lines.len() > rows {
        let mut state = ScrollbarState::new(view.max_top().max(1)).position(view.top);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area.inner(Margin::new(0, 1)),
            &mut state,
        );
    }
}

/// Add a signed `delta` to `base`, clamped to `0..=max`.
fn clamp_add(base: usize, delta: i32, max: usize) -> usize {
    (base as i64 + delta as i64).clamp(0, max as i64) as usize
}

/// The plain-text content of a line (styling stripped).
fn line_to_plain(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Drop the first `left` display cells from a line's spans (horizontal scroll).
fn shift_line(line: &Line<'static>, left: usize) -> Vec<Span<'static>> {
    if left == 0 {
        return line.spans.clone();
    }
    let mut remaining = left;
    let mut out: Vec<Span<'static>> = Vec::new();
    for span in &line.spans {
        if out.is_empty() {
            let width = span.content.width();
            if remaining >= width {
                remaining -= width;
                continue; // span is entirely to the left of the viewport
            }
            if remaining > 0 {
                let content = drop_cells(span.content.as_ref(), remaining);
                out.push(Span::styled(content, span.style));
                remaining = 0;
                continue;
            }
        }
        out.push(span.clone());
    }
    out
}

/// Return the suffix of `s` after dropping its first `n` display cells. A wide
/// glyph straddling the boundary is dropped whole.
fn drop_cells(s: &str, n: usize) -> String {
    let mut acc = 0;
    for (i, ch) in s.char_indices() {
        if acc >= n {
            return s[i..].to_string();
        }
        acc += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(lines: usize) -> CodeView {
        let lines = (0..lines)
            .map(|i| Line::from(format!("line {i}")))
            .collect();
        let mut v = CodeView::new(lines);
        v.viewport = (20, 10); // 20 cols, 10 rows of text
        v
    }

    #[test]
    fn scroll_by_clamps_at_top() {
        let mut v = view(100);
        v.scroll_by(-5);
        assert_eq!(v.top, 0);
    }

    #[test]
    fn scroll_by_clamps_at_bottom() {
        let mut v = view(100);
        v.goto_bottom();
        assert_eq!(v.top, 90); // 100 lines - 10 rows
        v.scroll_by(50);
        assert_eq!(v.top, 90);
    }

    #[test]
    fn page_steps_by_viewport_height() {
        let mut v = view(100);
        v.page(1);
        assert_eq!(v.top, 10);
        v.page(-1);
        assert_eq!(v.top, 0);
    }

    #[test]
    fn scroll_h_clamps_to_widest_line() {
        let mut v = CodeView::new(vec![Line::from("x".repeat(100)), Line::from("y")]);
        v.viewport = (20, 10); // gutter 3 → text width 17
        v.scroll_h(1000);
        assert_eq!(v.left, 100 - 17);
        v.scroll_h(-1000);
        assert_eq!(v.left, 0);
    }

    #[test]
    fn visible_text_returns_the_window() {
        let mut v = view(100);
        v.viewport = (20, 3);
        v.scroll_by(5);
        assert_eq!(v.visible_text(), "line 5\nline 6\nline 7");
    }

    #[test]
    fn shift_line_drops_leading_cells() {
        let line = Line::from("hello world");
        let shifted = shift_line(&line, 6);
        let text: String = shifted.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "world");
    }

    #[test]
    fn shift_line_drops_across_spans() {
        let line = Line::from(vec![Span::raw("abc"), Span::raw("def")]);
        let text: String = shift_line(&line, 4)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(text, "ef");
    }
}
