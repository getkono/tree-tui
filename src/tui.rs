//! Terminal setup and teardown.
//!
//! `ratatui::init()` installs a panic hook that restores the terminal before
//! unwinding, so it must run *after* `color_eyre::install()` (done in `main`).
//! That ordering means a panic restores the terminal first, then color-eyre
//! prints its report to a clean screen. Every successful [`init`] must be
//! paired with a [`restore`] on the way out.
//!
//! Mouse capture is enabled so the UI can wheel-scroll panes, focus the pane you
//! scroll or click, and drag the divider between the tree and the preview. The
//! tradeoff: while capture is on, the terminal's native click-drag text
//! selection is intercepted — use the in-app yank (OSC 52) or the
//! release-capture toggle ([`set_mouse_capture`]) to copy with the mouse.

use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::DefaultTerminal;

/// Whether we successfully pushed the keyboard-enhancement flags (so they must
/// be popped on teardown / suspension). Set once in [`init`].
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Whether mouse capture is currently on (so we know to disable it on teardown /
/// suspension and to report the state to the release-capture toggle).
static MOUSE_CAPTURED: AtomicBool = AtomicBool::new(false);

/// Enter the alternate screen + raw mode, returning the terminal.
///
/// Also asks the terminal — when it supports the kitty keyboard protocol — to
/// disambiguate key events, so `Shift+Enter` arrives distinct from `Enter`, and
/// enables mouse capture for wheel scrolling, click-to-focus, and the divider
/// drag.
pub fn init() -> std::io::Result<DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    push_keyboard_enhancement();
    let _ = set_mouse_capture(true);
    Ok(terminal)
}

/// Delete any Kitty graphics placement this process left on the terminal.
///
/// Both teardown paths need this and neither has placement state left to
/// consult, so it clears everything. Unlike the per-frame path — which only
/// transmits from a rect the Kitty branch reserved — these run blind, so the
/// protocol check happens here: a terminal that never spoke Kitty is not sent
/// an APC it would only have to swallow. A no-op without `raster`, which
/// compiles the image path out entirely.
fn clear_terminal_graphics() {
    #[cfg(feature = "raster")]
    {
        use crate::ui::fileview;

        // `clear_kitty_images` flushes what it writes.
        let _ = fileview::clear_kitty_images(fileview::graphics_protocol(), &mut std::io::stdout());
    }
}

/// Leave raw mode + the alternate screen.
///
/// Kitty graphics placements are not part of the alternate screen's cell buffer,
/// so any image the file view transmitted has to be deleted explicitly or it
/// survives on the shell the user comes back to.
pub fn restore() {
    clear_terminal_graphics();
    let _ = set_mouse_capture(false);
    pop_keyboard_enhancement();
    ratatui::restore();
}

/// Turn mouse capture on or off, recording the state. Used at startup/teardown
/// and by the release-capture toggle so the user can fall back to the
/// terminal's native text selection.
pub fn set_mouse_capture(on: bool) -> std::io::Result<()> {
    use std::io::Write;

    let mut out = std::io::stdout();
    out.write_all(capture_sequence(on))?;
    out.flush()?;
    MOUSE_CAPTURED.store(on, Ordering::Relaxed);
    Ok(())
}

/// The escape sequence that turns mouse reporting on or off.
///
/// Button + wheel reporting (mode 1000) and button-event tracking (1002), with
/// SGR coordinates (1006). 1002 reports motion *only while a button is held*,
/// which is exactly what the divider drag needs and costs nothing while the
/// pointer is idle. We still deliberately skip 1003 (any-motion): it streams an
/// event for every pixel of mouse movement, and that flood is what made the TUI
/// feel unresponsive. crossterm's `EnableMouseCapture` turns 1003 on too, so we
/// write the modes we want directly instead.
fn capture_sequence(on: bool) -> &'static [u8] {
    if on {
        b"\x1b[?1000h\x1b[?1002h\x1b[?1006h"
    } else {
        b"\x1b[?1006l\x1b[?1002l\x1b[?1000l"
    }
}

/// Whether mouse capture is currently on.
pub fn mouse_captured() -> bool {
    MOUSE_CAPTURED.load(Ordering::Relaxed)
}

/// Run `f` with the terminal handed back to the OS (cooked mode, normal
/// screen), then re-enter raw mode + the alternate screen and force a full
/// repaint. Used to suspend the TUI while an external program (e.g. `$EDITOR`
/// or `$PAGER`) takes over the terminal.
///
/// Toggles crossterm modes on the *same* terminal rather than going through
/// [`restore`] + [`init`]: re-`init` reinstalls the terminal-restoring panic
/// hook each time, which would chain (leak) hooks across a session of opens.
/// The keyboard-enhancement flags are popped before the handoff and re-pushed
/// after, so the external program sees an ordinary terminal.
pub fn suspended<T>(terminal: &mut DefaultTerminal, f: impl FnOnce() -> T) -> std::io::Result<T> {
    use crossterm::execute;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };

    // Delete any terminal-graphics image before handing the screen over: a
    // placement is not part of the cell buffer, so it can outlive the alternate
    // screen and land on top of the program we are about to run.
    clear_terminal_graphics();
    pop_keyboard_enhancement();
    // Hand the mouse back so the external program (and its native selection)
    // behaves normally; re-grab on return only if we had it.
    let had_mouse = mouse_captured();
    if had_mouse {
        let _ = set_mouse_capture(false);
    }
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    let result = f();
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    repush_keyboard_enhancement();
    if had_mouse {
        let _ = set_mouse_capture(true);
    }
    terminal.clear()?; // resync ratatui's buffer with the freshly cleared screen
    Ok(result)
}

/// Push the disambiguation flag if the terminal supports it, recording success
/// so it is popped on the way out. A no-op on terminals without support, which
/// then simply lose `Shift+Enter` (the `e` key still edits).
fn push_keyboard_enhancement() {
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    use crossterm::execute;
    use crossterm::terminal::supports_keyboard_enhancement;

    if matches!(supports_keyboard_enhancement(), Ok(true))
        && execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }
}

/// Re-push the flag after a suspension, without re-querying the terminal.
fn repush_keyboard_enhancement() {
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    use crossterm::execute;

    if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
}

/// Pop the flag we pushed, if any.
fn pop_keyboard_enhancement() {
    use crossterm::event::PopKeyboardEnhancementFlags;
    use crossterm::execute;

    if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The divider drag lives or dies by mode 1002: without it the terminal
    /// reports presses but no held motion, and the seam simply never moves.
    /// Mode 1003 must stay out — it reports *every* pointer movement, which is
    /// the flood this module's doc warns about.
    #[test]
    fn mouse_capture_tracks_held_motion_but_not_idle_motion() {
        let on = capture_sequence(true);
        for mode in [&b"?1000h"[..], b"?1002h", b"?1006h"] {
            assert!(
                on.windows(mode.len()).any(|w| w == mode),
                "enable sequence is missing {:?}",
                std::str::from_utf8(mode).unwrap()
            );
        }
        assert!(
            !on.windows(6).any(|w| w == b"?1003h"),
            "any-motion tracking must stay off"
        );

        // Every mode turned on is turned back off, or it outlives the TUI.
        let off = capture_sequence(false);
        for mode in [&b"1000"[..], b"1002", b"1006"] {
            let disabled: Vec<u8> = mode.iter().copied().chain(*b"l").collect();
            assert!(
                off.windows(disabled.len()).any(|w| w == disabled),
                "disable sequence is missing {:?}",
                std::str::from_utf8(mode).unwrap()
            );
        }
    }
}
