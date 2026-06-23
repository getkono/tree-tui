//! Handing the terminal to an external program (editor or pager).
//!
//! Both `editor` and `pager` resolve a command spec from the environment and run
//! it against the selected file. The shared mechanics — splitting the spec on
//! whitespace so simple flags work (e.g. `code -w`, `less -R`), spawning the
//! process, and reporting failures via `tracing` — live here. The caller has
//! already suspended the TUI, so the (currently torn-down) terminal is not used
//! for diagnostics.

use std::path::Path;
use std::process::Command;

/// Run `spec` against `path`, blocking until it exits. `kind` ("editor" or
/// "pager") only labels the log lines.
pub fn run(kind: &'static str, spec: &str, path: &Path) {
    let Some((program, args)) = split_command(spec) else {
        tracing::warn!(kind, "open command is empty; cannot open a file");
        return;
    };
    match Command::new(program).args(args).arg(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::warn!(kind, command = %spec, %status, "external program exited non-zero")
        }
        Err(err) => {
            tracing::warn!(kind, command = %spec, %err, "failed to launch external program")
        }
    }
}

/// Split a command spec into its program and leading arguments, or `None` when
/// the spec is blank.
fn split_command(spec: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = spec.split_whitespace();
    let program = parts.next()?;
    Some((program, parts.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_bare_program() {
        assert_eq!(split_command("vi"), Some(("vi", vec![])));
        assert_eq!(split_command("less"), Some(("less", vec![])));
    }

    #[test]
    fn splits_a_program_with_flags() {
        assert_eq!(split_command("code -w"), Some(("code", vec!["-w"])));
        assert_eq!(split_command("less -R"), Some(("less", vec!["-R"])));
        assert_eq!(
            split_command("emacsclient -t -a "),
            Some(("emacsclient", vec!["-t", "-a"]))
        );
    }

    #[test]
    fn blank_spec_is_none() {
        assert_eq!(split_command(""), None);
        assert_eq!(split_command("   "), None);
    }
}
