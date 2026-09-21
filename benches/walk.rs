//! The filesystem walk, sequential against parallel.
//!
//! The walk is the app's only eager cost and it re-runs on every debounced
//! filesystem event, so both halves of the curve matter: the speedup on a large
//! tree, and what a dozen threads cost on a small one, where spinning them up
//! can plausibly be more than the traversal itself.
//!
//! Roots:
//!
//! - `checkout` — this repository, a few dozen files. The small case.
//! - `synthetic/<n>-files` — a generated tree, so the benchmark reproduces
//!   anywhere and the crossover can be located rather than asserted. Sizes come
//!   from `TREE_TUI_BENCH_FILES` as a comma-separated sweep (default `2000`,
//!   modest because `cargo test` also builds and runs this target).
//! - `large` — whatever `TREE_TUI_BENCH_ROOT` points at; skipped when unset.
//!
//! Each root is measured three ways: the sequential traversal, the parallel
//! one, and the parallel one with nothing tracked. `union_tracked` returns
//! immediately with an empty tracked set, so the third is the traversal alone
//! and the gap to `parallel` is what the union post-pass costs. For that gap to
//! mean anything the root needs a tracked set, so the synthetic tree declares
//! every file it wrote as tracked — the shape a real checkout has.
//!
//! ```text
//! cargo bench
//! TREE_TUI_BENCH_FILES=200,2000,20000 TREE_TUI_BENCH_ROOT=~/src cargo bench
//! ```

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use tree_tui::collect::{repo_files, walk_with, walk_with_sequential};

/// The synthetic sweep when `TREE_TUI_BENCH_FILES` is unset.
const DEFAULT_SYNTHETIC_FILES: &[usize] = &[2_000];

/// Files per directory in the synthetic tree — enough breadth that the parallel
/// traversal has independent directories to hand out, which is the shape a
/// source tree has and a single wide fan does not.
const FILES_PER_DIR: usize = 10;

/// A generated directory tree, removed on drop.
struct Synthetic {
    root: PathBuf,
    /// Every file written, relative to the root: the benchmark's tracked set.
    files: Vec<PathBuf>,
}

impl Synthetic {
    /// `count` files over `count / FILES_PER_DIR` directories, nested two levels
    /// deep so the walk has a tree to descend.
    fn build(count: usize) -> std::io::Result<Self> {
        let root =
            std::env::temp_dir().join(format!("tree-tui-bench-{}-{count}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root)?;

        let mut files = Vec::with_capacity(count);
        let dirs = count.div_ceil(FILES_PER_DIR);
        for d in 0..dirs {
            let rel_dir = PathBuf::from(format!("g{}", d % 32)).join(format!("d{d}"));
            std::fs::create_dir_all(root.join(&rel_dir))?;
            // The last directory holds the remainder, which is why this is a
            // `min` and not a flat `FILES_PER_DIR`.
            for f in 0..FILES_PER_DIR.min(count - d * FILES_PER_DIR) {
                let rel = rel_dir.join(format!("f{f}.rs"));
                std::fs::write(root.join(&rel), "fn f() {}\n")?;
                files.push(rel);
            }
        }
        Ok(Self { root, files })
    }
}

impl Drop for Synthetic {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Bench one root three ways. `tracked` and `in_repo` are resolved by the
/// caller, *outside* the measured region: the subject is the traversal, not
/// gix's index read.
fn bench_root(
    c: &mut Criterion,
    name: &str,
    root: &Path,
    tracked: &[PathBuf],
    in_repo: bool,
    samples: usize,
) {
    let mut group = c.benchmark_group(name);
    // Small roots get the full sample count: the claim there is "no meaningful
    // regression", and at criterion's floor of 10 a small regression hides
    // inside the confidence interval. Large roots cannot afford it.
    group.sample_size(samples);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("sequential", |b| {
        b.iter(|| black_box(walk_with_sequential(black_box(root), tracked, in_repo)))
    });
    group.bench_function("parallel", |b| {
        b.iter(|| black_box(walk_with(black_box(root), tracked, in_repo)))
    });
    group.bench_function("parallel/traversal-only", |b| {
        b.iter(|| black_box(walk_with(black_box(root), &[], in_repo)))
    });

    group.finish();
}

fn benches(c: &mut Criterion) {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (in_repo, tracked) = repo_files(checkout);
    bench_root(c, "checkout", checkout, &tracked, in_repo, 100);

    for count in synthetic_sizes() {
        // A tree under ~10 000 files runs in single-digit milliseconds, so the
        // full sample count is affordable and the crossover needs the precision.
        let samples = if count <= 10_000 { 100 } else { 10 };
        match Synthetic::build(count) {
            // `in_repo: false` — the fixture has no `.git`, and saying otherwise
            // would make `require_git` switch off ignore-file handling.
            Ok(tree) => bench_root(
                c,
                &format!("synthetic/{count}-files"),
                &tree.root,
                &tree.files,
                false,
                samples,
            ),
            Err(err) => eprintln!("skipping the {count}-file synthetic tree: {err}"),
        }
    }

    match std::env::var_os("TREE_TUI_BENCH_ROOT") {
        Some(root) if Path::new(&root).is_dir() => {
            let root = Path::new(&root);
            let (in_repo, tracked) = repo_files(root);
            bench_root(c, "large", root, &tracked, in_repo, 10);
        }
        Some(root) => eprintln!("TREE_TUI_BENCH_ROOT is not a directory: {root:?}"),
        None => {} // the large case is opt-in; `cargo bench` alone stays quick
    }
}

/// The synthetic sweep, as a comma-separated list of file counts.
fn synthetic_sizes() -> Vec<usize> {
    let Ok(raw) = std::env::var("TREE_TUI_BENCH_FILES") else {
        return DEFAULT_SYNTHETIC_FILES.to_vec();
    };
    let mut sizes: Vec<usize> = raw
        .split(',')
        .filter_map(|part| match part.trim().parse::<usize>() {
            Ok(n) if n > 0 => Some(n),
            // Say so: a typo that silently benched one fewer size would just
            // look like the sweep had a gap.
            _ => {
                eprintln!("ignoring unusable size in TREE_TUI_BENCH_FILES: {part:?}");
                None
            }
        })
        .collect();
    // Two equal sizes would name the same criterion group twice.
    sizes.sort_unstable();
    sizes.dedup();
    if sizes.is_empty() {
        eprintln!("TREE_TUI_BENCH_FILES held no usable sizes: {raw:?}");
        return DEFAULT_SYNTHETIC_FILES.to_vec();
    }
    sizes
}

criterion_group!(walk, benches);
criterion_main!(walk);
