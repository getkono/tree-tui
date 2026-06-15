//! The `-V` / `--version` report, assembled from build-time metadata.
//!
//! The `TREE_*` values are injected by `build.rs`; `CARGO_PKG_VERSION` is the
//! semver from `Cargo.toml`.

use crate::cli::BIN_NAME;

/// The multi-line version / build report shown by `tree -V`.
pub fn long_version() -> String {
    let dirty = if env!("TREE_GIT_DIRTY") == "true" {
        " (dirty)"
    } else {
        ""
    };
    format!(
        "{name} {version}\n\
         commit:   {sha}{dirty}\n\
         built:    {built}\n\
         profile:  {profile}\n\
         rustc:    {rustc} · {channel}\n\
         target:   {target}",
        name = BIN_NAME,
        version = env!("CARGO_PKG_VERSION"),
        sha = env!("TREE_GIT_SHA"),
        built = env!("TREE_BUILD_TIME"),
        profile = env!("TREE_BUILD_PROFILE"),
        rustc = env!("TREE_RUSTC"),
        channel = env!("TREE_RUST_CHANNEL"),
        target = env!("TREE_BUILD_TARGET"),
    )
}
