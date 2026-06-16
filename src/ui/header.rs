//! The summary header: root label, language/file counts, scan time, and totals.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::theme;
use crate::model::Tree;

pub fn render(
    frame: &mut Frame,
    root_label: &str,
    tree: &Tree,
    duration: Duration,
    inaccurate: bool,
    area: Rect,
) {
    let totals = tree.totals();
    let language_count = tree.nodes[tree.root].langs.len();
    let scanned = if duration.as_millis() >= 1000 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    };

    let dot = Span::styled("  ·  ", Style::default().fg(theme::MUTED));
    let mut line1 = vec![
        Span::styled(
            root_label.to_string(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        dot.clone(),
        Span::raw(format!("{language_count} languages")),
        dot.clone(),
        Span::raw(format!("{} files", theme::group_thousands(totals.files))),
        dot,
        Span::styled(
            format!("scanned in {scanned}"),
            Style::default().fg(theme::MUTED),
        ),
    ];
    if inaccurate {
        line1.push(Span::styled(
            "  ⚠ some files inaccurate",
            Style::default().fg(theme::WARN),
        ));
    }

    let gap = "   ";
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme::MUTED));
    let line2 = Line::from(vec![
        Span::styled(
            theme::group_thousands(totals.lines()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        label(" lines"),
        Span::raw(gap),
        Span::styled(
            theme::group_thousands(totals.code),
            Style::default().fg(theme::CODE),
        ),
        label(" code"),
        Span::raw(gap),
        Span::styled(
            theme::group_thousands(totals.comments),
            Style::default().fg(theme::COMMENTS),
        ),
        label(" comments"),
        Span::raw(gap),
        Span::styled(
            theme::group_thousands(totals.blanks),
            Style::default().fg(theme::BLANKS),
        ),
        label(" blanks"),
    ]);

    let block = Block::bordered()
        .title(" tree ")
        .border_style(Style::default().fg(theme::MUTED))
        .padding(Padding::horizontal(1));
    frame.render_widget(
        Paragraph::new(vec![Line::from(line1), line2]).block(block),
        area,
    );
}
