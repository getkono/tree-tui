//! The keybinding help overlay.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::theme;

const BINDINGS: &[(&str, &str)] = &[
    ("j / k  ↓ / ↑", "move selection"),
    ("g / G", "jump to top / bottom"),
    ("Ctrl-d / Ctrl-u", "page down / up"),
    ("l / → / Enter", "expand, or descend into a directory"),
    ("h / ←", "collapse, or jump to the parent"),
    ("Space", "toggle the selected directory"),
    ("E / C", "expand all / collapse all"),
    ("s", "cycle the sort column"),
    ("r", "reverse the sort order"),
    ("d / Tab", "toggle the detail panel"),
    ("/", "filter by name (Esc clears)"),
    ("?", "toggle this help"),
    ("q / Ctrl-c", "quit"),
];

pub fn render(frame: &mut Frame, area: Rect) {
    let key_width = BINDINGS.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut lines = vec![Line::default()];
    for (keys, description) in BINDINGS {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{keys:<key_width$}"),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(*description, Style::default().fg(theme::MUTED)),
        ]));
    }

    let width = (key_width + 40).min(area.width.saturating_sub(4) as usize) as u16;
    let height = (lines.len() + 2).min(area.height.saturating_sub(2) as usize) as u16;
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);

    let block = Block::bordered()
        .title(" keybindings ")
        .border_style(Style::default().fg(theme::ACCENT))
        .padding(Padding::horizontal(1));

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}
