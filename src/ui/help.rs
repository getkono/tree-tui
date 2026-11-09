//! The keybinding help overlay.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::theme;
use crate::model::Facet;

/// The binding the facet rows follow (a test holds it to an existing key).
const FACET_ANCHOR: &str = "x";

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
    ("p / Tab", "toggle the preview pane"),
    ("w", "focus the tree / preview (scroll with j/k, h/l)"),
    ("drag", "resize the tree / preview split"),
    ("y", "copy preview text · selected path"),
    ("S", "release the mouse for native selection"),
    ("/", "filter by name (Esc clears)"),
    ("?", "toggle this help"),
    ("q / Ctrl-c", "quit"),
];

/// The bindings, with one row per line facet slotted in after `x` (generated
/// from `Facet::ALL`, so a new facet gets its row for free).
fn bindings() -> Vec<(String, String)> {
    let mut rows = Vec::with_capacity(BINDINGS.len() + Facet::ALL.len());
    for &(keys, description) in BINDINGS {
        rows.push((keys.to_string(), description.to_string()));
        if keys == FACET_ANCHOR {
            rows.extend(Facet::ALL.into_iter().map(|facet| {
                (
                    facet.key().to_string(),
                    format!("hide / show {} lines (or click)", facet.description()),
                )
            }));
        }
    }
    rows
}

pub fn render(frame: &mut Frame, area: Rect) {
    let bindings = bindings();
    // `{:<w$}` pads by chars, so measure keys in chars (not bytes: `↓`, `–`).
    let key_width = bindings
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = vec![Line::default()];
    for (keys, description) in &bindings {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{keys:<key_width$}"),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(description.as_str(), Style::default().fg(theme::MUTED)),
        ]));
    }

    // Size the popup to its widest row (plus border and padding), so no
    // description is cut off while the terminal has room for it.
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let width = (content_width + 4).min(area.width.saturating_sub(4) as usize) as u16;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every facet gets a row, right after the anchor binding.
    #[test]
    fn facet_rows_follow_the_anchor() {
        let rows = bindings();
        let anchor = rows
            .iter()
            .position(|(keys, _)| keys == FACET_ANCHOR)
            .expect("the anchor binding exists");
        let facets: Vec<Facet> = Facet::ALL.into_iter().collect();
        for (offset, facet) in facets.iter().enumerate() {
            let (keys, description) = &rows[anchor + 1 + offset];
            assert_eq!(keys, &facet.key().to_string());
            assert!(description.contains(facet.label()), "{description}");
        }
        // The bench chip says only `bench`; its help row names examples too.
        let bench = Facet::ALL
            .iter()
            .position(|&facet| facet == Facet::BenchExample)
            .expect("bench is a facet");
        assert!(rows[anchor + 1 + bench].1.contains("example"));
        assert_eq!(rows.len(), BINDINGS.len() + facets.len());
    }

    /// On a terminal with room for it, every row renders whole: the popup is
    /// as wide as its widest description, so none is cut off mid-word.
    #[test]
    fn every_row_renders_in_full() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let rows = bindings();
        let mut terminal = Terminal::new(TestBackend::new(120, rows.len() as u16 + 6)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();
        let view = format!("{}", terminal.backend());
        for (keys, description) in &rows {
            assert!(view.contains(keys.as_str()), "{keys} cut off:\n{view}");
            assert!(
                view.contains(description.as_str()),
                "{description} cut off:\n{view}"
            );
        }
    }
}
