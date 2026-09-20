//! The eager filesystem walk: the tree skeleton plus per-file size.
//!
//! The rule is "what git would show you": every tracked file, plus every
//! untracked file git wouldn't ignore. Concretely, the `ignore` crate's walk
//! with hidden-filtering **off** (a dot-entry is an ordinary file — `.github/`
//! and `.gitignore` are tracked like any other path), unioned with the git
//! index so a tracked-but-gitignored file still gets a node. VCS bookkeeping
//! (`.git`, `.jj`, `.hg`, `.svn`) is pruned explicitly, because `ignore`
//! excludes it only by way of the hidden filter this module turns off.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

use super::{git, relative_path};

/// Every file the tree should show (relative path + size in bytes) and every
/// directory under a root, ready to be turned into a
/// [`Tree`](crate::model::Tree) skeleton.
pub struct WalkResult {
    pub files: Vec<(PathBuf, u64)>,
    pub dirs: Vec<PathBuf>,
}

/// Walk `root`, collecting files (with sizes) and directories.
pub fn walk(root: &Path) -> WalkResult {
    let (in_repo, tracked) = git::repo_files(root);
    walk_with(root, &tracked, in_repo)
}

/// The walk proper, with the git-dependent inputs passed in so the union and
/// the ignore rules are testable without building a repository.
///
/// `in_repo` scopes `require_git`: inside a repository the `ignore` defaults
/// stand, so `.git/info/exclude` and gitdir files (linked worktrees and
/// submodules keep `.git` as a *file*) resolve as git resolves them. Outside
/// one, `require_git(false)` is what makes a stray `.gitignore` apply at all.
fn walk_with(root: &Path, tracked: &[PathBuf], in_repo: bool) -> WalkResult {
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    let mut dirs = Vec::new();

    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .require_git(in_repo)
        .filter_entry(|entry| !is_vcs_dir(entry));

    for result in builder.build() {
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
            // A symlink (never followed) is a file: that is how git stores one,
            // and `metadata` here is `symlink_metadata`, so the size is the
            // link's own — git's blob size — not the target's. Links *to a
            // directory* stay out: they would become file nodes the preview
            // then tried to read as a file.
            Some(ft) if ft.is_symlink() && !links_to_dir(entry.path()) => {
                let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push((rel, bytes));
            }
            // Directory symlinks and special files (sockets, fifos, devices).
            _ => {}
        }
    }

    union_tracked(root, &mut files, tracked);

    WalkResult { files, dirs }
}

/// Add tracked paths the walk didn't already yield — a tracked file matching a
/// `.gitignore` rule is still tracked, and the walk drops it.
///
/// Every path is added at most once. `build_skeleton` pushes a node per entry
/// without consulting its index, so a duplicate here would double-count bytes
/// and files up the whole tree — and the index *does* repeat a path, once per
/// stage, while a merge conflict is unresolved.
///
/// Only paths that exist on disk are added, which is also what skips an entry
/// staged for deletion. Sizing and the directory-symlink rule match the walk's
/// own, so a file doesn't change shape depending on which of the two found it.
fn union_tracked(root: &Path, files: &mut Vec<(PathBuf, u64)>, tracked: &[PathBuf]) {
    if tracked.is_empty() {
        return; // no repository, or nothing tracked: don't build the key set
    }
    let mut seen: HashSet<PathBuf> = files.iter().map(|(rel, _)| dedupe_key(rel)).collect();
    for rel in tracked {
        if !seen.insert(dedupe_key(rel)) {
            continue; // already walked, or a repeated conflict stage
        }
        let path = root.join(rel);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // staged for deletion, or otherwise gone from disk
        };
        if meta.is_dir() || (meta.is_symlink() && links_to_dir(&path)) {
            continue;
        }
        files.push((rel.clone(), meta.len()));
    }
}

/// The key a path is deduped under.
///
/// On a case-insensitive filesystem the index's spelling and the walk's can
/// differ for one physical file — `Makefile` in the index, `makefile` on disk
/// after a rename git doesn't notice — and the existence check resolves either.
/// Folding the key keeps that from becoming two nodes whose bytes are counted
/// twice in every ancestor. Case-sensitive filesystems must *not* fold: there,
/// `Foo` and `foo` really are two files.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dedupe_key(rel: &Path) -> PathBuf {
    PathBuf::from(rel.to_string_lossy().to_lowercase())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn dedupe_key(rel: &Path) -> PathBuf {
    rel.to_path_buf()
}

/// Whether an entry is VCS bookkeeping to prune — the directory, or the `.git`
/// *file* a linked worktree or submodule has in its place. `ignore` drops these
/// only via the hidden filter, which this module disables. Outside a repository
/// there is no `.gitignore` to fall back on, so the other VCSs are named too.
fn is_vcs_dir(entry: &DirEntry) -> bool {
    matches!(
        entry.file_name().to_str(),
        Some(".git" | ".jj" | ".hg" | ".svn")
    )
}

