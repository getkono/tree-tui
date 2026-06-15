//! The centered loading screen with an animated spinner.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::theme;
use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let spinner = theme::SPINNER[app.spinner % theme::SPINNER.len()];
    let text = vec![
        Line::from(vec![
            Span::styled(
                spinner,
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  scanning {}", app.root_label)),
        ]),
        Line::from(Span::styled(
            format!("{:.1}s elapsed", app.elapsed.as_secs_f64()),
            Style::default().fg(theme::MUTED),
        )),
    ];

    let [row] = Layout::vertical([Constraint::Length(4)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(40)])
        .flex(Flex::Center)
        .areas(row);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(Block::bordered().border_style(Style::default().fg(theme::MUTED))),
        popup,
    );
}
