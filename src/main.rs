//! tree — an interactive directory visualizer for large polyglot repos.
//!
//! Invoked as `tree [dir]` (defaulting to `.`). Everything lives in the library
//! half of the crate (`src/lib.rs`), which is what the tests and benchmarks link
//! against; this file is only the async entry point.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    tree_tui::run_cli().await
}
