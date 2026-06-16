//! File-based logging via `tracing-appender`.
//!
//! The TUI owns stdout, so diagnostics never go to the terminal. Logging is
//! **off by default**: it is enabled only when `TREE_LOG` (a target file
//! path) or `RUST_LOG` (a filter) is set, so a normal run leaves no stray
//! files behind. The returned guard must be kept alive for the process so the
//! non-blocking writer flushes on exit.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Initialize file logging if requested via the environment.
///
/// Returns `Some(guard)` when a subscriber was installed, `None` when logging
/// is disabled.
pub fn init() -> Option<WorkerGuard> {
    let log_path = std::env::var_os("TREE_LOG").map(PathBuf::from);
    if log_path.is_none() && std::env::var_os("RUST_LOG").is_none() {
        return None;
    }

    let path = log_path.unwrap_or_else(|| PathBuf::from("tree.log"));
    let file_name = path.file_name()?.to_owned();
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };

    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(dir, file_name));
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();

    Some(guard)
}
