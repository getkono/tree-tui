//! The summary header: root label, totals, scan time, and the active-lens recap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::theme;
use crate::app::Loaded;
use crate::model::{Facet, Hidden, Lens, SubKey};

/// Draw the header, returning the code lens's facet chips (rect + facet) for
/// mouse hit-testing — empty on other lenses.
pub fn render(
    frame: &mut Frame,
    root_label: &str,
    head_hash: Option<&str>,
    loaded: &Loaded,
    area: Rect,
) -> Vec<(Rect, Facet)> {
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
        // Facets act only on the code lens, so only there do they thin the count.
        let hidden = if loaded.active_lens == Lens::Code {
            loaded.hidden
        } else {
            Hidden::default()
        };
        line1.push(dot);
        line1.push(Span::styled(
            format!("{} languages", code.counted_langs(hidden).len()),
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
        recap.spans.insert(0, Span::raw(RECAP_GAP));
        recap.spans.insert(
            0,
            Span::styled(hash.to_string(), Style::default().fg(theme::MUTED)),
        );
    }

    let block = Block::bordered()
        .title(" tree ")
        .border_style(Style::default().fg(theme::MUTED))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let recap_width = recap.width() as u16;
    // Where each recap field ends: the start of every gap between fields.
    let mut field_ends = Vec::new();
    let mut x = 0;
    for span in &recap.spans {
        if span.content == RECAP_GAP {
            field_ends.push(x);
        }
        x += span.width() as u16;
    }
    frame.render_widget(
        Paragraph::new(vec![Line::from(line1), recap]).block(block),
        area,
    );

    if loaded.active_lens == Lens::Code && inner.height >= 2 {
        let row = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        render_facet_chips(frame, loaded.hidden, row, recap_width, &field_ends)
    } else {
        Vec::new()
    }
}

/// The gap between the recap's fields.
const RECAP_GAP: &str = "   ";

/// The gap kept between the recap and the facet chips.
const CHIP_GAP: u16 = 3;

/// The chip texts for one layout: `labelled(facet)` says whether a chip keeps
/// its label (`t test`) or shrinks to its key (`t`).
fn chip_texts(labelled: impl Fn(Facet) -> bool) -> Vec<(Facet, String)> {
    Facet::live()
        .map(|facet| {
            let text = if labelled(facet) {
                format!("{} {}", facet.key(), facet.label())
            } else {
                facet.key().to_string()
            };
            (facet, text)
        })
        .collect()
}

/// The width of a chip row: its texts, one column apart.
fn chips_width(chips: &[(Facet, String)]) -> u16 {
    chips
        .iter()
        .map(|(_, text)| text.len() as u16 + 1)
        .sum::<u16>()
        .saturating_sub(1)
}

/// The widest chip layout that fits beside the recap: every chip labelled;
/// else only the hidden ones labelled, so a hidden facet always names itself;
/// else every chip as its bare key. `None` when even the keys do not fit.
fn fit_chips(hidden: Hidden, room: u16) -> Option<Vec<(Facet, String)>> {
    [
        chip_texts(|_| true),
        chip_texts(|facet| hidden.contains(facet)),
        chip_texts(|_| false),
    ]
    .into_iter()
    .find(|chips| chips_width(chips) <= room)
}

/// The code lens's facet chips, right-aligned on `row`: one per live facet,
/// lit while its lines count and struck through while hidden. As the row
/// narrows, the chips shrink to their keys (see [`fit_chips`]). When even the
/// keys do not fit beside the recap's `recap_width` columns, they are left out
/// while nothing is hidden; while something is, the recap's trailing fields
/// are blanked whole (it ends at one of `field_ends`, never mid-number) to make
/// room for the keys, so reduced totals never show without a sign. Returns
/// each chip's rect for click hit-testing.
fn render_facet_chips(
    frame: &mut Frame,
    hidden: Hidden,
    row: Rect,
    recap_width: u16,
    field_ends: &[u16],
) -> Vec<(Rect, Facet)> {
    let room = row.width.saturating_sub(recap_width + CHIP_GAP);
    let chips = match fit_chips(hidden, room) {
        Some(chips) => chips,
        None if !hidden.is_empty() => {
            let keys = chip_texts(|_| false);
            let Some(avail) = row.width.checked_sub(chips_width(&keys) + CHIP_GAP) else {
                return Vec::new();
            };
            let keep = field_ends
                .iter()
                .copied()
                .filter(|&end| end <= avail)
                .max()
                .unwrap_or(0);
            frame.render_widget(
                Clear,
                Rect {
                    x: row.x + keep,
                    width: row.width - keep,
                    ..row
                },
            );
            keys
        }
        None => return Vec::new(),
    };
    let width = chips_width(&chips);

    let mut x = row.x + row.width - width;
    let mut rects = Vec::with_capacity(chips.len());
    for (facet, text) in chips {
        let rect = Rect {
            x,
            width: text.len() as u16,
            ..row
        };
        let (key_style, label_style) = if hidden.contains(facet) {
            let struck = Style::default()
                .fg(theme::MUTED)
                .add_modifier(Modifier::CROSSED_OUT);
            (struck, struck)
        } else {
            (
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
                Style::default(),
            )
        };
        let (key, label) = text.split_at(1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(key.to_string(), key_style),
                Span::styled(label.to_string(), label_style),
            ])),
            rect,
        );
        rects.push((rect, facet));
        x += rect.width + 1;
    }
    rects
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
    let gap = RECAP_GAP;
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

    // A hidden kind still shows its count, struck through: it is left out of
    // `lines`, not out of the tree.
    let kind = |span: Span<'static>, facet: Facet| {
        if loaded.hidden.contains(facet) {
            span.patch_style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::CROSSED_OUT),
            )
        } else {
            span
        }
    };

    match loaded.active_lens {
        Lens::Code => Line::from(vec![
            strong(value(SubKey::Lines)),
            label(" lines"),
            Span::raw(gap),
            tinted(value(SubKey::Code), theme::CODE),
            label(" code"),
            Span::raw(gap),
            kind(
                tinted(value(SubKey::Comments), theme::COMMENTS),
                Facet::Comment,
            ),
            kind(label(" comments"), Facet::Comment),
            Span::raw(gap),
            kind(tinted(value(SubKey::Blanks), theme::BLANKS), Facet::Blank),
            kind(label(" blanks"), Facet::Blank),
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
