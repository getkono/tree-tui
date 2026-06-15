//! Run tokei off the async runtime and deliver a built [`Tree`].
//!
//! tokei is synchronous and CPU-bound, so it runs on a blocking thread; the
//! UI loop awaits the result over a oneshot channel and animates a spinner
//! meanwhile.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokei::{Config, Languages};
use tokio::sync::oneshot;

use crate::model::{Tree, build_tree};

/// The result of a completed scan.
pub struct ScanOutcome {
    pub tree: Tree,
    pub duration: Duration,
    /// Whether any language reported a parsing ambiguity.
    pub inaccurate: bool,
}

/// Scan `root` on a blocking thread, returning a receiver for the outcome.
pub fn spawn(root: PathBuf, root_label: String) -> oneshot::Receiver<ScanOutcome> {
    let (tx, rx) = oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut languages = Languages::new();
        // tokei already honors .gitignore/.ignore and walks in parallel.
        let ignored: &[&str] = &[];
        languages.get_statistics(&[&root], ignored, &Config::default());

        let inaccurate = languages.values().any(|language| language.inaccurate);
        let tree = build_tree(&languages, &root, root_label);

        // The receiver is dropped if the user quits mid-scan; ignore that.
        let _ = tx.send(ScanOutcome {
            tree,
            duration: started.elapsed(),
            inaccurate,
        });
    });
    rx
}
