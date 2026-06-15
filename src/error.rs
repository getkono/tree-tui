//! Typed application errors. Used with `thiserror` inside modules; converted to
//! `color_eyre::Report` at the `main` boundary.

use std::path::PathBuf;

/// Errors surfaced to the user before/around the TUI.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The target path does not exist.
    #[error("path does not exist: {0}")]
    NotFound(PathBuf),
    /// The target path exists but is not a directory.
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
}
