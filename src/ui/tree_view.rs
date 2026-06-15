//! The scrollable tree table: one row per visible node.
//!
//! Columns drop progressively (blanks → comments → code) as the available width
//! shrinks, so the name and total-lines columns always stay readable. The
//! `languages` column flexes too: it lists every language with percentages when
//! wide, collapses tail languages into `Other`, and finally falls back to an
//! `N languages` count — important when the detail panel is open.

use std::cmp::Reverse;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};
use tokei::LanguageType;
use unicode_width::UnicodeWidthStr;

use super::theme;
use crate::app::Loaded;
use crate::model::{NodeKind, TreeNode};

/// Smallest worthwhile languages column (also the width of the `languages`
/// header); below this it is dropped entirely.
const LANG_MIN: usize = 9;
/// Upper bound on the languages column so it never dominates a wide terminal.
const LANG_MAX: usize = 64;
/// Floor and ceiling for the name reservation, sized around the longest name.
const NAME_FLOOR: usize = 6;
const NAME_MAX: usize = 44;

/// Which optional columns are shown, chosen from the available width.
struct Columns {
    code: bool,
    comments: bool,
    blanks: bool,
    /// Resolved width of the languages column (`0` hides it entirely).
    lang_width: usize,
}

impl Columns {
    /// Choose columns for inner width `width`. The languages column takes the
    /// width the widest visible legend needs (`desired`), after reserving room
    /// for the longest visible name (`name_needed`) so names aren't truncated.
    fn new(width: usize, name_needed: usize, desired: usize) -> Self {
        let code = width >= 30;
        let comments = width >= 56;
        let blanks = width >= 68;

        let fixed = 8 // total lines (always)
            + usize::from(code) * 8
            + usize::from(comments) * 9
            + usize::from(blanks) * 8;
        // Columns sharing one cell of spacing each: name, lines, the optional
        // numeric columns, and (tentatively) the languages column.
        let spacing = 2 + usize::from(code) + usize::from(comments) + usize::from(blanks);

        let budget = width.saturating_sub(fixed + spacing);
        let name_reserve = name_needed.clamp(NAME_FLOOR, NAME_MAX).min(budget);
        let avail = budget.saturating_sub(name_reserve).min(LANG_MAX);
        // Size the column to the widest legend, but never below the `languages`
        // header (`LANG_MIN`) — single-language trees want a narrow legend that
        // would otherwise be dropped. Hide it only when even the header won't fit.
        let lang_width = if desired == 0 || avail < LANG_MIN {
            0
        } else {
            desired.max(LANG_MIN).min(avail)
        };

        Self {
            code,
            comments,
            blanks,
            lang_width,
        }
    }

    fn widths(&self) -> Vec<Constraint> {
        let mut widths = vec![Constraint::Fill(1)];
        if self.lang_width > 0 {
            widths.push(Constraint::Length(self.lang_width as u16));
        }
        if self.code {
            widths.push(Constraint::Length(8));
        }
        if self.comments {
            widths.push(Constraint::Length(9));
        }
        if self.blanks {
            widths.push(Constraint::Length(8));
        }
        widths.push(Constraint::Length(8)); // total lines (always)
        widths
    }