/// Whether `path` is a symlink resolving to a directory (a broken link is not).
fn links_to_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real directory under the system temp dir, removed on drop. The walk is
    /// filesystem I/O all the way down, so there is nothing pure to test here;
    /// this mirrors the `TempRoot` fixture in `app`'s tests.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tree-tui-walk-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the temp root");
            Self(dir)
        }

        fn write(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create a fixture dir");
            }
            std::fs::write(path, body).expect("write a fixture file");
        }

        /// The walk's file set, as sorted strings for readable assertions.
        fn walked(&self, tracked: &[&str], in_repo: bool) -> Vec<String> {
            let tracked: Vec<PathBuf> = tracked.iter().map(PathBuf::from).collect();
            let result = walk_with(&self.0, &tracked, in_repo);
            let mut names: Vec<String> = result
                .files
                .iter()
                .map(|(rel, _)| rel.to_string_lossy().replace('\\', "/"))
                .collect();
            names.sort();
            names
        }

        fn size_of(&self, tracked: &[&str], rel: &str) -> Option<u64> {
            let tracked: Vec<PathBuf> = tracked.iter().map(PathBuf::from).collect();
            walk_with(&self.0, &tracked, false)
                .files
                .into_iter()
                .find(|(p, _)| p == Path::new(rel))
                .map(|(_, bytes)| bytes)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn dot_entries_appear() {
        let root = TempRoot::new("dotfiles");
        root.write("README.md", "hi\n");
        root.write(".env", "A=1\n");
        root.write(".github/workflows/ci.yml", "on: push\n");

        assert_eq!(
            root.walked(&[], false),
            vec![".env", ".github/workflows/ci.yml", "README.md"],
            "a dotfile and a dot-directory's contents are ordinary files"
        );
    }

    #[test]
    fn the_git_directory_is_never_walked() {
        let root = TempRoot::new("gitdir");
        root.write("README.md", "hi\n");
        root.write(".git/config", "[core]\n");
        root.write(".git/objects/ab/cdef", "blob\n");
        root.write(".jj/repo/store", "x\n");

        assert_eq!(
            root.walked(&[], false),
            vec!["README.md"],
            "VCS bookkeeping is pruned even with hidden-filtering off"
        );
    }

    #[test]
    fn ignore_files_are_honored() {
        let root = TempRoot::new("ignored");
        root.write("README.md", "hi\n");
        root.write(".ignore", "secret.txt\n");
        root.write("secret.txt", "shh\n");

        let walked = root.walked(&[], false);
        assert!(!walked.contains(&"secret.txt".to_string()));
        assert!(
            walked.contains(&".ignore".to_string()),
            "the rule file itself is a file"
        );
    }

    #[test]
    fn gitignore_applies_outside_a_repository() {
        // `require_git(false)` is what makes this hold: the fixture has no
        // `.git`, so the `ignore` default would leave .gitignore inert.
        let root = TempRoot::new("gitignore");
        root.write("README.md", "hi\n");
        root.write(".gitignore", "build/\n");
        root.write("build/out.bin", "bin\n");

        let walked = root.walked(&[], false);
        assert!(!walked.contains(&"build/out.bin".to_string()));
    }

    #[test]
    fn a_tracked_file_appears_even_when_gitignored() {
        let root = TempRoot::new("tracked-ignored");
        root.write("README.md", "hi\n");
        root.write(".gitignore", "build/\n");
        root.write("build/out.bin", "bin\n");

        let walked = root.walked(&["build/out.bin"], false);
        assert!(
            walked.contains(&"build/out.bin".to_string()),
            "tracked wins over the ignore rule: {walked:?}"
        );
        assert_eq!(
            walked.iter().filter(|p| *p == "build/out.bin").count(),
            1,
            "and it is not duplicated"
        );
    }

    #[test]
    fn a_walked_file_is_not_duplicated_by_the_union() {
        let root = TempRoot::new("dedupe");
        root.write("README.md", "hi\n");
        root.write("src/main.rs", "fn main() {}\n");

        let walked = root.walked(&["README.md", "src/main.rs"], true);
        assert_eq!(walked, vec!["README.md", "src/main.rs"]);
    }

    #[test]
    fn a_tracked_file_missing_from_disk_is_skipped() {
        let root = TempRoot::new("staged-delete");
        root.write("README.md", "hi\n");

        assert_eq!(
            root.walked(&["deleted.txt"], true),
            vec!["README.md"],
            "an index entry with no file on disk must not become a 0-byte node"
        );
    }

    #[test]
    fn a_conflicted_path_repeated_in_the_index_is_added_once() {
        // While a merge is unresolved the index holds the same path at stages
        // 1/2/3. `build_skeleton` trusts this list, so a repeat would double
        // the file's bytes in every ancestor total.
        let root = TempRoot::new("conflict");
        root.write(".gitignore", "build/\n");
        root.write("build/out.bin", "bin\n");

        // `in_repo: false` so the .gitignore actually applies and the file is
        // absent from the walk — otherwise the union never runs at all.
        let tracked = ["build/out.bin", "build/out.bin", "build/out.bin"];
        let walked = root.walked(&tracked, false);
        assert_eq!(
            walked.iter().filter(|p| *p == "build/out.bin").count(),
            1,
            "three conflict stages are one file: {walked:?}"
        );
    }

    #[test]
    fn a_tracked_directory_is_not_added_as_a_file() {
        let root = TempRoot::new("tracked-dir");
        root.write(".gitignore", "build/\n");
        root.write("build/out.bin", "bin\n");

        // A sparse/gitlink entry naming a directory must not become a file node.
        assert!(
            !root
                .walked(&["build"], false)
                .contains(&"build".to_string())
        );
    }

    /// Build a scratch repository with `git`, returning false if the binary
    /// isn't available so the test skips rather than fails.
    fn git_init(root: &Path, args: &[&[&str]]) -> bool {
        for a in args {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(*a)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !ok {
                return false;
            }
        }
        true
    }

    /// The union against a *real* index. Without this, stubbing
    /// `git::repo_files` to return nothing leaves every other test green:
    /// this crate tracks no ignored path, so the union contributes nothing to
    /// a walk of its own checkout.
    #[test]
    fn a_force_added_ignored_file_comes_back_from_the_index() {
        let root = TempRoot::new("real-index");
        root.write(".gitignore", "vendor/\n");
        root.write("vendor/lib.rs", "fn f() {}\n");
        root.write("src/main.rs", "fn main() {}\n");
        if !git_init(
            &root.0,
            &[
                &["init", "-q"],
                &["add", "src/main.rs", ".gitignore"],
                &["add", "-f", "vendor/lib.rs"],
            ],
        ) {
            return; // no usable `git`: skip rather than fail
        }

        let (in_repo, tracked) = git::repo_files(&root.0);
        assert!(in_repo, "the fixture is a repository");
        assert!(
            tracked.contains(&PathBuf::from("vendor/lib.rs")),
            "the index must report the force-added path: {tracked:?}"
        );

        let mut walked: Vec<String> = walk(&root.0)
            .files
            .iter()
            .map(|(rel, _)| rel.to_string_lossy().replace('\\', "/"))
            .collect();
        walked.sort();
        assert_eq!(
            walked,
            vec![".gitignore", "src/main.rs", "vendor/lib.rs"],
            "tracked-but-ignored appears exactly once, and .git never does"
        );
    }

    /// Scanning a subdirectory of a repository must strip the prefix, not bail.
    #[test]
    fn a_subdirectory_scan_root_still_resolves_tracked_paths() {
        let root = TempRoot::new("subdir-root");
        root.write(".gitignore", "crate/vendor/\n");
        root.write("crate/vendor/lib.rs", "fn f() {}\n");
        root.write("crate/src/main.rs", "fn main() {}\n");
        if !git_init(
            &root.0,
            &[&["init", "-q"], &["add", "-f", "crate/vendor/lib.rs"]],
        ) {
            return;
        }

        let (_, tracked) = git::repo_files(&root.0.join("crate"));
        assert!(
            tracked.contains(&PathBuf::from("vendor/lib.rs")),
            "paths are relative to the scan root, not the repo root: {tracked:?}"
        );
    }

    /// This crate's own checkout, which tracks dot-entries. Inert when built
    /// outside a repository (the published tarball has no `.git`).
    #[test]
    fn this_repository_shows_its_tracked_dot_entries() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        if !git::is_repo(root) {
            return;
        }
        let files: HashSet<PathBuf> = walk(root).files.into_iter().map(|(p, _)| p).collect();

        assert!(files.contains(Path::new(".gitignore")), "tracked dotfile");
        assert!(
            files.contains(Path::new(".github/workflows/ci.yml")),
            "tracked file under a dot-directory"
        );
        assert!(
            !files.iter().any(|p| p.starts_with(".git/")),
            "the git object store is never shown"
        );
        assert!(
            !files.iter().any(|p| p.starts_with("target/")),
            "gitignored build output stays hidden"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_tracked_directory_symlink_is_excluded_like_a_walked_one() {
        let root = TempRoot::new("tracked-dirlink");
        root.write(".gitignore", "vendor/\n");
        root.write("vendor/real/file.txt", "x\n");
        std::os::unix::fs::symlink("real", root.0.join("vendor/link")).expect("dir symlink");

        // Tracked or walked, a link to a directory must not become a file node.
        let walked = root.walked(&["vendor/link"], true);
        assert!(
            !walked.contains(&"vendor/link".to_string()),
            "union must apply the same rule as the walk: {walked:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_files_but_directory_links_are_not() {
        let root = TempRoot::new("symlinks");
        root.write("README.md", "hi\n");
        root.write("src/main.rs", "fn main() {}\n");
        std::os::unix::fs::symlink("README.md", root.0.join("link.md")).expect("file symlink");
        std::os::unix::fs::symlink("src", root.0.join("link-dir")).expect("dir symlink");

        let walked = root.walked(&[], false);
        assert!(
            walked.contains(&"link.md".to_string()),
            "a file symlink is a file"
        );
        assert!(
            !walked.contains(&"link-dir".to_string()),
            "a directory symlink is not, so the preview never reads a dir as a file"
        );
        assert_eq!(
            root.size_of(&[], "link.md"),
            Some("README.md".len() as u64),
            "size is the link's own bytes (git's blob size), not the target's"
        );
    }
}
