//! Opening the selected file in the user's editor (to edit it).

use std::path::Path;

use crate::launch;

/// Open `path` in the user's editor, blocking until it exits.
///
/// The command is `$VISUAL`, then `$EDITOR`, falling back to `vi`. The value is
/// split on whitespace so simple flags work (e.g. `code -w`, `emacsclient -t`).
/// The caller has already suspended the TUI.
pub fn open(path: &Path) {
    launch::run("editor", &resolve_spec(), path);
}

/// Open `path` at `line` in the user's editor. Appends a `+LINE` argument for
/// editors that understand it (vi/vim/nvim/nano/emacs/kak/hx); other editors
/// open at the top. The caller has already suspended the TUI.
pub fn open_at_line(path: &Path, line: usize) {
    let spec = resolve_spec();
    if supports_plus_line(&spec) {
        launch::run_with_extra("editor", &spec, &[format!("+{line}")], path);
    } else {
        launch::run("editor", &spec, path);
    }
}

/// Resolve the editor command spec: `$VISUAL`, then `$EDITOR`, else `vi`.
fn resolve_spec() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string())
}

/// Whether the spec's program understands the `+LINE` open-at-line argument.
fn supports_plus_line(spec: &str) -> bool {
    let program = spec.split_whitespace().next().unwrap_or("");
    let base = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    matches!(
        base,
        "vi" | "vim"
            | "nvim"
            | "gvim"
            | "mvim"
            | "view"
            | "nano"
            | "emacs"
            | "emacsclient"
            | "kak"
            | "hx"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_plus_line_editors() {
        assert!(supports_plus_line("vim"));
        assert!(supports_plus_line("nvim -u NONE"));
        assert!(supports_plus_line("/usr/bin/nano"));
        assert!(supports_plus_line("hx"));
    }

    #[test]
    fn rejects_editors_without_plus_line() {
        assert!(!supports_plus_line("code -w"));
        assert!(!supports_plus_line("subl"));
        assert!(!supports_plus_line(""));
    }
}
