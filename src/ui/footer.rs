//! The status / keybind footer, including the filter prompt and lens badge.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme;
use crate::model::{Lens, SortDir, SubKey};

#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    active_lens: Lens,
    sort_key: SubKey,
    sort_dir: SortDir,
    hide_zeros: bool,
    filter: &str,
    editing: bool,
    computing: Option<Lens>,
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
    let muted = |text: String| Span::styled(text, Style::default().fg(theme::MUTED));
    let sort_text = format!("sort: {} {}", sort_key.label(), sort_dir.arrow());

    let line = if editing {
        Line::from(vec![
            badge(" filter ".to_string()),
            Span::raw(format!(" /{filter}")),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
            muted("   Enter apply · Esc cancel".to_string()),
        ])
    } else if !filter.is_empty() {
        Line::from(vec![
            badge(format!(" filter: {filter} ")),
            muted("   Esc clear · ".to_string()),
            muted(format!("{} · {sort_text}", active_lens.label())),
        ])
    } else {
        let mut spans = vec![
            badge(format!(" {} ", active_lens.label())),
            Span::raw(" "),
            muted(sort_text),
        ];
        if hide_zeros {
            spans.push(muted("  · nonzero".to_string()));
        }
        match computing {
            Some(lens) => spans.push(Span::styled(
                format!("   computing {}…", lens.label()),
                Style::default().fg(theme::WARN),
            )),
            None => spans.push(muted(
                "   m lens · s sort · r reverse · z declutter · / filter · ? help · q quit"
                    .to_string(),
            )),
        }
        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(line), area);
}
