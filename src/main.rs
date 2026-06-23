//! tree — an interactive directory visualizer for large polyglot repos.
//!
//! Invoked as `tree [dir]` (defaulting to `.`). The directory is walked once into a navigable,
//! sortable tree, then viewed through swappable "lenses" (code lines, on-disk
//! size, git churn, git status). Each lens's data is computed lazily — the first
//! time it is opened — and cached for the session.

mod action;
mod app;
mod cli;
mod collect;
mod editor;
mod error;
mod event;
mod launch;
mod logging;
mod model;
mod pager;
mod scan;
mod tui;
mod ui;
mod version;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use app::App;
use cli::Command;
use error::AppError;

#[tokio::main]
async fn main() -> ExitCode {
    // Install color-eyre first; the terminal-restoring panic hook from
    // `ratatui::init()` must be installed *after* it (see `tui`).
    if let Err(err) = color_eyre::install() {
        eprintln!("{}: failed to install error handler: {err}", cli::BIN_NAME);
        return ExitCode::FAILURE;
    }

    match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Version) => {
            println!("{}", version::long_version());
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            println!("{}", cli::usage());
            ExitCode::SUCCESS
        }
        Ok(Command::Run { dir }) => {
            let _log_guard = logging::init();
            match run(dir).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    // Concise, chained message for expected errors; the
                    // color-eyre panic hook still gives rich reports for bugs.
                    eprintln!("{}: {err:#}", cli::BIN_NAME);
                    ExitCode::FAILURE
                }
            }
        }
        Err(err) => {
            eprintln!("{}: {err}\n\n{}", cli::BIN_NAME, cli::usage());
            // Conventional "usage error" exit code.
            ExitCode::from(2)
        }
    }
}

/// Validate the target directory, then run the TUI, restoring the terminal on
/// every exit path.
async fn run(dir: PathBuf) -> color_eyre::Result<()> {
    let root = validate_dir(&dir)?;
    let label = root_label(&dir, &root);
    tracing::info!(target = %root.display(), "tree starting");

    let mut app = App::new(root, label);
    let mut terminal = tui::init()?;
    let result = event::run(&mut terminal, &mut app).await;
    tui::restore();
    result
}

/// Ensure `dir` exists and is a directory, returning its canonical path.
fn validate_dir(dir: &Path) -> color_eyre::Result<PathBuf> {
    if !dir.exists() {
        return Err(AppError::NotFound(dir.to_path_buf()).into());
    }
    if !dir.is_dir() {
        return Err(AppError::NotADirectory(dir.to_path_buf()).into());
    }
    Ok(std::fs::canonicalize(dir)?)
}

/// A friendly label for the tree root: the user's argument as given, except for
/// `.`/`./`, where the canonical directory name reads better.
fn root_label(original: &Path, canonical: &Path) -> String {
    let shown = original.to_string_lossy();
    if matches!(shown.as_ref(), "." | "./" | "") {
        canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| shown.into_owned())
    } else {
        shown.into_owned()
    }
}
