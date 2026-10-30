//! Filesystem watching: debounced "something changed" signals for the root.
//!
//! A recursive watcher coalesces bursts of filesystem events into a single
//! signal over a tokio channel; the event loop reacts by re-walking the tree
//! (see `App::on_rescan`). Watcher creation is best-effort: if a watcher can't
//! be created (e.g. the inotify limit is reached), [`spawn`] returns `None` and
//! the app simply runs without live refresh.
//!
//! Only events that can change what is on disk count (see [`is_change`]).
//! notify's inotify backend reports *opens* too (`IN_OPEN`, as an access
//! event), and the re-walk itself opens every directory it lists — so counting
//! those would make each re-walk trigger the next, forever, on an idle tree.

use std::path::Path;
use std::time::Duration;

use notify_debouncer_full::notify::event::{AccessKind, AccessMode, EventKind};
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

/// Whether an event can have changed the tree or a file's contents.
///
/// Reads are not changes: an access event counts only when it closes a file
/// that was open for writing. Everything else — create, modify, remove,
/// rename, and the unclassified `Any`/`Other` — counts, so an event notify
/// can't place errs toward a refresh rather than a missed change.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(access) => matches!(access, AccessKind::Close(AccessMode::Write)),
        _ => true,
    }
}

/// Start watching `root` recursively, returning `None` (and logging) if a
/// watcher can't be created.
pub fn spawn(root: &Path) -> Option<Watch> {
    let (tx, rx) = mpsc::unbounded_channel();

    // A batch holding at least one real change becomes a single "changed"
    // signal; the event loop re-walks and decides whether anything visible
    // changed. Read-only batches and errored batches are skipped — the next
    // real change retries. A closed receiver (app shutting down) makes the send
    // a harmless no-op.
    let handler = move |result: DebounceEventResult| {
        if let Ok(events) = result
            && events.iter().any(|event| is_change(&event.kind))
        {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use notify_debouncer_full::notify::event::{
        CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };

    use super::*;

    #[test]
    fn reads_are_not_changes() {
        for access in [
            AccessKind::Open(AccessMode::Read),
            AccessKind::Open(AccessMode::Any),
            AccessKind::Close(AccessMode::Read),
            AccessKind::Read,
            AccessKind::Any,
        ] {
            assert!(!is_change(&EventKind::Access(access)), "{access:?}");
        }
    }

    #[test]
    fn writes_and_structure_changes_are_changes() {
        for kind in [
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)),
            EventKind::Remove(RemoveKind::Any),
            EventKind::Any,
            EventKind::Other,
        ] {
            assert!(is_change(&kind), "{kind:?}");
        }
    }

    /// A real directory under the system temp dir, removed on drop.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tree-tui-watch-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("sub")).expect("create the temp root");
            std::fs::write(dir.join("sub/file.txt"), "hello").expect("write a fixture file");
            Self(dir)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The feedback loop itself: listing and reading the tree — what a re-walk
    /// and a preview refresh do — must not signal, while a write still must.
    #[tokio::test]
    async fn reading_the_tree_does_not_signal_but_writing_does() {
        let root = TempRoot::new("reads");
        let mut watch = spawn(&root.0).expect("create a watcher");

        for entry in std::fs::read_dir(&root.0).unwrap() {
            let _ = entry.unwrap();
        }
        let _ = std::fs::read_dir(root.0.join("sub")).unwrap().count();
        let _ = std::fs::read_to_string(root.0.join("sub/file.txt")).unwrap();
        let read = tokio::time::timeout(Duration::from_secs(1), watch.rx.recv()).await;
        assert!(read.is_err(), "a read-only pass signalled a change");

        std::fs::write(root.0.join("sub/new.txt"), "x").unwrap();
        let wrote = tokio::time::timeout(Duration::from_secs(3), watch.rx.recv()).await;
        assert_eq!(wrote, Ok(Some(())), "a write did not signal a change");
    }
}
