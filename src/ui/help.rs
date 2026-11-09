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

/// The bindings, with one merged row for the line facets slotted in after `x`
/// (generated from `Facet::ALL`, so a new facet joins it for free). One row,
/// not one per facet, keeps the overlay short enough for a 30-row terminal.
fn bindings() -> Vec<(String, String)> {
    let mut rows = Vec::with_capacity(BINDINGS.len() + 1);
    for &(keys, description) in BINDINGS {
        rows.push((keys.to_string(), description.to_string()));
        if keys == FACET_ANCHOR {
            rows.push(facet_row());
        }
    }
    rows
}

/// `t / n / o / c / b` → `hide / show test · generated · … lines`. No
/// `(or click)`: it would widen the popup past a 100-column terminal, and the
/// header's facet chips already read as clickable.
fn facet_row() -> (String, String) {
    let keys: Vec<String> = Facet::ALL.iter().map(|f| f.key().to_string()).collect();
    let names: Vec<&str> = Facet::ALL.iter().map(|f| f.description()).collect();
    (
        keys.join(" / "),
        format!("hide / show {} lines", names.join(" · ")),
    )
}

pub fn render(frame: &mut Frame, area: Rect) {
    let bindings = bindings();
    // `{:<w$}` pads by chars, so measure keys in chars (not bytes: `↓`, `–`).
    let key_width = bindings
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines = Vec::with_capacity(bindings.len());
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

    let mut block = Block::bordered()
        .title(" keybindings ")
        .border_style(Style::default().fg(theme::ACCENT))
        .padding(Padding::horizontal(1));
    // A terminal too short for every row says how many fell off the bottom,
    // rather than dropping them silently.
    let hidden = lines
        .len()
        .saturating_sub(height.saturating_sub(2) as usize);
    if hidden > 0 {
        block = block.title_bottom(Line::from(format!(" {hidden} more ↓ ")).right_aligned());
    }

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every facet is named in one merged row, right after the anchor binding.
    #[test]
    fn facet_row_follows_the_anchor() {
        let rows = bindings();
        let anchor = rows
            .iter()
            .position(|(keys, _)| keys == FACET_ANCHOR)
            .expect("the anchor binding exists");
        let (keys, description) = &rows[anchor + 1];
        for facet in Facet::ALL {
            assert!(keys.contains(facet.key()), "{keys} lacks {}", facet.key());
            assert!(description.contains(facet.label()), "{description}");
        }
        // The bench chip says only `bench`; its help row names examples too.
        assert!(description.contains("example"), "{description}");
        assert_eq!(rows.len(), BINDINGS.len() + 1);
    }

    fn draw(width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, frame.area())).unwrap();
        format!("{}", terminal.backend())
    }

    /// A 120x30 terminal shows every binding, down to `/`, `?` and `q`, with
    /// no more-indicator.
    #[test]
    fn fits_a_30_row_terminal() {
        let view = draw(120, 30);
        for (keys, description) in &bindings() {
            assert!(view.contains(keys.as_str()), "{keys} missing:\n{view}");
            assert!(
                view.contains(description.as_str()),
                "{description} missing:\n{view}"
            );
        }
        for row in [
            "filter by name (Esc clears)",
            "toggle this help",
            "q / Ctrl-c",
        ] {
            assert!(view.contains(row), "{row} missing:\n{view}");
        }
        assert!(!view.contains("more ↓"), "{view}");
    }

    /// A terminal too short for every row says how many it cut off.
    #[test]
    fn too_short_a_terminal_counts_the_hidden_rows() {
        // 20 rows: a popup of 18, so 16 bindings show and the rest are counted.
        let hidden = bindings().len() - 16;
        let view = draw(120, 20);
        assert!(view.contains(&format!(" {hidden} more ↓ ")), "{view}");
    }

    /// On a terminal with room for it, every row renders whole: the popup is
    /// as wide as its widest description, so none is cut off mid-word — even
    /// on a 100-column terminal.
    #[test]
    fn every_row_renders_in_full() {
        let rows = bindings();
        let view = draw(100, rows.len() as u16 + 6);
        for (keys, description) in &rows {
            assert!(view.contains(keys.as_str()), "{keys} cut off:\n{view}");
            assert!(
                view.contains(description.as_str()),
                "{description} cut off:\n{view}"
            );
        }
    }
}
