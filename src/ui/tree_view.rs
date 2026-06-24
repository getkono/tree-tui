//! The scrollable tree table: one row per visible node.
//!
//! The numeric columns and their values come from the active lens (and its cached
//! layer); the name column is always present. Columns drop progressively (right
//! to left) as width shrinks. For the code lens a `languages` column flexes the
//! same way it always has: full percentage list → collapsed `Other` → an
//! `N languages` count. While the active lens's layer is still computing, numeric
//! cells show a `…` placeholder.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};
use tokei::LanguageType;
use unicode_width::UnicodeWidthStr;

use super::theme;
use crate::app::{Focus, Loaded, RowCache};
use crate::model::{CodeNum, ColumnSpec, Lens, NodeId, NodeKind, SubKey, TreeNode};

/// Width of every numeric column (fits `comments`, `modified`, and grouped
/// counts or `123.4 MB`).
const COL_W: usize = 9;
/// Smallest worthwhile languages column (also the `languages` header width).
const LANG_MIN: usize = 9;
/// Upper bound on the languages column so it never dominates a wide terminal.
const LANG_MAX: usize = 64;
/// Floor and ceiling for the name reservation, sized around the longest name.
const NAME_FLOOR: usize = 6;
const NAME_MAX: usize = 44;

/// The resolved set of columns for the current width and lens.
struct Columns {
    /// Optional numeric columns shown (a width-trimmed prefix of the lens's set).
    cols: Vec<ColumnSpec>,
    /// The always-present primary column (rightmost, bold).
    primary: ColumnSpec,
    /// Resolved width of the languages column (`0` hides it; only ever set for
    /// the code lens).
    lang_width: usize,
}

impl Columns {
    fn choose(inner: usize, name_needed: usize, desired_legend: usize, lens: Lens) -> Self {
        let legend_on = matches!(lens, Lens::Code);
        let primary = lens.primary();
        let mut cols: Vec<ColumnSpec> = lens.columns().to_vec();

        // Drop optional columns (rightmost first) until a minimal name fits
        // alongside the primary + remaining columns.
        while !cols.is_empty() {
            if column_run(cols.len(), legend_on) + NAME_FLOOR <= inner {
                break;
            }
            cols.pop();
        }

        // Size the legend from whatever budget remains (code lens only).
        let budget = inner.saturating_sub(column_run(cols.len(), legend_on));
        let name_reserve = name_needed.clamp(NAME_FLOOR, NAME_MAX).min(budget);
        let avail = budget.saturating_sub(name_reserve).min(LANG_MAX);
        let lang_width = if legend_on && desired_legend > 0 && avail >= LANG_MIN {
            desired_legend.max(LANG_MIN).min(avail)
        } else {
            0
        };

        Self {
            cols,
            primary,
            lang_width,
        }
    }

    fn widths(&self) -> Vec<Constraint> {
        let mut widths = vec![Constraint::Fill(1)];
        if self.lang_width > 0 {
            widths.push(Constraint::Length(self.lang_width as u16));
        }
        for _ in &self.cols {
            widths.push(Constraint::Length(COL_W as u16));
        }
        widths.push(Constraint::Length(COL_W as u16)); // primary (always)
        widths
    }

    fn header(&self) -> Row<'static> {
        let right = |text: &'static str| Cell::from(Line::from(text).alignment(Alignment::Right));
        let mut cells = vec![Cell::from("name")];
        if self.lang_width > 0 {
            cells.push(Cell::from("languages"));
        }
        for col in &self.cols {
            cells.push(right(col.header));
        }
        cells.push(right(self.primary.header));
        Row::new(cells).style(
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn row(&self, loaded: &Loaded, id: NodeId) -> Row<'static> {
        let node = &loaded.tree.nodes[id];
        let primary_lang = loaded.code_at(id).and_then(|c| c.primary_lang);
        let name = loaded.display_name(id);
        let indent = loaded.display_depth(id);
        let mut cells = vec![Cell::from(Line::from(name_spans(
            node,
            &name,
            indent,
            primary_lang,
        )))];

        if self.lang_width > 0 {
            let empty = BTreeMap::new();
            let langs = loaded.code_at(id).map_or(&empty, |c| &c.langs);
            let total = loaded.value(SubKey::Lines, id) as usize;
            cells.push(Cell::from(Line::from(language_legend(
                langs,
                total,
                self.lang_width,
            ))));
        }

        let computing = loaded.active_computing();
        for col in &self.cols {
            cells.push(num_cell(loaded, col, id, computing, false));
        }
        cells.push(num_cell(loaded, &self.primary, id, computing, true));
        Row::new(cells)
    }
}