    fn header(&self) -> Row<'static> {
        let right = |text: &'static str| Cell::from(Line::from(text).alignment(Alignment::Right));
        let mut cells = vec![Cell::from("name")];
        if self.lang_width > 0 {
            cells.push(Cell::from("languages"));
        }
        if self.code {
            cells.push(right("code"));
        }
        if self.comments {
            cells.push(right("comments"));
        }
        if self.blanks {
            cells.push(right("blanks"));
        }
        cells.push(right("lines"));
        Row::new(cells).style(
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn row(&self, node: &TreeNode) -> Row<'static> {
        let mut cells = vec![Cell::from(Line::from(name_spans(node)))];
        if self.lang_width > 0 {
            cells.push(Cell::from(Line::from(language_legend(
                node,
                self.lang_width,
            ))));
        }
        if self.code {
            cells.push(num_cell(node.stats.code, theme::CODE));
        }
        if self.comments {
            cells.push(num_cell(node.stats.comments, theme::COMMENTS));
        }
        if self.blanks {
            cells.push(num_cell(node.stats.blanks, theme::BLANKS));
        }
        cells.push(Cell::from(
            Line::from(Span::styled(
                theme::group_thousands(node.stats.lines()),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
        ));
        Row::new(cells)
    }
}

pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    // Visible tree rows = body height minus borders (2) and the header row (1).
    loaded.viewport_rows = area.height.saturating_sub(3) as usize;

    // Inner width available to columns: minus borders (2) and the 2-cell
    // selection gutter.
    let inner = area.width.saturating_sub(4) as usize;
    let mut name_needed = 0;
    let mut desired = 0;
    for &id in &loaded.visible {
        let node = &loaded.tree.nodes[id];
        name_needed = name_needed.max(spans_width(&name_spans(node)));
        desired = desired.max(desired_legend_width(node));
    }
    let columns = Columns::new(inner, name_needed, desired);

    let rows: Vec<Row> = loaded
        .visible
        .iter()
        .map(|&id| columns.row(&loaded.tree.nodes[id]))
        .collect();

    let table = Table::new(rows, columns.widths())
        .header(columns.header())
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(theme::SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ")
        .block(
            Block::bordered()
                .border_style(Style::default().fg(theme::MUTED))
                .title(" tree "),
        );

    frame.render_stateful_widget(table, area, &mut loaded.table_state);
}

fn name_spans(node: &TreeNode) -> Vec<Span<'static>> {
    let indent = node.depth.saturating_sub(1);
    let mut spans = vec![Span::raw("  ".repeat(indent))];
    match node.kind {
        NodeKind::Dir => {
            let glyph = if node.expanded {
                theme::GLYPH_EXPANDED
            } else {
                theme::GLYPH_COLLAPSED
            };
            spans.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(theme::DIR),
            ));
            spans.push(Span::styled(
                format!("{}/", node.name),
                Style::default().fg(theme::DIR).add_modifier(Modifier::BOLD),
            ));
        }
        NodeKind::File => {
            let color = node
                .primary_lang
                .map_or(theme::MUTED, theme::language_color);
            spans.push(Span::styled(
                format!("{} ", theme::GLYPH_FILE),
                Style::default().fg(color),
            ));
            spans.push(Span::raw(node.name.clone()));
        }
    }
    spans
}

/// A node's languages, largest first, dropping any with no lines.
fn sorted_langs(node: &TreeNode) -> Vec<(LanguageType, usize)> {
    let mut langs: Vec<(LanguageType, usize)> = node
        .langs
        .iter()
        .map(|(lang, stats)| (*lang, stats.lines()))
        .filter(|(_, lines)| *lines > 0)
        .collect();
    langs.sort_by_key(|(_, lines)| Reverse(*lines));
    langs
}

/// Total display width of a sequence of spans.
fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// `Lang (12.3%)` entries (plus an optional `Other` bucket) joined by `, `.
fn legend_spans(shown: &[(LanguageType, usize)], other: usize, total: usize) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let entry = |label: String, color: Color, lines: usize, spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            spans.push(Span::styled(", ", Style::default().fg(theme::MUTED)));
        }
        spans.push(Span::styled(
            format!("{label} "),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            format!("({})", theme::percent(lines, total)),
            Style::default().fg(theme::MUTED),
        ));
    };
    for (lang, lines) in shown {
        entry(
            theme::language_label(*lang),
            theme::language_color(*lang),
            *lines,
            &mut spans,
        );
    }
    if other > 0 {
        entry("Other".to_string(), theme::MUTED, other, &mut spans);
    }
    spans
}

/// Width the full (untruncated) legend would occupy for `node`.
fn desired_legend_width(node: &TreeNode) -> usize {
    let langs = sorted_langs(node);
    match langs.as_slice() {
        [] => 0,
        [(lang, _)] => theme::language_label(*lang).width(),
        _ => spans_width(&legend_spans(&langs, 0, node.stats.lines())),
    }
}

