//! Git collectors (pure-Rust via `gix`): churn over recent history and
//! working-tree status, each keyed by path relative to the scanned root.
//!
//! Everything degrades gracefully: a missing/unreadable repository, an unborn
//! HEAD, or any gix error yields an empty map rather than failing the lens. All
//! work runs on a blocking thread (see the event loop).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gix::bstr::BStr;

use crate::model::{ChurnData, StatusData};

/// How many commits of history churn inspects, newest first. Bounds the cost on
/// repositories with very long histories.
const CHURN_MAX_COMMITS: usize = 5000;

/// Whether `root` is inside a git repository.
pub fn is_repo(root: &Path) -> bool {
    gix::discover(root).is_ok()
}

/// The short (7-hex) hash of the repository's current `HEAD` commit, or `None`
/// when `root` is not in a git repository or `HEAD` is unborn.
pub fn head_short_hash(root: &Path) -> Option<String> {
    let repo = gix::discover(root).ok()?;
    let head = repo.head_id().ok()?;
    Some(head.to_hex_with_len(7).to_string())
}

/// Per-file churn (lines added/deleted and commit touches) over the most recent
/// [`CHURN_MAX_COMMITS`] commits reachable from HEAD.
///
/// Note: summed up the tree a directory's `commits` double-counts a commit that
/// touched several of its files — it is a churn weight, not a commit count.
pub fn churn(scan_root: &Path) -> HashMap<PathBuf, ChurnData> {
    let mut map = HashMap::new();
    let Ok(repo) = gix::discover(scan_root) else {
        return map;
    };
    let prefix = repo_prefix(&repo, scan_root);

    let Ok(head_id) = repo.head_id() else {
        return map; // unborn HEAD / empty repo
    };
    let Ok(walk) = repo.rev_walk(Some(head_id.detach())).all() else {
        return map;
    };
    let Ok(mut cache) = repo.diff_resource_cache_for_tree_diff() else {
        return map;
    };

    for info in walk.filter_map(Result::ok).take(CHURN_MAX_COMMITS) {
        let Ok(commit) = repo.find_commit(info.id) else {
            continue;
        };
        let Ok(tree) = commit.tree() else {
            continue;
        };
        let parent_tree = match commit.parent_ids().next() {
            Some(parent) => {
                let Ok(parent_commit) = repo.find_commit(parent.detach()) else {
                    continue;
                };
                let Ok(parent_tree) = parent_commit.tree() else {
                    continue;
                };
                parent_tree
            }
            None => repo.empty_tree(),
        };

        let Ok(mut platform) = parent_tree.changes() else {
            continue;
        };
        let _ = platform.for_each_to_obtain_tree(&tree, |change| {
            if let Some(counts) = change
                .diff(&mut cache)
                .ok()
                .and_then(|mut p| p.line_counts().ok())
                .flatten()
                && let Some(rel) = tree_rel(change.location(), &prefix)
            {
                let entry: &mut ChurnData = map.entry(rel).or_default();
                entry.added += u64::from(counts.insertions);
                entry.deleted += u64::from(counts.removals);
                entry.commits += 1;
            }
            cache.clear_resource_cache_keep_allocation();
            Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Continue(()))
        });
    }

    map
}

/// Per-file working-tree status counts (added/modified/deleted), including
/// untracked files.
pub fn status(scan_root: &Path) -> HashMap<PathBuf, StatusData> {
    use gix::status::index_worktree::iter::Summary;

    let mut map = HashMap::new();
    let Ok(repo) = gix::discover(scan_root) else {
        return map;
    };
    let prefix = repo_prefix(&repo, scan_root);

    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return map;
    };
    let platform = platform.untracked_files(gix::status::UntrackedFiles::Files);
    let Ok(iter) = platform.into_index_worktree_iter(Vec::<gix::bstr::BString>::new()) else {
        return map;
    };

    for item in iter {
        let Ok(item) = item else {
            continue;
        };
        let Some(summary) = item.summary() else {
            continue;
        };
        let Some(rel) = tree_rel(item.rela_path(), &prefix) else {
            continue;
        };
        let entry: &mut StatusData = map.entry(rel).or_default();
        match summary {
            Summary::Added | Summary::IntentToAdd | Summary::Copied => entry.added += 1,
            Summary::Removed => entry.deleted += 1,
            // Modified / TypeChange / Conflict / Renamed and any future variant
            // count as a modification.
            _ => entry.modified += 1,
        }
    }

    map
}

/// The scan root's path relative to the repository work directory (the prefix to
/// strip from repo-relative git paths). `None` if it can't be determined.
fn repo_prefix(repo: &gix::Repository, scan_root: &Path) -> Option<PathBuf> {
    let workdir = repo.workdir()?.canonicalize().ok()?;
    scan_root.strip_prefix(&workdir).ok().map(Path::to_path_buf)
}

/// Convert a repo-relative git path to a path relative to the scanned root, or
/// `None` if it lies outside the scanned subtree.
fn tree_rel(location: &BStr, prefix: &Option<PathBuf>) -> Option<PathBuf> {
    let path = gix::path::from_bstr(location);
    match prefix {
        Some(p) if !p.as_os_str().is_empty() => path.strip_prefix(p).ok().map(Path::to_path_buf),
        _ => Some(path.into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(s: &str) -> &BStr {
        BStr::new(s.as_bytes())
    }

    #[test]
    fn tree_rel_passes_through_at_repo_root() {
        // No prefix (scan root == repo root): paths are already tree-relative.
        assert_eq!(
            tree_rel(loc("src/main.rs"), &None),
            Some(PathBuf::from("src/main.rs"))
        );
        // An empty prefix behaves the same.
        assert_eq!(
            tree_rel(loc("src/main.rs"), &Some(PathBuf::new())),
            Some(PathBuf::from("src/main.rs"))
        );
    }

    #[test]
    fn tree_rel_strips_the_subdir_prefix() {
        let prefix = Some(PathBuf::from("crate"));
        assert_eq!(
            tree_rel(loc("crate/src/main.rs"), &prefix),
            Some(PathBuf::from("src/main.rs"))
        );
    }

    #[test]
    fn tree_rel_drops_paths_outside_the_scanned_subtree() {
        let prefix = Some(PathBuf::from("crate"));
        assert_eq!(tree_rel(loc("other/file.rs"), &prefix), None);
    }
}
