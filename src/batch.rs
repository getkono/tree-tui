//! Coalescing of bursty input events (key auto-repeat, mouse wheel) so a flood
//! of events yields one net state change and a single redraw.
//!
//! The event loop ([`crate::event`]) drains everything that is immediately
//! ready into a `Vec<Event>`, then hands it here. Keeping the collapse logic in
//! a pure function makes it unit-testable without driving a real terminal: the
//! impure draining stays in the loop, the rules live here.

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};

/// A single unit of work the event loop applies after coalescing. The mouse
/// variants are produced here but only acted on once mouse capture is enabled
/// (see issue #23); until then the loop treats them as no-ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A key press (release/repeat kinds are dropped during coalescing).
    Key(KeyEvent),
    /// Net wheel scroll at a screen position: positive = down, negative = up.
    Scroll { col: u16, row: u16, delta: i32 },
    /// The final cursor position after a run of moves (focus-follows-mouse).
    MouseMoved { col: u16, row: u16 },
    /// A redraw-worthy resize.
    Resize,
}

/// Largest number of events drained from one burst, so a pathological flood
/// can't starve the other `select!` arms (scan, lens, watch, ticker) for long.
pub const MAX_BATCH: usize = 256;

/// Collapse a drained burst of raw events into the minimal set of effects.
///
/// Rules:
/// - Key **presses** are preserved individually and in order — each can have a
///   distinct, non-idempotent effect (expand, open, lens jump), so they must
///   not be merged. Release/repeat kinds are dropped.
/// - Consecutive wheel events at the same spot and direction sum into one
///   [`Effect::Scroll`]; a direction flip or a different position flushes the
///   run.
/// - A run of `Moved` events keeps only the latest position.
/// - Resizes collapse to a single [`Effect::Resize`].
pub fn coalesce(events: impl IntoIterator<Item = Event>) -> Vec<Effect> {
    let mut out: Vec<Effect> = Vec::new();
    for ev in events {
        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => out.push(Effect::Key(key)),
            // Release/Repeat kinds carry no action of their own.
            Event::Key(_) => {}
            Event::Resize(_, _) => {
                if !matches!(out.last(), Some(Effect::Resize)) {
                    out.push(Effect::Resize);
                }
            }
            Event::Mouse(m) => merge_mouse(&mut out, m),
            // FocusGained/FocusLost/Paste: nothing to do.
            _ => {}
        }
    }
    out
}

/// Fold a single mouse event into the running effect list, merging adjacent
/// same-position wheel runs and collapsing move runs to the latest position.
fn merge_mouse(out: &mut Vec<Effect>, m: MouseEvent) {
    let step: i32 = match m.kind {
        MouseEventKind::ScrollDown => 1,
        MouseEventKind::ScrollUp => -1,
        // Moves coalesce to the latest position; everything else is unused by
        // the wheel-scroll / focus-follows-mouse model.
        MouseEventKind::Moved => {
            if let Some(Effect::MouseMoved { col, row }) = out.last_mut() {
                *col = m.column;
                *row = m.row;
            } else {
                out.push(Effect::MouseMoved {
                    col: m.column,
                    row: m.row,
                });
            }
            return;
        }
        _ => return,
    };

    if let Some(Effect::Scroll { col, row, delta }) = out.last_mut()
        && *col == m.column
        && *row == m.row
        && delta.signum() == step.signum()
    {
        *delta += step;
        return;
    }
    out.push(Effect::Scroll {
        col: m.column,
        row: m.row,
        delta: step,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn key_kind(code: KeyCode, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind))
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn scroll_downs_at_one_spot_coalesce_into_one() {
        let burst = (0..10).map(|_| mouse(MouseEventKind::ScrollDown, 5, 5));
        assert_eq!(
            coalesce(burst),
            vec![Effect::Scroll {
                col: 5,
                row: 5,
                delta: 10
            }]
        );
    }

    #[test]
    fn direction_flip_flushes_the_run() {
        let burst = vec![
            mouse(MouseEventKind::ScrollDown, 5, 5),
            mouse(MouseEventKind::ScrollDown, 5, 5),
            mouse(MouseEventKind::ScrollDown, 5, 5),
            mouse(MouseEventKind::ScrollUp, 5, 5),
            mouse(MouseEventKind::ScrollUp, 5, 5),
        ];
        assert_eq!(
            coalesce(burst),
            vec![
                Effect::Scroll {
                    col: 5,
                    row: 5,
                    delta: 3
                },
                Effect::Scroll {
                    col: 5,
                    row: 5,
                    delta: -2
                },
            ]
        );
    }

    #[test]
    fn scroll_at_a_different_position_does_not_merge() {
        let burst = vec![
            mouse(MouseEventKind::ScrollDown, 5, 5),
            mouse(MouseEventKind::ScrollDown, 80, 5),
        ];
        assert_eq!(
            coalesce(burst),
            vec![
                Effect::Scroll {
                    col: 5,
                    row: 5,
                    delta: 1
                },
                Effect::Scroll {
                    col: 80,
                    row: 5,
                    delta: 1
                },
            ]
        );
    }

    #[test]
    fn moves_keep_only_the_latest_position() {
        let burst = (0..5).map(|i| mouse(MouseEventKind::Moved, i, i));
        assert_eq!(coalesce(burst), vec![Effect::MouseMoved { col: 4, row: 4 }]);
    }

    #[test]
    fn keys_are_preserved_in_order() {
        let burst = vec![
            key(KeyCode::Char('j')),
            key(KeyCode::Char('j')),
            key(KeyCode::Enter),
        ];
        assert_eq!(
            coalesce(burst),
            vec![
                Effect::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
                Effect::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
                Effect::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ]
        );
    }

    #[test]
    fn key_release_kinds_are_dropped() {
        let burst = vec![
            key_kind(KeyCode::Char('j'), KeyEventKind::Release),
            key_kind(KeyCode::Char('j'), KeyEventKind::Repeat),
        ];
        assert!(coalesce(burst).is_empty());
    }

    #[test]
    fn resizes_collapse_to_one() {
        let burst = vec![Event::Resize(10, 10), Event::Resize(20, 20)];
        assert_eq!(coalesce(burst), vec![Effect::Resize]);
    }

    #[test]
    fn a_key_between_scrolls_flushes_the_run() {
        let burst = vec![
            mouse(MouseEventKind::ScrollDown, 5, 5),
            key(KeyCode::Char('j')),
            mouse(MouseEventKind::ScrollDown, 5, 5),
        ];
        assert_eq!(
            coalesce(burst),
            vec![
                Effect::Scroll {
                    col: 5,
                    row: 5,
                    delta: 1
                },
                Effect::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
                Effect::Scroll {
                    col: 5,
                    row: 5,
                    delta: 1
                },
            ]
        );
    }

    #[test]
    fn drag_and_button_events_are_ignored() {
        let burst = vec![
            mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
            mouse(MouseEventKind::Drag(MouseButton::Left), 2, 2),
            mouse(MouseEventKind::Up(MouseButton::Left), 3, 3),
        ];
        assert!(coalesce(burst).is_empty());
    }
}
