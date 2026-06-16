//! Opening the selected file in the user's editor.

use std::path::Path;
use std::process::Command;

/// Open `path` in the user's editor, blocking until it exits.
///
/// The command is `$VISUAL`, then `$EDITOR`, falling back to `vi`. The value is
/// split on whitespace so simple flags work (e.g. `code -w`, `emacsclient -t`).
/// The caller has already suspended the TUI, so failures are reported via
/// `tracing` rather than the (currently torn-down) terminal.
pub fn open(path: &Path) {
    let spec = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let Some((program, args)) = split_editor(&spec) else {
        tracing::warn!("VISUAL/EDITOR is empty; cannot open a file");
        return;
    };
    match Command::new(program).args(args).arg(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(editor = %spec, %status, "editor exited non-zero"),
        Err(err) => tracing::warn!(editor = %spec, %err, "failed to launch editor"),
    }
}

/// Split an editor spec into its program and leading arguments, or `None` when
/// the spec is blank.
fn split_editor(spec: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = spec.split_whitespace();
    let program = parts.next()?;
    Some((program, parts.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_bare_program() {
        assert_eq!(split_editor("vi"), Some(("vi", vec![])));
    }

    #[test]
    fn splits_a_program_with_flags() {
        assert_eq!(split_editor("code -w"), Some(("code", vec!["-w"])));
        assert_eq!(
            split_editor("emacsclient -t -a "),
            Some(("emacsclient", vec!["-t", "-a"]))
        );
    }

    #[test]
    fn blank_spec_is_none() {
        assert_eq!(split_editor(""), None);
        assert_eq!(split_editor("   "), None);
    }
}
