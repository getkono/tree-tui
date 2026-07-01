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
    ("l / →", "expand, or descend into a directory"),
    ("Enter", "open a file in the reader · expand a directory"),
    ("Shift-Enter / e", "edit a file in $EDITOR"),
    ("h / ←", "collapse, or jump to the parent"),
    ("Space", "toggle the selected directory"),
    ("E / C", "expand all / collapse all"),
    ("m", "cycle the active lens"),
    ("1 – 4", "jump to a lens (code/size/churn/status)"),
    ("s", "cycle the sort column (within the lens)"),
    ("r", "reverse the sort order"),
    ("z", "hide rows that are zero under the lens"),
    ("x", "exclude / include the selected node"),
    ("d / Tab", "toggle the detail panel"),
    ("p", "toggle the preview pane"),
    ("w", "focus the tree / preview (scroll with j/k, h/l)"),
    ("y", "copy preview text · selected path"),
    ("S", "release the mouse for native selection"),
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
