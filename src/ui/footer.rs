//! The status / keybind footer, including the filter prompt.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme;
use crate::model::{SortDir, SortKey};

pub fn render(
    frame: &mut Frame,
    sort_key: SortKey,
    sort_dir: SortDir,
    filter: &str,
    editing: bool,
    area: Rect,
) {
    let badge = |text: String| {
        Span::styled(
            text,
            Style::default()
                .fg(theme::BADGE_FG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let muted = |text: &'static str| Span::styled(text, Style::default().fg(theme::MUTED));

    let line = if editing {
        Line::from(vec![
            badge(" filter ".to_string()),
            Span::raw(format!(" /{filter}")),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
            muted("   Enter apply · Esc cancel"),
        ])
    } else if !filter.is_empty() {
        Line::from(vec![
            badge(format!(" filter: {filter} ")),
            muted("   Esc clear · "),
            Span::styled(
                format!("sort: {} {}", sort_key.label(), sort_dir.arrow()),
                Style::default().fg(theme::MUTED),
            ),
        ])
    } else {
        Line::from(vec![
            badge(format!(" sort: {} {} ", sort_key.label(), sort_dir.arrow())),
            muted(" j/k move · l/h expand · s sort · r reverse · / filter · ? help · q quit"),
        ])
    };

    frame.render_widget(Paragraph::new(line), area);
}
