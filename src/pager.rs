//! Opening the selected file in the user's pager (to preview it).

use std::path::Path;

use crate::launch;

/// Open `path` in the user's pager, blocking until it exits.
///
/// The command is `$PAGER`, falling back to `less`. The value is split on
/// whitespace so simple flags work (e.g. `less -R`, `bat --paging=always`).
/// The caller has already suspended the TUI.
pub fn open(path: &Path) {
    let spec = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    launch::run("pager", &spec, path);
}
