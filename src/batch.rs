//! Coalescing of bursty input events (key auto-repeat, mouse wheel) so a flood
//! of events yields one net state change and a single redraw.
//!
//! The event loop ([`crate::event`]) drains everything that is immediately
//! ready into a `Vec<Event>`, then hands it here. Keeping the collapse logic in
//! a pure function makes it unit-testable without driving a real terminal: the
//! impure draining stays in the loop, the rules live here.

use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};

/// A single unit of work the event loop applies after coalescing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A key press or repeat (release kinds are dropped during coalescing).
    Key(KeyEvent),
    /// Net wheel scroll at a screen position: positive = down, negative = up.
    /// The handler maps this to a small fixed step — magnitude is direction
    /// only, never the raw event count (terminals emit several per notch).
    Scroll { col: u16, row: u16, delta: i32 },
    /// A left click at a screen position (interact-to-focus).
    Click { col: u16, row: u16 },
    /// A redraw-worthy resize.
    Resize,
}

/// Largest number of events drained from one burst, so a pathological flood
/// can't starve the other `select!` arms (scan, lens, watch, ticker) for long.
pub const MAX_BATCH: usize = 256;

/// Collapse a drained burst of raw events into the minimal set of effects.
///
/// Rules:
/// - Key **presses and repeats** are preserved individually and in order — each
///   can have a distinct, non-idempotent effect (expand, open, lens jump), so
///   they must not be merged. Repeats drive held-key navigation; only release
///   kinds are dropped.
/// - Consecutive wheel events at the same spot and direction sum into one
///   [`Effect::Scroll`]; a direction flip or a different position flushes the
///   run. (The handler uses only the sign — see [`Effect::Scroll`].)
/// - A left click becomes one [`Effect::Click`]; other mouse events (motion,
///   drags, other buttons) are dropped.
/// - Resizes collapse to a single [`Effect::Resize`].
pub fn coalesce(events: impl IntoIterator<Item = Event>) -> Vec<Effect> {
    let mut out: Vec<Effect> = Vec::new();
    for ev in events {
        match ev {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                out.push(Effect::Key(key))
            }
            // Release kinds carry no action of their own.
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
/// same-position wheel runs. A left click becomes a [`Effect::Click`]; motion,
/// drags, and other buttons are ignored (interact-to-focus has no use for
/// bare hover).
fn merge_mouse(out: &mut Vec<Effect>, m: MouseEvent) {
    let step: i32 = match m.kind {
        MouseEventKind::ScrollDown => 1,
        MouseEventKind::ScrollUp => -1,
        MouseEventKind::Down(MouseButton::Left) => {
            out.push(Effect::Click {
                col: m.column,
                row: m.row,
            });
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
    fn motion_events_are_ignored() {
        let burst = (0..5).map(|i| mouse(MouseEventKind::Moved, i, i));
        assert!(coalesce(burst).is_empty());
    }

    #[test]
    fn left_click_becomes_one_click_effect() {
        let burst = vec![mouse(MouseEventKind::Down(MouseButton::Left), 7, 3)];
        assert_eq!(coalesce(burst), vec![Effect::Click { col: 7, row: 3 }]);
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
    fn release_is_dropped_but_repeat_is_kept() {
        let burst = vec![
            key_kind(KeyCode::Char('j'), KeyEventKind::Repeat),
            key_kind(KeyCode::Char('j'), KeyEventKind::Release),
        ];
        // The repeat drives held-key navigation; only the release is dropped.
        assert_eq!(
            coalesce(burst),
            vec![Effect::Key(KeyEvent::new_with_kind(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            ))]
        );
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
    fn drag_release_and_non_left_buttons_are_ignored() {
        let burst = vec![
            mouse(MouseEventKind::Drag(MouseButton::Left), 2, 2),
            mouse(MouseEventKind::Up(MouseButton::Left), 3, 3),
            mouse(MouseEventKind::Down(MouseButton::Right), 4, 4),
        ];
        assert!(coalesce(burst).is_empty());
    }
}
