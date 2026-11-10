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

/// The narrowest terminal the overlay must fit without cutting a row off.
const FIT_COLUMNS: usize = 80;

/// The bindings, with the merged line-facet rows slotted in after `x`
/// (generated from `Facet::ALL`, so a new facet joins them for free). Merging
/// facets rather than giving each its own row keeps the overlay short enough
/// for a 30-row terminal.
fn bindings() -> Vec<(String, String)> {
    let mut rows = Vec::with_capacity(BINDINGS.len() + Facet::ALL.len());
    for &(keys, description) in BINDINGS {
        rows.push((keys.to_string(), description.to_string()));
        if keys == FACET_ANCHOR {
            rows.extend(facet_rows());
        }
    }
    rows
}

/// `t / n / o` → `hide / show test · generated · bench / example lines`, and
/// so on: the facets packed in order, as many to a row as fit the description
/// column an `FIT_COLUMNS`-wide terminal leaves. No `(or click)`: it would
/// cost a row, and the header's facet chips already read as clickable.
fn facet_rows() -> Vec<(String, String)> {
    let row = |facets: &[Facet]| {
        let keys: Vec<String> = facets.iter().map(|f| f.key().to_string()).collect();
        let names: Vec<&str> = facets.iter().map(|f| f.description()).collect();
        (
            keys.join(" / "),
            format!("hide / show {} lines", names.join(" · ")),
        )
    };
    // The popup keeps 2 columns of margin a side, then border and padding take
    // 2 more a side; each row is `  KEYS   DESCRIPTION`, KEYS padded to the
    // widest static key (a packed row's keys are shorter, see the tests).
    let key_width = BINDINGS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let room = FIT_COLUMNS - 8 - 2 - key_width - 3;

    let mut rows = Vec::new();
    let mut start = 0;
    for end in 1..=Facet::ALL.len() {
        let wider = row(&Facet::ALL[start..end]).1.chars().count() > room;
        if wider && end - 1 > start {
            rows.push(row(&Facet::ALL[start..end - 1]));
            start = end - 1;
        }
    }
    rows.push(row(&Facet::ALL[start..]));
    rows
}

/// The overlay's scroll position. A terminal too short for every binding row
/// scrolls them (j/k, arrows, PgUp/PgDn, g/G, the wheel) instead of cutting
/// the bottom off for good.
///
/// The reducer moves `offset`; [`render`] records how many rows fit and clamps
/// `offset` to them, so a resize that makes room never strands the view past
/// the last row.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HelpScroll {
    /// Index of the first binding row on screen.
    offset: usize,
    /// Binding rows the last frame had room for (0 before the first frame).
    page: usize,
}

impl HelpScroll {
    /// Back to the top, as the overlay is when it opens.
    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Scroll by `delta` rows (positive is down), stopping at either end.
    pub fn scroll_by(&mut self, delta: isize) {
        self.offset = self.offset.saturating_add_signed(delta).min(self.max());
    }

    /// Scroll by `pages` screenfuls (positive is down).
    pub fn page_by(&mut self, pages: isize) {
        self.scroll_by(pages.saturating_mul(self.page.max(1) as isize));
    }

    /// Jump to the last screenful.
    pub fn jump_to_bottom(&mut self) {
        self.offset = self.max();
    }

    /// The furthest `offset` can go with the last frame's page: the one that
    /// puts the last row at the bottom.
    fn max(&self) -> usize {
        row_count().saturating_sub(self.page)
    }
}

/// How many rows the overlay lists.
fn row_count() -> usize {
    bindings().len()
}