/// The languages cell: the widest representation that fits in `width`.
///
/// Falls back from the full percentage list, to leading languages plus an
/// `Other` bucket, to a bare `N languages` count.
fn language_legend(node: &TreeNode, width: usize) -> Vec<Span<'static>> {
    let langs = sorted_langs(node);
    let total = node.stats.lines();
    match langs.as_slice() {
        [] => Vec::new(),
        [(lang, _)] => vec![Span::styled(
            theme::language_label(*lang),
            Style::default().fg(theme::language_color(*lang)),
        )],
        _ => {
            let full = legend_spans(&langs, 0, total);
            if spans_width(&full) <= width {
                return full;
            }
            // Drop tail languages into an `Other` bucket until it fits.
            for keep in (1..langs.len()).rev() {
                let other: usize = langs[keep..].iter().map(|(_, lines)| lines).sum();
                let spans = legend_spans(&langs[..keep], other, total);
                if spans_width(&spans) <= width {
                    return spans;
                }
            }
            // Degrade the count gracefully rather than truncate mid-word.
            let n = langs.len();
            let label = [
                format!("{n} languages"),
                format!("{n} langs"),
                n.to_string(),
            ]
            .into_iter()
            .find(|s| s.width() <= width)
            .unwrap_or_else(|| format!("{n} languages"));
            vec![Span::styled(label, Style::default().fg(theme::MUTED))]
        }
    }
}

fn num_cell(value: usize, color: Color) -> Cell<'static> {
    Cell::from(
        Line::from(Span::styled(
            theme::group_thousands(value),
            Style::default().fg(color),
        ))
        .alignment(Alignment::Right),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stats;
    use tokei::LanguageType as L;

    fn node(langs: &[(L, usize)]) -> TreeNode {
        let mut node = TreeNode::dir("src".into(), "src".into(), None, 1);
        let mut total = 0;
        for (lang, lines) in langs {
            node.langs.insert(
                *lang,
                Stats {
                    code: *lines,
                    ..Stats::default()
                },
            );
            total += *lines;
        }
        node.stats = Stats {
            code: total,
            ..Stats::default()
        };
        node
    }

    fn text(node: &TreeNode, width: usize) -> String {
        language_legend(node, width)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    // src/: Rust 75.0%, Python 17.4%, TOML 4.4%, Markdown 3.2%.
    fn polyglot() -> TreeNode {
        node(&[
            (L::Rust, 775),
            (L::Python, 180),
            (L::Toml, 45),
            (L::Markdown, 33),
        ])
    }

    #[test]
    fn legend_lists_every_language_when_wide() {
        let s = text(&polyglot(), 200);
        assert_eq!(
            s,
            "Rust (75.0%), Python (17.4%), TOML (4.4%), Markdown (3.2%)"
        );
        assert_eq!(desired_legend_width(&polyglot()), s.width());
    }

    #[test]
    fn legend_collapses_tail_into_other() {
        // Fits "Rust (75.0%), Other (25.0%)" (27) but not the next tier (42).
        assert_eq!(text(&polyglot(), 27), "Rust (75.0%), Other (25.0%)");
    }

    #[test]
    fn legend_falls_back_to_a_count_then_degrades() {
        assert_eq!(text(&polyglot(), 15), "4 languages");
        assert_eq!(text(&polyglot(), 7), "4 langs");
        assert_eq!(text(&polyglot(), 3), "4");
    }

    #[test]
    fn legend_single_language_is_just_the_label() {
        assert_eq!(text(&node(&[(L::Rust, 500)]), 40), "Rust");
    }

    #[test]
    fn legend_empty_node_is_blank() {
        assert!(language_legend(&node(&[]), 40).is_empty());
    }

    #[test]
    fn languages_column_shows_when_legends_are_narrow() {
        // A wide terminal whose widest legend is a short single label still
        // shows the column, floored to the header width. (The earlier bug
        // dropped it because `desired` (8) fell below LANG_MIN.)
        assert_eq!(Columns::new(96, 11, 8).lang_width, LANG_MIN);
    }

    #[test]
    fn languages_column_caps_at_available_space() {
        // A legend wider than the room left for it is truncated to fit.
        assert!(Columns::new(96, 11, 100).lang_width < 100);
        assert!(Columns::new(96, 11, 100).lang_width >= LANG_MIN);
    }

    #[test]
    fn languages_column_hidden_when_absent_or_cramped() {
        assert_eq!(Columns::new(96, 11, 0).lang_width, 0); // no languages
        assert_eq!(Columns::new(40, 30, 50).lang_width, 0); // detail panel squeeze
    }
}
