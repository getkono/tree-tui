//! Walk the directory off the async runtime and deliver a built skeleton [`Tree`].
//!
//! The walk is cheap (structure + sizes only, no file contents), so it runs on a
//! blocking thread and the UI shows a brief spinner. Expensive per-lens metrics
//! are computed later, lazily, when a lens is first opened (see `collect` and the
//! event loop).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::collect;
use crate::model::{Tree, build_skeleton};

/// The result of the initial directory walk.
pub struct ScanOutcome {
    pub tree: Tree,
    pub duration: Duration,
    /// Whether the root is inside a git repository (gates the git lenses).
    pub repo: bool,
}

/// Walk `root` on a blocking thread, returning a receiver for the skeleton.
pub fn spawn(root: PathBuf, root_label: String) -> oneshot::Receiver<ScanOutcome> {
    let (tx, rx) = oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let result = collect::walk(&root);
        let tree = build_skeleton(&result.files, &result.dirs, root_label);
        let repo = collect::is_repo(&root);

        // The receiver is dropped if the user quits mid-scan; ignore that.
        let _ = tx.send(ScanOutcome {
            tree,
            duration: started.elapsed(),
            repo,
        });
    });
    rx
}