pub fn render(frame: &mut Frame, area: Rect, scroll: &mut HelpScroll) {
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
    // A terminal too short for every row scrolls them, and says how many are
    // out of view on each side rather than dropping them silently.
    scroll.page = height.saturating_sub(2) as usize;
    scroll.offset = scroll.offset.min(scroll.max());
    let above = scroll.offset;
    let below = lines.len().saturating_sub(above + scroll.page);
    if above > 0 {
        block = block.title_top(Line::from(format!(" {above} more ↑ ")).right_aligned());
    }
    if below > 0 {
        block = block.title_bottom(Line::from(format!(" {below} more ↓ ")).right_aligned());
    }

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((u16::try_from(above).unwrap_or(u16::MAX), 0)),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every facet is named, in order, in the merged rows right after the
    /// anchor binding, and merging saves rows over one row per facet.
    #[test]
    fn facet_rows_follow_the_anchor() {
        let rows = bindings();
        let anchor = rows
            .iter()
            .position(|(keys, _)| keys == FACET_ANCHOR)
            .expect("the anchor binding exists");
        let facet_rows = facet_rows();
        assert_eq!(&rows[anchor + 1..anchor + 1 + facet_rows.len()], facet_rows);
        assert!(facet_rows.len() < Facet::ALL.len(), "{facet_rows:?}");
        let keys: Vec<&str> = facet_rows.iter().map(|(k, _)| k.as_str()).collect();
        let descriptions: Vec<&str> = facet_rows.iter().map(|(_, d)| d.as_str()).collect();
        let (keys, descriptions) = (keys.join(" / "), descriptions.join(" "));
        let expected: Vec<String> = Facet::ALL.iter().map(|f| f.key().to_string()).collect();
        assert_eq!(keys, expected.join(" / "));
        for facet in Facet::ALL {
            assert!(descriptions.contains(facet.label()), "{descriptions}");
        }
        // The bench chip says only `bench`; its help row names examples too.
        assert!(descriptions.contains("example"), "{descriptions}");
        // A packed row's keys never widen the key column past the static keys.
        let static_width = BINDINGS.iter().map(|(k, _)| k.chars().count()).max();
        for (keys, _) in &facet_rows {
            assert!(Some(keys.chars().count()) <= static_width, "{keys}");
        }
    }

    /// An 80x30 terminal shows every binding whole, facet rows included, with
    /// no more-indicator.
    #[test]
    fn fits_an_80_column_terminal() {
        let view = draw(80, 30);
        for (keys, description) in &bindings() {
            assert!(view.contains(keys.as_str()), "{keys} cut off:\n{view}");
            assert!(
                view.contains(description.as_str()),
                "{description} cut off:\n{view}"
            );
        }
        assert!(view.contains("comment · blank lines"), "{view}");
        assert!(!view.contains("more ↓"), "{view}");
    }

    fn draw(width: u16, height: u16) -> String {
        draw_scrolled(width, height, &mut HelpScroll::default())
    }

    fn draw_scrolled(width: u16, height: u16, scroll: &mut HelpScroll) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), scroll))
            .unwrap();
        format!("{}", terminal.backend())
    }

    /// On a terminal too short for every row, scrolling reaches each of them,
    /// and the indicators count what is out of view above and below.
    #[test]
    fn a_short_terminal_scrolls_to_every_row() {
        let rows = bindings();
        // 20 rows: a popup of 18, so 16 bindings show at a time.
        let mut scroll = HelpScroll::default();
        let top = draw_scrolled(120, 20, &mut scroll);
        assert!(!top.contains("more ↑"), "{top}");
        assert_eq!(scroll.page, 16);

        scroll.scroll_by(1);
        let one = draw_scrolled(120, 20, &mut scroll);
        assert!(one.contains(" 1 more ↑ "), "{one}");
        let below = rows.len() - 16 - 1;
        assert!(one.contains(&format!(" {below} more ↓ ")), "{one}");
        assert!(!one.contains(rows[0].1.as_str()), "{one}");
        assert!(one.contains(rows[16].1.as_str()), "{one}");

        scroll.jump_to_bottom();
        let bottom = draw_scrolled(120, 20, &mut scroll);
        let above = rows.len() - 16;
        assert!(bottom.contains(&format!(" {above} more ↑ ")), "{bottom}");
        assert!(!bottom.contains("more ↓"), "{bottom}");
        for row in [
            "filter by name (Esc clears)",
            "toggle this help",
            "q / Ctrl-c",
        ] {
            assert!(bottom.contains(row), "{row} unreachable:\n{bottom}");
        }

        // Scrolling stops at the last screenful and at the top.
        scroll.scroll_by(5);
        assert_eq!(scroll.offset, above);
        scroll.page_by(-10);
        assert_eq!(scroll.offset, 0);
        scroll.page_by(1);
        assert_eq!(scroll.offset, 16.min(above));
    }

    /// A resize that makes room for every row pulls a scrolled view back to
    /// the top, rather than leaving rows above it out of view for nothing.
    #[test]
    fn growing_the_terminal_unscrolls_the_view() {
        let mut scroll = HelpScroll::default();
        draw_scrolled(120, 20, &mut scroll);
        scroll.jump_to_bottom();
        assert!(scroll.offset > 0);
        let view = draw_scrolled(120, 40, &mut scroll);
        assert_eq!(scroll.offset, 0);
        assert!(!view.contains("more ↑"), "{view}");
        assert!(!view.contains("more ↓"), "{view}");
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
    /// as wide as its widest description, so none is cut off mid-word.
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
