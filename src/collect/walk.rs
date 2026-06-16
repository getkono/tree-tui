//! The eager filesystem walk: the tree skeleton plus per-file size.
//!
//! Uses the `ignore` crate (the same one tokei walks with), so `.gitignore` /
//! `.ignore` rules, hidden files, and `.git` are handled exactly as a language
//! counter would handle them — keeping the skeleton aligned with the code lens.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::relative_path;

/// Every non-ignored file (relative path + size in bytes) and directory under a
/// root, ready to be turned into a [`Tree`](crate::model::Tree) skeleton.
pub struct WalkResult {
    pub files: Vec<(PathBuf, u64)>,
    pub dirs: Vec<PathBuf>,
}

/// Walk `root`, collecting files (with sizes) and directories.
pub fn walk(root: &Path) -> WalkResult {
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for result in WalkBuilder::new(root).build() {
        let Ok(entry) = result else {
            continue; // unreadable entry: skip rather than fail the whole scan
        };
        let rel = relative_path(entry.path(), root);
        if rel.as_os_str().is_empty() {
            continue; // the root itself is the tree root node
        }
        match entry.file_type() {
            Some(ft) if ft.is_dir() => dirs.push(rel),
            Some(ft) if ft.is_file() => {
                let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((rel, bytes));
            }
            // Symlinks (not followed by default) and special files are ignored.
            _ => {}
        }
    }

    WalkResult { files, dirs }
}
