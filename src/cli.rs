//! Strict command-line parsing: `tree [dir]` plus `-V/--version` and `-h/--help`.
//!
//! The grammar is intentionally tight — at most one positional directory
//! (defaulting to `.`), no unknown flags, no extra positionals. Anything else is
//! a [`CliError`] that `main` renders to stderr alongside [`usage`] with a
//! non-zero exit code. Parsing is pure (no filesystem access) so it is trivially
//! unit-testable; the directory is validated separately by `main`.

use std::path::PathBuf;

/// The user-facing binary name, used in usage text and the `-V` report.
pub const BIN_NAME: &str = "tree";

/// A fully parsed invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Render the interactive directory visualizer for `dir`.
    Run { dir: PathBuf },
    /// Print the version / build report and exit.
    Version,
    /// Print usage and exit.
    Help,
}

/// A command-line parsing failure.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CliError {
    /// More than one positional argument was supplied.
    #[error("unexpected extra argument: {0:?}")]
    ExtraArg(String),
    /// An unrecognized flag was supplied.
    #[error("unknown flag: {0}")]
    UnknownFlag(String),
}

/// Parse arguments (everything after `argv[0]`).
///
/// `-V`/`-h` are terminal: they win as soon as they are seen, regardless of
/// surrounding positionals. The directory is optional and defaults to `.`, so
/// bare `tree` is equivalent to `tree .`.
pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut dir: Option<String> = None;
    for arg in args {
        let arg = arg.into();
        match arg.as_str() {
            "-V" | "--version" => return Ok(Command::Version),
            "-h" | "--help" => return Ok(Command::Help),
            s if s.starts_with('-') && s != "-" => return Err(CliError::UnknownFlag(arg)),
            _ if dir.is_some() => return Err(CliError::ExtraArg(arg)),
            _ => dir = Some(arg),
        }
    }
    Ok(Command::Run {
        dir: PathBuf::from(dir.unwrap_or_else(|| ".".to_string())),
    })
}

/// The multi-line usage string.
pub fn usage() -> String {
    format!(
        "{BIN_NAME} — interactive directory visualizer (code, size, git)\n\
         \n\
         usage:\n  \
           {BIN_NAME} [dir]           explore [dir] (default: .) through swappable lenses\n  \
           {BIN_NAME} -V, --version   print version and build info\n  \
           {BIN_NAME} -h, --help      print this help"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_a_directory() {
        assert_eq!(
            parse_args(&["src"]).unwrap(),
            Command::Run { dir: "src".into() }
        );
    }

    #[test]
    fn version_flags() {
        assert_eq!(parse_args(&["-V"]).unwrap(), Command::Version);
        assert_eq!(parse_args(&["--version"]).unwrap(), Command::Version);
    }

    #[test]
    fn help_flags() {
        assert_eq!(parse_args(&["-h"]).unwrap(), Command::Help);
        assert_eq!(parse_args(&["--help"]).unwrap(), Command::Help);
    }

    #[test]
    fn terminal_flag_wins_over_positional() {
        assert_eq!(parse_args(&["src", "-V"]).unwrap(), Command::Version);
    }

    #[test]
    fn missing_dir_defaults_to_cwd() {
        assert_eq!(parse_args(&[]).unwrap(), Command::Run { dir: ".".into() });
    }

    #[test]
    fn extra_positional_is_an_error() {
        assert_eq!(
            parse_args(&["a", "b"]),
            Err(CliError::ExtraArg("b".to_string()))
        );
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert_eq!(
            parse_args(&["--nope"]),
            Err(CliError::UnknownFlag("--nope".to_string()))
        );
    }
}
