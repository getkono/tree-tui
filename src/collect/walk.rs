//! The eager filesystem walk: the tree skeleton plus per-file size.
//!
//! The rule is "what git would show you": every tracked file, plus every
//! untracked file git wouldn't ignore. Concretely, the `ignore` crate's walk
//! with hidden-filtering **off** (a dot-entry is an ordinary file — `.github/`
//! and `.gitignore` are tracked like any other path), unioned with the git
//! index so a tracked-but-gitignored file still gets a node. VCS bookkeeping
//! (`.git`, `.jj`, `.hg`, `.svn`) is pruned explicitly, because `ignore`
//! excludes it only by way of the hidden filter this module turns off.
//!
//! The traversal is **parallel**. It is the one eager cost in the app — it runs
//! before the first frame, and again on every debounced filesystem event — and
//! it is stat-bound, so it scales with threads. [`walk_with_sequential`] is the
//! same walk over `ignore`'s single-threaded iterator, kept as the oracle the
//! parallel traversal is differentially tested against and as the benchmark
//! baseline; the two share [`builder`] and [`record`], so they can differ in
//! how an entry is reached and never in what is recorded for it.
//!
//! The returned `files`/`dirs` are in traversal order, which the parallel walk
//! leaves unspecified. Nothing depends on it: `build_skeleton` sorts both
//! before building the arena, and `App::same_skeleton` compares a path-to-bytes
//! map. The *set* is what matters, and that is what the tests pin.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, WalkBuilder, WalkState};

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
/// Thread count is `ignore`'s own default — `available_parallelism`, capped at
/// 12 — rather than a number this crate invents.
#[doc(hidden)]
pub fn walk_with(root: &Path, tracked: &[PathBuf], in_repo: bool) -> WalkResult {
    let merged = Mutex::new(Findings::default());
    builder(root, in_repo)
        .build_parallel()
        .visit(&mut VisitorBuilder {
            root,
            merged: &merged,
        });

    let Findings { mut files, dirs } = merged.into_inner().unwrap_or_else(PoisonError::into_inner);
    union_tracked(root, &mut files, tracked);

    WalkResult { files, dirs }
}

/// [`walk_with`] over `ignore`'s single-threaded iterator.
///
/// Retained as the oracle the parallel traversal is differentially tested
/// against and as the benchmark baseline — `#[cfg(test)]` would put it out of
/// reach of `benches/`, which is a separate crate. Production never calls it.
#[doc(hidden)]
pub fn walk_with_sequential(root: &Path, tracked: &[PathBuf], in_repo: bool) -> WalkResult {
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    let mut dirs = Vec::new();

    for result in builder(root, in_repo).build() {
        let Ok(entry) = result else {
            continue; // unreadable entry: skip rather than fail the whole scan
        };
        record(&entry, root, &mut files, &mut dirs);
    }

    union_tracked(root, &mut files, tracked);

    WalkResult { files, dirs }
}

/// What one traversal thread found, and — once merged — what all of them did.
#[derive(Default)]
struct Findings {
    files: Vec<(PathBuf, u64)>,
    dirs: Vec<PathBuf>,
}

/// Hands `ignore` one [`Visitor`] per traversal thread.
struct VisitorBuilder<'a> {
    root: &'a Path,
    merged: &'a Mutex<Findings>,
}

impl<'s> ParallelVisitorBuilder<'s> for VisitorBuilder<'s> {
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        Box::new(Visitor {
            root: self.root,
            local: Findings::default(),
            merged: self.merged,
        })
    }
}

/// One traversal thread. It accumulates into `local` and takes the lock exactly
/// once, on the way out — a walk is millions of cheap entries, and contending a
/// mutex per entry would hand back everything the threads win.
///
/// `WalkParallel::visit` runs its workers inside `std::thread::scope`, so
/// borrowing the root and the shared sink from the caller's frame is sound and
/// no `Arc` is needed.
struct Visitor<'a> {
    root: &'a Path,
    local: Findings,
    merged: &'a Mutex<Findings>,
}

impl ParallelVisitor for Visitor<'_> {
    fn visit(&mut self, result: Result<DirEntry, ignore::Error>) -> WalkState {
        let Ok(entry) = result else {
            return WalkState::Continue; // unreadable entry: skip, don't fail the scan
        };
        record(
            &entry,
            self.root,
            &mut self.local.files,
            &mut self.local.dirs,
        );
        WalkState::Continue
    }
}

impl Drop for Visitor<'_> {
    fn drop(&mut self) {
        // `into_inner` rather than `unwrap` on a poisoned lock: this also runs
        // while unwinding from a panicking worker, and panicking here would
        // turn that into a double panic, which aborts.
        let mut merged = self.merged.lock().unwrap_or_else(PoisonError::into_inner);
        merged.files.append(&mut self.local.files);
        merged.dirs.append(&mut self.local.dirs);
    }
}

