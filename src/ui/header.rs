//! The summary header: root label, totals, scan time, and the active-lens recap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::theme;
use crate::app::Loaded;
use crate::model::{Lens, SubKey};

pub fn render(
    frame: &mut Frame,
    root_label: &str,
    head_hash: Option<&str>,
    loaded: &Loaded,
    area: Rect,
) {
    let tree = &loaded.tree;
    let root = tree.root;
    let scanned = if loaded.duration.as_millis() >= 1000 {
        format!("{:.2}s", loaded.duration.as_secs_f64())
    } else {
        format!("{}ms", loaded.duration.as_millis())
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
        Span::raw(format!(
            "{} files",
            theme::group_thousands(loaded.effective_value(SubKey::Files, root) as usize)
        )),
        dot.clone(),
        Span::raw(theme::human_bytes(
            loaded.effective_value(SubKey::Bytes, root) as u64,
        )),
        dot.clone(),
        Span::styled(
            format!("scanned in {scanned}"),
            Style::default().fg(theme::MUTED),
        ),
    ];
    if let Some(code) = loaded.code_at(root) {
        line1.push(dot);
        line1.push(Span::styled(
            format!("{} languages", code.langs.len()),
            Style::default().fg(theme::MUTED),
        ));
    }
    if loaded.inaccurate {
        line1.push(Span::styled(
            "  ⚠ some files inaccurate",
            Style::default().fg(theme::WARN),
        ));
    }

    // The second line summarizes the active lens's grand totals (for the default
    // code lens, the LOC summary). Prefix the repo's HEAD short hash to its left.
    let mut recap = lens_recap(loaded);
    if let Some(hash) = head_hash {
        recap.spans.insert(0, Span::raw("   "));
        recap.spans.insert(
            0,
            Span::styled(hash.to_string(), Style::default().fg(theme::MUTED)),
        );
    }

    let block = Block::bordered()
        .title(" tree ")
        .border_style(Style::default().fg(theme::MUTED))
        .padding(Padding::horizontal(1));
    frame.render_widget(
        Paragraph::new(vec![Line::from(line1), recap]).block(block),
        area,
    );
}

/// The second header line: a recap of the active lens's grand totals.
fn lens_recap(loaded: &Loaded) -> Line<'static> {
    if loaded.active_computing() {
        return Line::from(Span::styled(
            format!("computing {}…", loaded.active_lens.label()),
            Style::default()
                .fg(theme::WARN)
                .add_modifier(Modifier::ITALIC),
        ));
    }

    let root = loaded.tree.root;
    let value = |key: SubKey| loaded.effective_value(key, root);
    let gap = "   ";
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme::MUTED));
    let strong = |value: u128| {
        Span::styled(
            theme::group_thousands(value as usize),
            Style::default().add_modifier(Modifier::BOLD),
        )
    };
    let tinted = |value: u128, color| {
        Span::styled(
            theme::group_thousands(value as usize),
            Style::default().fg(color),
        )
    };

    match loaded.active_lens {
        Lens::Code => Line::from(vec![
            strong(value(SubKey::Lines)),
            label(" lines"),
            Span::raw(gap),
            tinted(value(SubKey::Code), theme::CODE),
            label(" code"),
            Span::raw(gap),
            tinted(value(SubKey::Comments), theme::COMMENTS),
            label(" comments"),
            Span::raw(gap),
            tinted(value(SubKey::Blanks), theme::BLANKS),
            label(" blanks"),
        ]),
        Lens::Size => Line::from(vec![
            Span::styled(
                theme::human_bytes(value(SubKey::Bytes) as u64),
                Style::default()
                    .fg(theme::SIZE)
                    .add_modifier(Modifier::BOLD),
            ),
            label(" on disk across "),
            strong(value(SubKey::Files)),
            label(" files"),
        ]),
        Lens::Churn => Line::from(vec![
            tinted(value(SubKey::Added), theme::ADD),
            label(" added"),
            Span::raw(gap),
            tinted(value(SubKey::Deleted), theme::DEL),
            label(" deleted"),
            Span::raw(gap),
            strong(value(SubKey::Commits)),
            label(" commits"),
        ]),
        Lens::Status => Line::from(vec![
            tinted(value(SubKey::StatusAdded), theme::ADD),
            label(" added"),
            Span::raw(gap),
            tinted(value(SubKey::StatusModified), theme::STATUS),
            label(" modified"),
            Span::raw(gap),
            tinted(value(SubKey::StatusDeleted), theme::DEL),
            label(" deleted"),
        ]),
    }
}
