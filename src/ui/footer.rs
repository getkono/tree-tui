//! The status / keybind footer, including the filter prompt and lens badge.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme;
use crate::model::{Facet, Lens, SortDir, SubKey};

#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    active_lens: Lens,
    sort_key: SubKey,
    sort_dir: SortDir,
    hide_zeros: bool,
    filter: &str,
    editing: bool,
    computing: Option<Lens>,
    area: Rect,
) {
    let badge = |text: String| {
        Span::styled(
            text,
            Style::default()
                .fg(theme::BADGE_FG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let muted = |text: String| Span::styled(text, Style::default().fg(theme::MUTED));
    let sort_text = format!("sort: {} {}", sort_key.label(), sort_dir.arrow());

    let line = if editing {
        Line::from(vec![
            badge(" filter ".to_string()),
            Span::raw(format!(" /{filter}")),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
            muted("   Enter apply · Esc cancel".to_string()),
        ])
    } else if !filter.is_empty() {
        Line::from(vec![
            badge(format!(" filter: {filter} ")),
            muted("   Esc clear · ".to_string()),
            muted(format!("{} · {sort_text}", active_lens.label())),
        ])
    } else {
        let mut spans = vec![
            badge(format!(" {} ", active_lens.label())),
            Span::raw(" "),
            muted(sort_text),
        ];
        if hide_zeros {
            spans.push(muted("  · nonzero".to_string()));
        }
        match computing {
            Some(lens) => spans.push(Span::styled(
                format!("   computing {}…", lens.label()),
                Style::default().fg(theme::WARN),
            )),
            None => {
                let used: usize = spans.iter().map(Span::width).sum();
                let budget = usize::from(area.width).saturating_sub(used);
                let hint = fit_hints(&key_hints(active_lens), budget);
                if !hint.is_empty() {
                    spans.push(muted(hint));
                }
            }
        }
        Line::from(spans)
    };

    frame.render_widget(Paragraph::new(line), area);
}

/// Gap between the sort text and the first key hint.
const HINT_LEAD: &str = "   ";
/// Separator between two key hints.
const HINT_SEP: &str = " · ";

/// One key hint and how readily it gives way when the footer is narrow:
/// a higher `drop_rank` is dropped first.
struct KeyHint {
    text: String,
    drop_rank: u8,
}

/// The key hints for `lens`, in display order.
///
/// When they don't all fit, whole hints drop out (never a clipped word):
/// `reverse` and `declutter` first, since the sort arrow and `nonzero` badge
/// already show their state; then `exclude` and `sort`; the facet keys only
/// after those, because the code lens is the only place they are advertised;
/// and `help` and `quit` last, since help lists every binding.
fn key_hints(lens: Lens) -> Vec<KeyHint> {
    let hint = |text: &str, drop_rank| KeyHint {
        text: text.to_string(),
        drop_rank,
    };
    let mut hints = vec![
        hint("m lens", 4),
        hint("s sort", 5),
        hint("r reverse", 8),
        hint("z declutter", 7),
        hint("x exclude", 6),
        hint("/ filter", 3),
    ];
    // Facets only act on the code lens, so only it advertises them.
    if lens == Lens::Code {
        let keys = Facet::ALL
            .into_iter()
            .map(|facet| facet.key().to_string())
            .collect::<Vec<_>>()
            .join("/");
        hints.push(KeyHint {
            text: format!("{keys} hide lines"),
            drop_rank: 2,
        });
    }
    hints.push(hint("? help", 1));
    hints.push(hint("q quit", 0));
    hints
}

/// Join `hints` into one muted string no wider than `budget` columns,
/// dropping whole hints, highest `drop_rank` first, until the rest fit.
/// Display order is kept; an empty string means not even one fits.
fn fit_hints(hints: &[KeyHint], budget: usize) -> String {
    let mut kept: Vec<&KeyHint> = hints.iter().collect();
    let width = |kept: &[&KeyHint]| -> usize {
        if kept.is_empty() {
            return 0;
        }
        let text: usize = kept
            .iter()
            .map(|hint| Span::raw(hint.text.as_str()).width())
            .sum();
        HINT_LEAD.chars().count() + text + HINT_SEP.chars().count() * (kept.len() - 1)
    };
    while width(&kept) > budget {
        let Some(victim) = kept
            .iter()
            .enumerate()
            .max_by_key(|(_, hint)| hint.drop_rank)
            .map(|(i, _)| i)
        else {
            break;
        };
        kept.remove(victim);
    }
    if kept.is_empty() {
        return String::new();
    }
    let joined = kept
        .iter()
        .map(|hint| hint.text.as_str())
        .collect::<Vec<_>>()
        .join(HINT_SEP);
    format!("{HINT_LEAD}{joined}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fitted(lens: Lens, budget: usize) -> String {
        fit_hints(&key_hints(lens), budget)
    }

    #[test]
    fn every_hint_shows_when_there_is_room() {
        let hint = fitted(Lens::Code, usize::MAX);
        assert_eq!(
            hint,
            "   m lens · s sort · r reverse · z declutter · x exclude · / filter · \
             t/n/o/c/b hide lines · ? help · q quit"
        );
    }

    #[test]
    fn the_fitted_hint_never_exceeds_its_budget() {
        for lens in Lens::ALL {
            for budget in 0..=140 {
                let hint = fitted(lens, budget);
                assert!(
                    Span::raw(hint.as_str()).width() <= budget,
                    "{lens:?} at {budget}: {hint:?}"
                );
            }
        }
    }

    #[test]
    fn hints_drop_whole_in_rank_order() {
        let full = fitted(Lens::Code, usize::MAX).chars().count();
        // One column short: only `r reverse` (highest rank) goes.
        let hint = fitted(Lens::Code, full - 1);
        assert!(!hint.contains("reverse"), "{hint}");
        assert!(hint.contains("z declutter"), "{hint}");
        assert!(hint.contains("t/n/o/c/b hide lines"), "{hint}");
        // Tight: the essentials outlast everything else.
        let hint = fitted(Lens::Code, 20);
        assert_eq!(hint, "   ? help · q quit");
        assert_eq!(fitted(Lens::Code, 9), "   q quit");
        assert_eq!(fitted(Lens::Code, 8), "");
    }
}