/// The walk's configuration, in one place so the traversal can never be the
/// thing that changes which files are shown.
///
/// `in_repo` scopes `require_git`: inside a repository the `ignore` defaults
/// stand, so `.git/info/exclude` and gitdir files (linked worktrees and
/// submodules keep `.git` as a *file*) resolve as git resolves them. Outside
/// one, `require_git(false)` is what makes a stray `.gitignore` apply at all.
fn builder(root: &Path, in_repo: bool) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .require_git(in_repo)
        .filter_entry(|entry| !is_vcs_dir(entry));
    builder
}

/// Classify one walked entry into the file and directory buckets.
///
/// Everything the walk *decides* lives here rather than in a traversal loop, so
/// the two traversals can differ in how they reach an entry and never in what
/// they record for it.
fn record(entry: &DirEntry, root: &Path, files: &mut Vec<(PathBuf, u64)>, dirs: &mut Vec<PathBuf>) {
    let rel = relative_path(entry.path(), root);
    if rel.as_os_str().is_empty() {
        return; // the root itself is the tree root node
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A real directory under the system temp dir, removed on drop. The walk is
    /// filesystem I/O all the way down, so there is nothing pure to test here;
    /// this mirrors the `TempRoot` fixture in `app`'s tests.
    pub(super) struct TempRoot(PathBuf);

    impl TempRoot {
        pub(super) fn new(tag: &str) -> Self {
            // The nonce is what makes this reusable from a property test, which
            // materializes many trees per thread: pid + thread id alone repeat.
            static NONCE: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "tree-tui-walk-{tag}-{}-{:?}-{}",
                std::process::id(),
                std::thread::current().id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the temp root");
            Self(dir)
        }

        pub(super) fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, body: &str) {
            self.try_write(rel, body).expect("write a fixture file");
        }

        /// [`write`](Self::write) for generated paths, where a failure is
        /// expected and meaningful: a wish list can name `a/b` and `a` both, and
        /// only one of a file and a directory can exist at `a`. The walk reads
        /// whatever landed.
        pub(super) fn try_write(&self, rel: &str, body: &str) -> std::io::Result<()> {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, body)
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

    /// A walk result reduced to two sorted lists, so two walks compare by what
    /// they found rather than by the order threads happened to finish in.
    pub(super) fn normalized(result: &WalkResult) -> (Vec<(PathBuf, u64)>, Vec<PathBuf>) {
        let mut files = result.files.clone();
        let mut dirs = result.dirs.clone();
        files.sort();
        dirs.sort();
        (files, dirs)
    }

    /// The two traversals must agree on a real checkout: a real index, real
    /// ignore rules, a real `target/` to prune, and a directory shape no
    /// fixture here is untidy enough to reproduce.
    ///
    /// The index is read once and handed to both. `walk` would re-read it per
    /// call, and a `git` process rewriting `.git/index` mid-test — this suite
    /// also runs from a pre-commit hook — would then look like a traversal that
    /// disagreed with itself.
    #[test]
    fn the_two_traversals_agree_on_this_checkout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (in_repo, tracked) = git::repo_files(root);

        assert_eq!(
            normalized(&walk_with(root, &tracked, in_repo)),
            normalized(&walk_with_sequential(root, &tracked, in_repo)),
            "the parallel walk found a different tree than the sequential one"
        );
    }

    /// A tree wide enough that the traversal genuinely fans out, walked
    /// repeatedly.
    ///
    /// `ignore` hands work out one directory at a time and stealing is lazy, so
    /// on the handful of directories a generated fixture holds the spare
    /// workers usually find nothing before the walk is over — which would leave
    /// the property differential comparing an effectively single-threaded
    /// parallel walk against the sequential one. 64 sibling directories give
    /// the workers something to steal, so this is where several `Visitor`s
    /// really do accumulate and the `Drop`-time merge is under test: a flush
    /// that lost a worker's findings shows up as a short count.
    ///
    /// Repeating it also pins the stability `App::same_skeleton` depends on —
    /// a set that wobbled between walks would rebuild the arena and drop all
    /// three cached metric layers on a rescan that changed nothing. Unlike the
    /// checkout above, this root is ours, so nothing but scheduling can vary.
    #[test]
    fn a_wide_tree_fans_out_and_merges_every_worker() {
        const DIRS: usize = 64;
        const PER_DIR: usize = 8;

        let root = TempRoot::new("wide");
        for d in 0..DIRS {
            for f in 0..PER_DIR {
                root.write(&format!("d{d}/f{f}.rs"), "fn f() {}\n");
            }
        }
        let expected = normalized(&walk_with_sequential(root.path(), &[], false));

        for run in 0..20 {
            let result = walk_with(root.path(), &[], false);
            assert_eq!(
                result.files.len(),
                DIRS * PER_DIR,
                "run {run}: a worker's findings went missing in the merge"
            );
            assert_eq!(
                result.dirs.len(),
                DIRS,
                "run {run}: a directory went missing"
            );
            assert_eq!(normalized(&result), expected, "run {run} disagreed");
        }
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

/// Property tests over generated directory trees.
///
/// The example tests above each pin one rule. These pin the rules that must
/// hold for *every* tree: that the walk reports real, non-directory files at
/// their own size, exactly once each; that VCS bookkeeping never leaks; and
/// that the tracked-file union adds what git tracks without duplicating it.
///
/// They exist to ground the walk's behaviour independently of how it traverses,
/// so that swapping the traversal is a change no test has to be edited for.
#[cfg(test)]
mod props {
    use std::path::Component;

    use proptest::prelude::*;

    use super::tests::{TempRoot, normalized};
    use super::*;

    /// The alphabet generated paths are drawn from.
    ///
    /// Deliberately tiny, and deliberately made of names that *mean* something
    /// to the walk. With a large alphabet a generated tree is a pile of
    /// unrelated files and the interesting cases — a path colliding with a
    /// directory, an ignore rule that actually matches, VCS bookkeeping in the
    /// way — essentially never occur.
    const NAMES: &[&str] = &[
        "a",
        "b",
        "dir",
        "nested",
        ".hidden",
        ".env",
        ".git",
        ".jj",
        ".hg",
        ".svn",
        ".gitignore",
        ".ignore",
        "build",
        "target",
        "vendor",
    ];

    /// VCS bookkeeping directories, which no walk may ever report through.
    const VCS: &[&str] = &[".git", ".jj", ".hg", ".svn"];

    /// A wish list for a directory tree. Nothing here is guaranteed to land —
    /// a file cannot be created under a path that is already a file — and that
    /// is the point: the disk is the truth, and both traversals see the same disk.
    #[derive(Debug, Clone)]
    pub(super) struct Spec {
        /// Each path is a component list; the last component names the file.
        paths: Vec<Vec<&'static str>>,
        /// Lines written to a root `.gitignore`, if any.
        ignore_rules: Vec<&'static str>,
        /// Selections from `paths`, with repeats — a merge-conflicted index
        /// holds the same path once per stage.
        tracked: Vec<prop::sample::Index>,
        /// Whether the walk is told it is inside a repository, which scopes
        /// `require_git` and so decides whether `.gitignore` applies at all.
        in_repo: bool,
        /// `(target selection, kind)` for symlinks placed at the root.
        links: Vec<(prop::sample::Index, u8)>,
    }

    /// `prop::sample::Index` rather than a `0..N` range: it resolves against the
    /// actual length of `paths`, so every generated tracked entry and every
    /// generated symlink names a path that exists. A fixed range against a
    /// variable-length list silently drops roughly half of them, and the
    /// survivors skew to the lowest indices.
    fn arb_spec() -> impl Strategy<Value = Spec> {
        (
            prop::collection::vec(
                prop::collection::vec(prop::sample::select(NAMES), 1..=4),
                1..=24,
            ),
            prop::collection::vec(prop::sample::select(NAMES), 0..=3),
            prop::collection::vec(any::<prop::sample::Index>(), 0..=8),
            any::<bool>(),
            prop::collection::vec((any::<prop::sample::Index>(), 0u8..3), 0..=3),
        )
            .prop_map(|(paths, ignore_rules, tracked, in_repo, links)| Spec {
                paths,
                ignore_rules,
                tracked,
                in_repo,
                links,
            })
    }

    impl Spec {
        /// Write the tree and return the tracked list the walk should be given.
        fn materialize(&self, root: &TempRoot) -> Vec<PathBuf> {
            for comps in &self.paths {
                let _ = root.try_write(&comps.join("/"), "x\n");
            }
            if !self.ignore_rules.is_empty() {
                let body = self
                    .ignore_rules
                    .iter()
                    .map(|r| format!("{r}\n"))
                    .collect::<String>();
                let _ = root.try_write(".gitignore", &body);
            }
            self.place_links(root);

            self.tracked
                .iter()
                .map(|i| i.get(&self.paths))
                // A real git index never holds a path under `.git/` — the union
                // trusts its input and would happily add one, so modelling that
                // would be testing a state git cannot produce.
                .filter(|comps| !comps.iter().any(|c| VCS.contains(c)))
                .map(|comps| PathBuf::from(comps.join("/")))
                .collect()
        }

        #[cfg(unix)]
        fn place_links(&self, root: &TempRoot) {
            for (n, (target, kind)) in self.links.iter().enumerate() {
                let target = match kind {
                    // A link to a generated path (a file, if that path landed).
                    0 => target.get(&self.paths).join("/"),
                    // A link to a directory that exists: the root itself.
                    1 => ".".to_string(),
                    // A broken link.
                    _ => "nowhere-at-all".to_string(),
                };
                let _ = std::os::unix::fs::symlink(target, root.path().join(format!("link{n}")));
            }
        }

        #[cfg(not(unix))]
        fn place_links(&self, _root: &TempRoot) {}
    }

    /// Materialize `spec` and walk it, returning the fixture alongside the
    /// result so the assertions can stat the disk the walk just read.
    fn walked(spec: &Spec) -> (TempRoot, Vec<PathBuf>, WalkResult) {
        let root = TempRoot::new("prop");
        let tracked = spec.materialize(&root);
        let result = walk_with(root.path(), &tracked, spec.in_repo);
        (root, tracked, result)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        /// Every reported file is a real, non-directory path under the root,
        /// carrying its own size, listed exactly once.
        ///
        /// The "exactly once" half is what `build_skeleton` trusts: it pushes a
        /// node per entry without consulting its index, so a duplicate would
        /// double the file's bytes in every ancestor total.
        #[test]
        fn reported_files_are_real_sized_and_unique(spec in arb_spec()) {
            let (root, _, result) = walked(&spec);

            for (rel, bytes) in &result.files {
                prop_assert!(rel.is_relative(), "{} is not relative", rel.display());
                prop_assert!(
                    rel.components().all(|c| matches!(c, Component::Normal(_))),
                    "{} escapes the root", rel.display()
                );

                let abs = root.path().join(rel);
                let meta = std::fs::symlink_metadata(&abs);
                prop_assert!(meta.is_ok(), "{} was reported but is not on disk", rel.display());
                let meta = meta.expect("checked just above");

                prop_assert!(!meta.is_dir(), "{} is a directory", rel.display());
                prop_assert!(
                    !(meta.is_symlink() && links_to_dir(&abs)),
                    "{} is a symlink to a directory", rel.display()
                );
                prop_assert_eq!(
                    *bytes, meta.len(),
                    "{} reported the wrong size", rel.display()
                );
            }

            let mut keys: Vec<PathBuf> = result.files.iter().map(|(r, _)| dedupe_key(r)).collect();
            let total = keys.len();
            keys.sort();
            keys.dedup();
            prop_assert_eq!(keys.len(), total, "a path was reported more than once");
        }

        /// The parallel walk and the sequential one find the same files, at the
        /// same sizes, and the same directories.
        ///
        /// This is the whole justification for traversing in parallel:
        /// `build_skeleton` sorts both lists before building the arena, so the
        /// *set* is the contract and the order is not.
        #[test]
        fn the_two_traversals_agree(spec in arb_spec()) {
            let root = TempRoot::new("prop");
            let tracked = spec.materialize(&root);

            let parallel = walk_with(root.path(), &tracked, spec.in_repo);
            let sequential = walk_with_sequential(root.path(), &tracked, spec.in_repo);

            prop_assert_eq!(normalized(&parallel), normalized(&sequential));
        }

        /// VCS bookkeeping never appears, as a file or as a directory. `ignore`
        /// drops it only via the hidden filter, which this module turns off, so
        /// this is entirely on `filter_entry`.
        #[test]
        fn vcs_bookkeeping_is_never_reported(spec in arb_spec()) {
            let (_root, _, result) = walked(&spec);

            let reported = result.files.iter().map(|(r, _)| r).chain(result.dirs.iter());
            for rel in reported {
                prop_assert!(
                    !rel.components().any(|c| matches!(
                        c, Component::Normal(name) if name.to_str().is_some_and(|n| VCS.contains(&n))
                    )),
                    "{} is VCS bookkeeping", rel.display()
                );
            }
        }

        /// Every tracked path that exists on disk as a file gets exactly one
        /// entry — whether the walk found it or the union added it, and however
        /// many conflict stages the index repeats it for.
        #[test]
        fn tracked_files_on_disk_are_reported_exactly_once(spec in arb_spec()) {
            let (root, tracked, result) = walked(&spec);

            for rel in &tracked {
                let abs = root.path().join(rel);
                let Ok(meta) = std::fs::symlink_metadata(&abs) else {
                    continue; // staged for deletion, or the write never landed
                };
                if meta.is_dir() || (meta.is_symlink() && links_to_dir(&abs)) {
                    continue; // a gitlink or sparse entry is not a file node
                }

                let key = dedupe_key(rel);
                let count = result.files.iter().filter(|(r, _)| dedupe_key(r) == key).count();
                prop_assert_eq!(
                    count, 1,
                    "tracked {} was reported {} times", rel.display(), count
                );
            }
        }
    }
}
