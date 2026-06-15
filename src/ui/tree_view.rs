//! The scrollable tree table: one row per visible node.
//!
//! Columns are dropped progressively (mix bar → blanks → comments → language)
//! as the available width shrinks, so the name and total-lines columns always
//! stay readable — important when the detail panel is open.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};

use super::theme;
use crate::app::Loaded;
use crate::model::{NodeKind, TreeNode};

const BAR_WIDTH: usize = 12;

/// Which optional columns are shown, chosen from the available width.
struct Columns {
    lang: bool,
    code: bool,
    comments: bool,
    blanks: bool,
    mix: bool,
}

impl Columns {
    fn for_width(width: u16) -> Self {
        Self {
            code: width >= 28,
            lang: width >= 46,
            comments: width >= 60,
            blanks: width >= 72,
            mix: width >= 90,
        }
    }

    fn widths(&self) -> Vec<Constraint> {
        let mut widths = vec![Constraint::Fill(1)];
        if self.lang {
            widths.push(Constraint::Length(12));
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
        if self.mix {
            widths.push(Constraint::Length(BAR_WIDTH as u16));
        }
        widths
    }

    fn header(&self) -> Row<'static> {
        let right = |text: &'static str| Cell::from(Line::from(text).alignment(Alignment::Right));
        let mut cells = vec![Cell::from("name")];
        if self.lang {
            cells.push(Cell::from("language"));
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
        if self.mix {
            cells.push(Cell::from("mix"));
        }
        Row::new(cells).style(
            Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn row(&self, node: &TreeNode) -> Row<'static> {
        let mut cells = vec![Cell::from(Line::from(name_spans(node)))];
        if self.lang {
            cells.push(Cell::from(Line::from(language_span(node))));
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
        if self.mix {
            cells.push(Cell::from(Line::from(theme::composition_bar(
                &node.stats,
                BAR_WIDTH,
            ))));
        }
        Row::new(cells)
    }
}

pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    // Visible tree rows = body height minus borders (2) and the header row (1).
    loaded.viewport_rows = area.height.saturating_sub(3) as usize;

    // Inner width available to columns: minus borders (2) and the 2-cell
    // selection gutter.
    let columns = Columns::for_width(area.width.saturating_sub(4));

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

fn language_span(node: &TreeNode) -> Span<'static> {
    match node.kind {
        NodeKind::File => match node.primary_lang {
            Some(lang) => Span::styled(
                theme::language_label(lang),
                Style::default().fg(theme::language_color(lang)),
            ),
            None => Span::raw(""),
        },
        NodeKind::Dir => {
            let n = node.langs.len();
            let suffix = if n == 1 { "" } else { "s" };
            Span::styled(
                format!("{n} lang{suffix}"),
                Style::default().fg(theme::MUTED),
            )
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
