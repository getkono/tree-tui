//! The scrollable tree table: one row per visible node.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Row, Table};

use super::theme;
use crate::app::Loaded;
use crate::model::{NodeKind, TreeNode};

const BAR_WIDTH: usize = 12;

pub fn render(frame: &mut Frame, loaded: &mut Loaded, area: Rect) {
    // Visible tree rows = body height minus borders (2) and the header row (1).
    loaded.viewport_rows = area.height.saturating_sub(3) as usize;

    let rows: Vec<Row> = loaded
        .visible
        .iter()
        .map(|&id| row_for(&loaded.tree.nodes[id]))
        .collect();

    let widths = [
        Constraint::Fill(1),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(BAR_WIDTH as u16),
    ];

    let header = Row::new(vec![
        Cell::from("name"),
        Cell::from("language"),
        Cell::from(Line::from("code").alignment(Alignment::Right)),
        Cell::from(Line::from("comments").alignment(Alignment::Right)),
        Cell::from(Line::from("blanks").alignment(Alignment::Right)),
        Cell::from(Line::from("lines").alignment(Alignment::Right)),
        Cell::from("mix"),
    ])
    .style(
        Style::default()
            .fg(theme::MUTED)
            .add_modifier(Modifier::BOLD),
    );

    let table = Table::new(rows, widths)
        .header(header)
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

fn row_for(node: &TreeNode) -> Row<'static> {
    let indent = node.depth.saturating_sub(1);
    let mut name = vec![Span::raw("  ".repeat(indent))];
    match node.kind {
        NodeKind::Dir => {
            let glyph = if node.expanded {
                theme::GLYPH_EXPANDED
            } else {
                theme::GLYPH_COLLAPSED
            };
            name.push(Span::styled(
                format!("{glyph} "),
                Style::default().fg(theme::DIR),
            ));
            name.push(Span::styled(
                format!("{}/", node.name),
                Style::default().fg(theme::DIR).add_modifier(Modifier::BOLD),
            ));
        }
        NodeKind::File => {
            let color = node
                .primary_lang
                .map_or(theme::MUTED, theme::language_color);
            name.push(Span::styled(
                format!("{} ", theme::GLYPH_FILE),
                Style::default().fg(color),
            ));
            name.push(Span::raw(node.name.clone()));
        }
    }

    let language = match node.kind {
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
    };

    Row::new(vec![
        Cell::from(Line::from(name)),
        Cell::from(Line::from(language)),
        num_cell(node.stats.code, theme::CODE),
        num_cell(node.stats.comments, theme::COMMENTS),
        num_cell(node.stats.blanks, theme::BLANKS),
        Cell::from(
            Line::from(Span::styled(
                theme::group_thousands(node.stats.lines()),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
        ),
        Cell::from(Line::from(theme::composition_bar(&node.stats, BAR_WIDTH))),
    ])
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
