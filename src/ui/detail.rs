//! The detail panel: the selected node's identity plus an active-lens breakdown.

use std::cmp::Reverse;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::theme;
use crate::app::Loaded;
use crate::model::{Lens, NodeId, NodeKind};

const BAR_WIDTH: usize = 10;

pub fn render(frame: &mut Frame, loaded: &Loaded, area: Rect) {
    let block = Block::bordered()
        .title(" detail ")
        .border_style(Style::default().fg(theme::MUTED))
        .padding(Padding::horizontal(1));

    let Some(id) = loaded.selected_id() else {
        frame.render_widget(block, area);
        return;
    };
    let node = &loaded.tree.nodes[id];

    let mut lines = Vec::new();

    // Identity block (always).
    let title_color = match node.kind {
        NodeKind::Dir => theme::DIR,
        NodeKind::File => loaded
            .code_at(id)
            .and_then(|c| c.primary_lang)
            .map_or(theme::ACCENT, theme::language_color),
    };
    lines.push(Line::from(Span::styled(
        loaded.display_name(id),
        Style::default()
            .fg(title_color)
            .add_modifier(Modifier::BOLD),
    )));
    let subtitle = match node.kind {
        NodeKind::Dir => format!(
            "directory · {} files · {}",
            theme::group_thousands(node.files),
            theme::human_bytes(node.bytes)
        ),
        NodeKind::File => format!("file · {}", theme::human_bytes(node.bytes)),
    };
    lines.push(Line::from(Span::styled(
        subtitle,
        Style::default().fg(theme::MUTED),
    )));
    lines.push(Line::default());

    // Active-lens section.
    match loaded.active_lens {
        Lens::Code => code_section(&mut lines, loaded, id),
        Lens::Size => size_section(&mut lines, loaded, id),
        Lens::Churn => churn_section(&mut lines, loaded, id),
        Lens::Status => status_section(&mut lines, loaded, id),
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn code_section(lines: &mut Vec<Line<'static>>, loaded: &Loaded, id: NodeId) {
    let Some(code) = loaded.code_at(id) else {
        lines.push(computing_line());
        return;
    };

    lines.push(stat_line("lines", code.num.lines(), None));
    lines.push(stat_line("code", code.num.code, Some(theme::CODE)));
    lines.push(stat_line(
        "comments",
        code.num.comments,
        Some(theme::COMMENTS),
    ));
    lines.push(stat_line("blanks", code.num.blanks, Some(theme::BLANKS)));

    let mut mix = vec![label_span("mix")];
    mix.extend(theme::segments_bar(
        &[
            (code.num.code, theme::CODE),
            (code.num.comments, theme::COMMENTS),
            (code.num.blanks, theme::BLANKS),
        ],
        BAR_WIDTH,
    ));
    lines.push(Line::from(mix));
    lines.push(Line::default());

    lines.push(heading("languages"));
    let total = code.num.lines().max(1);
    let mut langs: Vec<_> = code.langs.iter().filter(|(_, n)| n.lines() > 0).collect();
    langs.sort_by_key(|(_, num)| Reverse(num.lines()));
    for (lang, num) in langs {
        let color = theme::language_color(*lang);
        let mut spans = vec![Span::styled(
            format!("{:<10} ", theme::language_label(*lang)),
            Style::default().fg(color),
        )];
        spans.extend(theme::ratio_bar(num.lines(), total, BAR_WIDTH, color));
        spans.push(Span::styled(
            format!(" {:>6}", theme::percent(num.lines(), total)),
            Style::default().fg(theme::MUTED),
        ));
        lines.push(Line::from(spans));
    }
}

fn size_section(lines: &mut Vec<Line<'static>>, loaded: &Loaded, id: NodeId) {
    let bytes = loaded.tree.nodes[id].bytes;
    lines.push(text_line(
        "size",
        theme::human_bytes(bytes),
        Some(theme::SIZE),
    ));
    lines.push(stat_line("files", loaded.tree.nodes[id].files, None));
    lines.push(Line::default());
    lines.push(share_line(
        "of repo",
        bytes as usize,
        loaded.tree.total_bytes() as usize,
        theme::SIZE,
    ));
}

fn churn_section(lines: &mut Vec<Line<'static>>, loaded: &Loaded, id: NodeId) {
    let Some(churn) = loaded.churn_at(id) else {
        lines.push(computing_line());
        return;
    };
    let added = churn.added as usize;
    let deleted = churn.deleted as usize;
    lines.push(stat_line("added", added, Some(theme::ADD)));
    lines.push(stat_line("deleted", deleted, Some(theme::DEL)));
    lines.push(stat_line("commits", churn.commits as usize, None));

    let mut bar = vec![label_span("churn")];
    bar.extend(theme::segments_bar(
        &[(added, theme::ADD), (deleted, theme::DEL)],
        BAR_WIDTH,
    ));
    lines.push(Line::from(bar));
}

fn status_section(lines: &mut Vec<Line<'static>>, loaded: &Loaded, id: NodeId) {
    let Some(status) = loaded.status_at(id) else {
        lines.push(computing_line());
        return;
    };
    lines.push(stat_line("added", status.added, Some(theme::ADD)));
    lines.push(stat_line("modified", status.modified, Some(theme::STATUS)));
    lines.push(stat_line("deleted", status.deleted, Some(theme::DEL)));

    let mut bar = vec![label_span("changes")];
    bar.extend(theme::segments_bar(
        &[
            (status.added, theme::ADD),
            (status.modified, theme::STATUS),
            (status.deleted, theme::DEL),
        ],
        BAR_WIDTH,
    ));
    lines.push(Line::from(bar));
}

fn stat_line(label: &'static str, value: usize, color: Option<Color>) -> Line<'static> {
    text_line(label, theme::group_thousands(value), color)
}

fn text_line(label: &'static str, value: String, color: Option<Color>) -> Line<'static> {
    let value_style = color.map_or_else(
        || Style::default().add_modifier(Modifier::BOLD),
        |c| Style::default().fg(c),
    );
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::default().fg(theme::MUTED)),
        Span::styled(value, value_style),
    ])
}

fn share_line(label: &'static str, value: usize, total: usize, color: Color) -> Line<'static> {
    let mut spans = vec![label_span(label)];
    spans.extend(theme::ratio_bar(value, total, BAR_WIDTH, color));
    spans.push(Span::styled(
        format!(" {:>6}", theme::percent(value, total)),
        Style::default().fg(theme::MUTED),
    ));
    Line::from(spans)
}

fn label_span(label: &'static str) -> Span<'static> {
    Span::styled(format!("{label:<10} "), Style::default().fg(theme::MUTED))
}

fn heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme::MUTED)
            .add_modifier(Modifier::BOLD),
    ))
}

fn computing_line() -> Line<'static> {
    Line::from(Span::styled(
        "computing…",
        Style::default()
            .fg(theme::MUTED)
            .add_modifier(Modifier::ITALIC),
    ))
}
