//! Filesystem watching: debounced "something changed" signals for the root.
//!
//! A recursive watcher coalesces bursts of filesystem events into a single
//! signal over a tokio channel; the event loop reacts by re-walking the tree
//! (see `App::on_rescan`). Watcher creation is best-effort: if a watcher can't
//! be created (e.g. the inotify limit is reached), [`spawn`] returns `None` and
//! the app simply runs without live refresh.

use std::path::Path;
use std::time::Duration;

use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Debounce window: bursts of events within this window collapse to one signal.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// A live filesystem watch. Holds the debouncer alive (dropping it stops
/// watching) alongside the receiver of coalesced change signals.
pub struct Watch {
    pub rx: UnboundedReceiver<()>,
    // Kept alive for the lifetime of the watch; never read directly.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Start watching `root` recursively, returning `None` (and logging) if a
/// watcher can't be created.
pub fn spawn(root: &Path) -> Option<Watch> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Any successful batch becomes a single "changed" signal; the event loop
    // re-walks and decides whether anything actually changed. Errored batches
    // are skipped — the next real change retries. A closed receiver (app
    // shutting down) makes the send a harmless no-op.
    let handler = move |result: DebounceEventResult| {
        if result.is_ok() {
            let _ = tx.send(());
        }
    };

    let mut debouncer = match new_debouncer(DEBOUNCE, None, handler) {
        Ok(debouncer) => debouncer,
        Err(err) => {
            tracing::warn!(%err, "file watcher unavailable; live refresh disabled");
            return None;
        }
    };

    if let Err(err) = debouncer.watch(root, RecursiveMode::Recursive) {
        tracing::warn!(%err, "failed to watch the root; live refresh disabled");
        return None;
    }

    Some(Watch {
        rx,
        _debouncer: debouncer,
    })
}
