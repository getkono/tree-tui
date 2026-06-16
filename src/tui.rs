//! Terminal setup and teardown.
//!
//! `ratatui::init()` installs a panic hook that restores the terminal before
//! unwinding, so it must run *after* `color_eyre::install()` (done in `main`).
//! That ordering means a panic restores the terminal first, then color-eyre
//! prints its report to a clean screen. Every successful [`init`] must be
//! paired with a [`restore`] on the way out.

use ratatui::DefaultTerminal;

/// Enter the alternate screen + raw mode, returning the terminal.
pub fn init() -> std::io::Result<DefaultTerminal> {
    ratatui::try_init()
}

/// Leave raw mode + the alternate screen.
pub fn restore() {
    ratatui::restore();
}

/// Run `f` with the terminal handed back to the OS (cooked mode, normal
/// screen), then re-enter raw mode + the alternate screen and force a full
/// repaint. Used to suspend the TUI while an external program (e.g. `$EDITOR`)
/// takes over the terminal.
///
/// Toggles crossterm modes on the *same* terminal rather than going through
/// [`restore`] + [`init`]: re-`init` reinstalls the terminal-restoring panic
/// hook each time, which would chain (leak) hooks across a session of edits.
pub fn suspended<T>(terminal: &mut DefaultTerminal, f: impl FnOnce() -> T) -> std::io::Result<T> {
    use crossterm::execute;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    let result = f();
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?; // resync ratatui's buffer with the freshly cleared screen
    Ok(result)
}
