//! Terminal setup and teardown.
//!
//! `ratatui::init()` installs a panic hook that restores the terminal before
//! unwinding, so it must run *after* `color_eyre::install()` (done in `main`).
//! That ordering means a panic restores the terminal first, then color-eyre
//! prints its report to a clean screen. Every successful [`init`] must be
//! paired with a [`restore`] on the way out.
//!
//! Mouse capture is enabled so the UI can wheel-scroll panes and follow the
//! cursor for focus. The tradeoff: while capture is on, the terminal's native
//! click-drag text selection is intercepted — use the in-app yank (OSC 52) or
//! the release-capture toggle ([`set_mouse_capture`]) to copy with the mouse.

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
/// enables mouse capture for wheel scrolling and focus-follows-mouse.
pub fn init() -> std::io::Result<DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    push_keyboard_enhancement();
    let _ = set_mouse_capture(true);
    Ok(terminal)
}

/// Leave raw mode + the alternate screen.
pub fn restore() {
    let _ = set_mouse_capture(false);
    pop_keyboard_enhancement();
    ratatui::restore();
}

/// Turn mouse capture on or off, recording the state. Used at startup/teardown
/// and by the release-capture toggle so the user can fall back to the
/// terminal's native text selection.
pub fn set_mouse_capture(on: bool) -> std::io::Result<()> {
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    use crossterm::execute;

    if on {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    } else {
        execute!(std::io::stdout(), DisableMouseCapture)?;
    }
    MOUSE_CAPTURED.store(on, Ordering::Relaxed);
    Ok(())
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
