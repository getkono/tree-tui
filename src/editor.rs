//! Opening the selected file in the user's editor (to edit it).

use std::path::Path;

use crate::launch;

/// Open `path` in the user's editor, blocking until it exits.
///
/// The command is `$VISUAL`, then `$EDITOR`, falling back to `vi`. The value is
/// split on whitespace so simple flags work (e.g. `code -w`, `emacsclient -t`).
/// The caller has already suspended the TUI.
pub fn open(path: &Path) {
    let spec = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    launch::run("editor", &spec, path);
}
