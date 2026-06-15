//! The detail panel: the selected node's path, totals, and language breakdown.

use std::cmp::Reverse;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::theme;
use crate::app::Loaded;
use crate::model::{NodeKind, Stats};

const BAR_WIDTH: usize = 10;

pub fn render(frame: &mut Frame, loaded: &Loaded, area: Rect) {
    let block = Block::bordered()
        .title(" detail ")
        .border_style(Style::default().fg(theme::MUTED))
        .padding(Padding::horizontal(1));

    let Some(node) = loaded.selected_id().map(|id| &loaded.tree.nodes[id]) else {
        frame.render_widget(block, area);
        return;
    };

    let mut lines = Vec::new();

    let title_color = match node.kind {
        NodeKind::Dir => theme::DIR,
        NodeKind::File => node
            .primary_lang
            .map_or(theme::ACCENT, theme::language_color),
    };
    lines.push(Line::from(Span::styled(
        node.name.clone(),
        Style::default()
            .fg(title_color)
            .add_modifier(Modifier::BOLD),
    )));
    let subtitle = match node.kind {
        NodeKind::Dir => format!(
            "directory · {} files",
            theme::group_thousands(node.stats.files)
        ),
        NodeKind::File => "file".to_string(),
    };
    lines.push(Line::from(Span::styled(
        subtitle,
        Style::default().fg(theme::MUTED),
    )));
    lines.push(Line::default());

    lines.push(stat_line("lines", node.stats.lines(), None));
    lines.push(stat_line("code", node.stats.code, Some(theme::CODE)));
    lines.push(stat_line(
        "comments",
        node.stats.comments,
        Some(theme::COMMENTS),
    ));
    lines.push(stat_line("blanks", node.stats.blanks, Some(theme::BLANKS)));

    // Code / comment / blank composition, as a single recap bar.
    let mut composition = vec![Span::styled(
        format!("{:<10} ", "mix"),
        Style::default().fg(theme::MUTED),
    )];
    composition.extend(theme::composition_bar(&node.stats, BAR_WIDTH));
    lines.push(Line::from(composition));
    lines.push(Line::default());

    lines.push(Line::from(Span::styled(
        "languages",
        Style::default()
            .fg(theme::MUTED)
            .add_modifier(Modifier::BOLD),
    )));

    let total = node.stats.lines().max(1);
    let mut langs: Vec<(_, &Stats)> = node.langs.iter().collect();
    langs.sort_by_key(|(_, stats)| Reverse(stats.lines()));
    for (lang, stats) in langs {
        let color = theme::language_color(*lang);
        let mut spans = vec![Span::styled(
            format!("{:<10} ", theme::language_label(*lang)),
            Style::default().fg(color),
        )];
        spans.extend(theme::ratio_bar(stats.lines(), total, BAR_WIDTH, color));
        spans.push(Span::styled(
            format!(" {:>6}", theme::percent(stats.lines(), total)),
            Style::default().fg(theme::MUTED),
        ));
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn stat_line(
    label: &'static str,
    value: usize,
    color: Option<ratatui::style::Color>,
) -> Line<'static> {
    let value_style = color.map_or_else(
        || Style::default().add_modifier(Modifier::BOLD),
        |c| Style::default().fg(c),
    );
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::default().fg(theme::MUTED)),
        Span::styled(theme::group_thousands(value), value_style),
    ])
}