/// Total width consumed by the primary + `cols` numeric columns plus the spacing
/// between name, those columns, and (optionally) the legend.
fn column_run(cols: usize, legend_on: bool) -> usize {
    let num_cols = cols + 1; // + primary
    let spacing = 1 + num_cols + usize::from(legend_on);
    num_cols * COL_W + spacing
}

pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    // Visible tree rows = body height minus borders (2) and the header row (1).
    loaded.viewport_rows = area.height.saturating_sub(3) as usize;

    // Build the rows (and resolved columns) only when the content, width, or
    // computing state changed; otherwise reuse the cache. Pure navigation
    // (selection/scroll) doesn't alter row content — ratatui re-applies the
    // highlight from `table_state` at render — so this keeps held-key and wheel
    // scrolling instant regardless of how many rows are expanded.
    let rev = loaded.rebuild_rev;
    let computing = loaded.active_computing();
    let width = area.width;
    let fresh = matches!(
        &loaded.row_cache,
        Some(c) if c.width == width && c.rev == rev && c.computing == computing
    );

    if !fresh {
        // Inner width available to columns: minus borders (2) and the 2-cell
        // selection gutter.
        let inner = width.saturating_sub(4) as usize;
        let lens = loaded.active_lens;

        let mut name_needed = 0;
        let mut desired_legend = 0;
        for &id in &loaded.visible {
            let node = &loaded.tree.nodes[id];
            let primary_lang = loaded.code_at(id).and_then(|c| c.primary_lang);
            let name = loaded.display_name(id);
            let indent = loaded.display_depth(id);
            name_needed =
                name_needed.max(spans_width(&name_spans(node, &name, indent, primary_lang)));
            if matches!(lens, Lens::Code)
                && let Some(code) = loaded.code_at(id)
            {
                desired_legend =
                    desired_legend.max(desired_legend_width(&code.langs, code.num.lines()));
            }
        }

        let columns = Columns::choose(inner, name_needed, desired_legend, lens);
        let rows: Vec<Row> = loaded
            .visible
            .iter()
            .map(|&id| columns.row(loaded, id))
            .collect();
        loaded.row_cache = Some(RowCache {
            rows,
            header: columns.header(),
            widths: columns.widths(),
            width,
            rev,
            computing,
        });
    }

    let focused = loaded.focus == Focus::Tree;
    let cache = loaded
        .row_cache
        .as_ref()
        .expect("row_cache populated above");
    let table = Table::new(cache.rows.clone(), cache.widths.clone())
        .header(cache.header.clone())
        .column_spacing(1)
        .row_highlight_style(
            Style::default()
                .bg(theme::SELECTION_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ")
        .block(
            Block::bordered()
                .border_style(Style::default().fg(if focused {
                    theme::ACCENT
                } else {
                    theme::MUTED
                }))
                .title(" tree "),
        );

    frame.render_stateful_widget(table, area, &mut loaded.table_state);
}

/// A right-aligned numeric cell for `col` at node `id`. Shows `…` while the
/// active lens's layer is still computing; the `primary` cell is bold.
fn num_cell(
    loaded: &Loaded,
    col: &ColumnSpec,
    id: NodeId,
    computing: bool,
    primary: bool,
) -> Cell<'static> {
    let text = if computing {
        "…".to_string()
    } else {
        theme::format_value(col.key, loaded.value(col.key, id))
    };
    let mut style = Style::default();
    if let Some(color) = theme::tint_color(col.tint) {
        style = style.fg(color);
    }
    if primary {
        style = style.add_modifier(Modifier::BOLD);
    }
    Cell::from(Line::from(Span::styled(text, style)).alignment(Alignment::Right))
}

