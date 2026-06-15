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
