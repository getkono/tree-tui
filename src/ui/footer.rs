//! The status/keybind footer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme;
use crate::model::{SortDir, SortKey};

pub fn render(frame: &mut Frame, sort_key: SortKey, sort_dir: SortDir, area: Rect) {
    let badge = format!(" sort: {} {} ", sort_key.label(), sort_dir.arrow());
    let hints = " j/k move · l/h expand · space toggle · E/C all · s sort · r reverse · q quit";
    let line = Line::from(vec![
        Span::styled(
            badge,
            Style::default()
                .fg(theme::BADGE_FG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(hints, Style::default().fg(theme::MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