/// Render a row's name column: indentation, glyph, then `name` (the concatenated
/// chain for a directory row, the file name otherwise).
fn name_spans(
    node: &TreeNode,
    name: &str,
    indent: usize,
    primary_lang: Option<LanguageType>,
) -> Vec<Span<'static>> {
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
                format!("{name}/"),
                Style::default().fg(theme::DIR).add_modifier(Modifier::BOLD),
            ));
        }
        NodeKind::File => {
            let color = primary_lang.map_or(theme::MUTED, theme::language_color);
            spans.push(Span::styled(
                format!("{} ", theme::GLYPH_FILE),
                Style::default().fg(color),
            ));
            spans.push(Span::raw(name.to_string()));
        }
    }
    spans
}

/// Languages largest first, dropping any with no lines.
fn sorted_langs(langs: &BTreeMap<LanguageType, CodeNum>) -> Vec<(LanguageType, usize)> {
    let mut out: Vec<(LanguageType, usize)> = langs
        .iter()
        .map(|(lang, num)| (*lang, num.lines()))
        .filter(|(_, lines)| *lines > 0)
        .collect();
    out.sort_by_key(|(_, lines)| Reverse(*lines));
    out
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

/// Width the full (untruncated) legend would occupy.
fn desired_legend_width(langs: &BTreeMap<LanguageType, CodeNum>, total: usize) -> usize {
    let langs = sorted_langs(langs);
    match langs.as_slice() {
        [] => 0,
        [(lang, _)] => theme::language_label(*lang).width(),
        _ => spans_width(&legend_spans(&langs, 0, total)),
    }
}

/// The languages cell: the widest representation that fits in `width`.
fn language_legend(
    langs: &BTreeMap<LanguageType, CodeNum>,
    total: usize,
    width: usize,
) -> Vec<Span<'static>> {
    let langs = sorted_langs(langs);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokei::LanguageType as L;

    fn langs(items: &[(L, usize)]) -> BTreeMap<L, CodeNum> {
        items
            .iter()
            .map(|(lang, lines)| {
                (
                    *lang,
                    CodeNum {
                        code: *lines,
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn total(items: &[(L, usize)]) -> usize {
        items.iter().map(|(_, n)| *n).sum()
    }

    fn text(items: &[(L, usize)], width: usize) -> String {
        language_legend(&langs(items), total(items), width)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    // Rust 75.0%, Python 17.4%, TOML 4.4%, Markdown 3.2% (total 1033).
    const POLY: &[(L, usize)] = &[
        (L::Rust, 775),
        (L::Python, 180),
        (L::Toml, 45),
        (L::Markdown, 33),
    ];

    #[test]
    fn legend_lists_every_language_when_wide() {
        assert_eq!(
            text(POLY, 200),
            "Rust (75.0%), Python (17.4%), TOML (4.4%), Markdown (3.2%)"
        );
    }

    #[test]
    fn legend_collapses_tail_into_other() {
        assert_eq!(text(POLY, 27), "Rust (75.0%), Other (25.0%)");
    }

    #[test]
    fn legend_falls_back_to_a_count_then_degrades() {
        assert_eq!(text(POLY, 15), "4 languages");
        assert_eq!(text(POLY, 7), "4 langs");
        assert_eq!(text(POLY, 3), "4");
    }

    #[test]
    fn legend_single_language_is_just_the_label() {
        assert_eq!(text(&[(L::Rust, 500)], 40), "Rust");
    }

    #[test]
    fn legend_empty_is_blank() {
        assert!(language_legend(&langs(&[]), 0, 40).is_empty());
    }

    #[test]
    fn code_legend_floors_to_header_width() {
        assert_eq!(Columns::choose(96, 11, 8, Lens::Code).lang_width, LANG_MIN);
    }

    #[test]
    fn code_columns_drop_and_legend_hides_when_cramped() {
        let columns = Columns::choose(40, 30, 50, Lens::Code);
        assert!(columns.cols.len() < Lens::Code.columns().len());
        assert_eq!(columns.lang_width, 0);
    }

    #[test]
    fn size_lens_has_no_optional_columns_or_legend() {
        let columns = Columns::choose(96, 11, 0, Lens::Size);
        assert!(columns.cols.is_empty());
        assert_eq!(columns.lang_width, 0);
        assert_eq!(columns.primary.key, SubKey::Bytes);
    }
}
