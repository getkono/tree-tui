//! Strict command-line parsing: `tree [dir]` plus `--icons`, `-V/--version`, and
//! `-h/--help`.
//!
//! The grammar is intentionally tight — at most one positional directory
//! (defaulting to `.`), no unknown flags, no extra positionals. Anything else is
//! a [`CliError`] that `main` renders to stderr alongside [`usage`] with a
//! non-zero exit code. Parsing is pure (no filesystem or environment access) so
//! it is trivially unit-testable: `--icons` parses to an `Option`, and
//! [`resolve_icons`] folds in the environment separately. The directory is
//! validated by `main`.

use std::path::PathBuf;

use karet_filetype::IconStyle;

/// Environment variable consulted when `--icons` is absent.
pub const ICONS_ENV: &str = "TREE_TUI_ICONS";

/// The glyph tier used when neither `--icons` nor [`ICONS_ENV`] picks one.
///
/// Unicode rather than karet's own `IconStyle::default()` (Nerd Font): the
/// richer tier needs a patched font and renders as tofu without one, and the
/// default is what a first run lands on. `--icons nerd` is one flag away.
pub const DEFAULT_ICONS: IconStyle = IconStyle::Unicode;

/// The user-facing binary name, used in usage text and the `-V` report.
pub const BIN_NAME: &str = "tree";

/// A fully parsed invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Render the interactive directory visualizer for `dir`.
    Run {
        dir: PathBuf,
        /// The glyph tier from `--icons`, or `None` to fall back (see
        /// [`resolve_icons`]).
        icons: Option<IconStyle>,
    },
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
    /// A flag that takes a value was supplied without one.
    #[error("{0} needs a value")]
    MissingValue(&'static str),
    /// A flag's value was not one this flag accepts.
    #[error("invalid value for {flag}: {value:?} (expected nerd, unicode, or ascii)")]
    BadValue { flag: &'static str, value: String },
}

/// Resolve the icon tier: the `--icons` flag wins, then `TREE_TUI_ICONS`, then
/// [`DEFAULT_ICONS`].
///
/// An unrecognized environment value is ignored rather than fatal — a stale
/// shell export should not stop the tool from starting, whereas a mistyped flag
/// (rejected in [`parse`]) should be reported.
#[must_use]
pub fn resolve_icons(flag: Option<IconStyle>, env: Option<&str>) -> IconStyle {
    flag.or_else(|| env.and_then(IconStyle::from_name))
        .unwrap_or(DEFAULT_ICONS)
}

/// Parse arguments (everything after `argv[0]`).
///
/// `-V`/`-h` are terminal: they win as soon as they are seen, regardless of
/// surrounding positionals. The one exception is the position right after
/// `--icons`, which is read as that flag's value (so `tree --icons -h` reports a
/// bad value rather than printing help). The directory is optional and defaults
/// to `.`, so bare `tree` is equivalent to `tree .`.
pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut dir: Option<String> = None;
    let mut icons: Option<IconStyle> = None;
    let mut args = args.into_iter().map(Into::into);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-V" | "--version" => return Ok(Command::Version),
            "-h" | "--help" => return Ok(Command::Help),
            // `--icons nerd` and `--icons=nerd` are both accepted.
            "--icons" => {
                let value = args.next().ok_or(CliError::MissingValue("--icons"))?;
                icons = Some(icon_style(&value)?);
            }
            s if s.starts_with("--icons=") => {
                icons = Some(icon_style(&s["--icons=".len()..])?);
            }
            s if s.starts_with('-') && s != "-" => return Err(CliError::UnknownFlag(arg)),
            _ if dir.is_some() => return Err(CliError::ExtraArg(arg)),
            _ => dir = Some(arg),
        }
    }
    Ok(Command::Run {
        dir: PathBuf::from(dir.unwrap_or_else(|| ".".to_string())),
        icons,
    })
}

/// Parse an `--icons` value, reporting the accepted names on a miss.
fn icon_style(value: &str) -> Result<IconStyle, CliError> {
    IconStyle::from_name(value).ok_or_else(|| CliError::BadValue {
        flag: "--icons",
        value: value.to_string(),
    })
}

/// The multi-line usage string.
pub fn usage() -> String {
    format!(
        "{BIN_NAME} — interactive directory visualizer (code, size, git)\n\
         \n\
         usage:\n  \
           {BIN_NAME} [dir]           explore [dir] (default: .) through swappable lenses\n  \
           {BIN_NAME} --icons <tier>  glyph tier: unicode (default), nerd, or ascii\n  \
           {BIN_NAME} -V, --version   print version and build info\n  \
           {BIN_NAME} -h, --help      print this help\n\
         \n\
         {BIN_NAME} reads {ICONS_ENV} when --icons is absent."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    /// `Command::Run` for `dir` with no `--icons`.
    fn run(dir: &str) -> Command {
        Command::Run {
            dir: dir.into(),
            icons: None,
        }
    }

    #[test]
    fn parses_a_directory() {
        assert_eq!(parse_args(&["src"]).unwrap(), run("src"));
    }

    #[test]
    fn icons_flag_takes_a_value_either_way() {
        for args in [&["--icons", "ascii"][..], &["--icons=ascii"][..]] {
            assert_eq!(
                parse_args(args).unwrap(),
                Command::Run {
                    dir: ".".into(),
                    icons: Some(IconStyle::Ascii),
                }
            );
        }
    }

    #[test]
    fn icons_rejects_a_missing_or_unknown_value() {
        assert_eq!(
            parse_args(&["--icons"]),
            Err(CliError::MissingValue("--icons"))
        );
        assert_eq!(
            parse_args(&["--icons", "emoji"]),
            Err(CliError::BadValue {
                flag: "--icons",
                value: "emoji".to_string(),
            })
        );
    }

    #[test]
    fn icons_resolve_flag_then_env_then_default() {
        assert_eq!(
            resolve_icons(Some(IconStyle::Ascii), Some("unicode")),
            IconStyle::Ascii
        );
        assert_eq!(resolve_icons(None, Some("unicode")), IconStyle::Unicode);
        // The default is ours, not karet's `IconStyle::default()` — the rich
        // tier is tofu without a patched font.
        assert_eq!(resolve_icons(None, None), IconStyle::Unicode);
        // ...but the environment still reaches it.
        assert_eq!(resolve_icons(None, Some("nerd")), IconStyle::NerdFont);
        // A stale export must not stop the tool from starting.
        assert_eq!(resolve_icons(None, Some("bogus")), IconStyle::Unicode);
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
        assert_eq!(parse_args(&[]).unwrap(), run("."));
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
