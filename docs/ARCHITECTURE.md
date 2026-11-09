# Architecture

`tree` is a directory visualizer: one navigable tree, viewed through swappable **lenses** (code,
size, churn, status). The design separates a **shared, metric-agnostic core** from **modular
per-tool pieces**, and computes expensive metrics **lazily** (on first use) and **caches** them.

## Data flow

```
walk (ignore, parallel)  ──►  build_skeleton  ──►  Tree (skeleton + bytes + files, + path index)
                                          │
   open a lens ──► request compute ──► collector (blocking thread) ──► per-file map
                                          │
                          aggregate (bottom-up) ──► cached Layer<T>
                                          │
                       value_of / sort ──► flatten ──► render (ratatui)
```

- The **walk** (`collect::walk`, via the `ignore` crate) is cheap (structure + file sizes, no
  contents). Its rule is "what git would show you": every tracked file (unioned in from the git
  index, so a tracked-but-gitignored one still gets a node) plus every untracked file git wouldn't
  ignore. Hidden-filtering is off — a dot-entry is an ordinary file — so `.git`/`.jj` are pruned
  explicitly. Non-code files appear too.
- The walk **traverses in parallel** (`ignore`'s `build_parallel`, one accumulator per thread
  merged once at the end). It is the app's only eager cost, and it runs on more than startup: every
  debounced batch of filesystem events that holds a change (anything but a read: a create, modify,
  remove, close after writing, or unclassified event; see `watch::is_change`) re-walks the whole
  tree. A batch of reads alone (opens and closes without a write) is dropped and re-walks nothing.
  `walk_with_sequential` is the same walk over the single-threaded iterator, kept as the oracle for the differential tests and as the benchmark
  baseline (`cargo bench`); both share the walk's `builder` and `record`, so they can differ in how
  an entry is reached and never in what is recorded for it.
- The walk's output **order is unspecified** — `build_skeleton` sorts both lists before building
  the arena, and `App::same_skeleton` compares a path-to-bytes map, so only the set matters.
- A **lens** is opened on demand. If its data isn't cached, the event loop spawns the lens's
  **collector** on a blocking thread; the result comes back over an `mpsc` channel, is **aggregated**
  bottom-up into a per-node `Layer`, and cached for the session.

## Crate layout

The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`, the `tree` executable,
which only wraps `run_cli` in `#[tokio::main]`). The split exists so `benches/` can link against
the walk; a binary-only crate has no target to link to. Only `collect` and `model` are `pub` —
everything else is a private module, and the library is an internal detail of the binary with no
API-stability promise.

## Shared core vs modular tools

**Shared core (metric-agnostic):**

- `model::node` — the arena `Tree` / `TreeNode` skeleton, the always-on `bytes`/`files`, the cached
  `Layer<T>` type, and the per-lens data structs (`CodeData`, `ChurnData`, `StatusData`).
- `model::build` — `build_skeleton` plus the generic bottom-up folds `aggregate` / `aggregate_code`.
- `model::view` — `sort_by_values` / `sort_by_name` and the visible/filtered flattening. A
  directory whose only child is a single sub-directory is **concatenated** with it into one row
  (`src/main/java`); the chain expands/collapses as a unit (see `segment_tail` / `segment_head`).
- `app` — state, the reducer, the lazy-compute request/cache wiring, and `Loaded::value` (the one
  place that maps a `SubKey` to the node field or layer it reads).
- `event` — the `tokio::select!` loop: input, walk completion, lens results, spinner ticks.
- `ui` — the render scaffold (header / tree table / preview / footer / help) driven by the active
  lens. The tree/preview split is state (`Loaded::split_share`, in thousandths of the body width),
  not a constant: the seam between the panes is a two-column drag handle, which is why `tui` enables
  xterm mouse mode 1002 (motion while a button is held) but still not 1003 (all motion).

File viewing is not part of the metric core. The inline **preview** pane (`ui::preview`) and the
full-screen **reader** (`ui::reader`) both render through **`ui::fileview`**, which composes the
karet toolkit into one "render any file" widget: `FileDoc::prepare` classifies the bytes and
decodes / parses / highlights them once per opened file, and `FileView` dispatches that payload to
a primitive every frame — `karet-editor`'s read-only editor for text, `karet-fileview`'s `HexView`
for binaries, its terminal image renderer for images and rasterized PDF pages, and its placeholder
for everything else. `FileViewState` carries the scroll position across frames, so the two callers
keep only their own glue: bounded reads, search state, the `$EDITOR`/`$PAGER` handoffs, and the
suspended-tree ownership.

Three things the dispatch owns are worth knowing about:

- **The highlight budget.** Parsing is `O(file)`, so `prepare` skips it above a line budget (500
  lines for the preview, 20 000 for the reader) and renders the text unhighlighted. Nothing in
  karet enforces this; without it the preview pane stalls the loop on a large file.
- **The Kitty handshake.** A Kitty graphics placement is not part of the cell buffer, so the
  widget only *reserves* a rect during the frame and `event::draw` transmits the pixels after
  `terminal.draw`. The same seam clears the placement whenever a frame stops showing one — a new
  selection, the help overlay, the pane folding away, teardown — since redrawing the cells
  underneath would not remove it. Terminals without Kitty get truecolor half-blocks instead, and
  detection is env-only so it never races crossterm's event reader.
- **Freshness.** A prepared document is a snapshot, so something has to notice when the file it
  came from changes. `ui::preview::load` records the file's length and mtime as a `Stamp`, and
  `App::on_rescan` re-stats the previewed path on every watcher signal — *before* the
  `same_skeleton` gate, which compares paths and sizes and so would otherwise swallow an
  in-place edit of the same length. A mismatch marks the preview stale, which arms the same
  80 ms debounce a selection move does, so the read and the parse stay off the navigation
  frames. The reader's document is deliberately exempt: it holds its snapshot rather than
  discarding a scrolled, folded, mid-search view.

**Modular tools:**

- **Collectors** (`collect::{walk, code, git}`) — independent data sources keyed by relative path.
  The code collector counts exactly the walked files (the event loop hands it the tree's file
  list), so a force-added ignored file is counted like any other. It uses tokei per file, for the
  language and the code/comment split, and corrects two things tokei does differently from what the
  lens means. **Blank** is decided per line, in any comment or string state: whitespace (Unicode,
  plus zero-width characters) or a bare comment marker (`//`, `///`, `#`, a lone `*` in a C-family
  language (both `/* */` and `//`)). **Embedded languages** (Markdown fences) are folded back into the file's totals, and
  Rust doc comments are counted as the file's own comments rather than lifted into a Markdown
  blob, which tokei mis-scans. Each file keeps `code + comments + blanks` equal to its physical
  line count; a property test holds the per-file count to it.
- **Lens presentation** (`model::lens`) — each lens is a variant of the `Lens` enum, with its
  behavior localized to `match` arms (`columns`, `primary`, `sub_keys`, …).

## Why `Lens` is an enum, not a trait

The whole point is *one* shared core. An enum keeps a single `Tree`, a single aggregation, and a
single sort; lenses only *select and present*. It also matches the codebase's existing style
(`SortDir`, `SubKey`). Crucially, with `clippy -D warnings` and **no `_` arms over `Lens`/`SubKey`**,
adding a variant turns every site that must handle it into a compile error — a compiler-enforced
checklist. (The genuine plug-in seam is the **collectors**, which are independent modules.)

## Lazy computation & caching

- Each lens with a layer tracks state via `Layer<T>` = `NotComputed` → `Computing` → `Ready(Box<[T]>)`.
- Activating a lens whose layer is `NotComputed` sets `App.pending_compute`; the event loop drains
  it and spawns `collect::compute(lens, root, files)` on a blocking thread.
- The result (`LayerResult`) returns over an `mpsc` channel; `Loaded::apply_layer` aggregates and
  caches it, then re-sorts if it's the active lens.
- `Size` needs no layer — it reads `TreeNode.bytes` directly, so it's instant.
- The spinner ticks while any layer is `Computing`; an idle, fully-loaded tree consumes no CPU.

## Line facets (code lens)

A **facet** (`model::facet::Facet`) is a kind of line the code lens can hide. There are two sorts:

- **Tags** (test, generated, bench/example) mark *which lines*. The collector files each line under
  the `TagSet` it carries, and `CodeData` keeps per language a `TagBuckets` (a sparse map from tag
  set to `CodeNum`, at most eight entries, usually one). `aggregate_code` merges buckets up the
  tree, so every directory holds its subtree's lines split by tags.
- **Kinds** (comment, blank) mark *which column* to leave out of `lines`. Their own columns keep
  showing them, struck through in the header.

Hidden facets are view state (`Loaded::hidden`). They are set at startup by `--hide`, and toggled
on the code lens by key or by clicking a header chip. A toggle only re-sorts and rebuilds the rows
(bumping `rebuild_rev`). It **never invalidates or recomputes the code layer**, because the layer
already caches every bucket.

The chips shrink as the header narrows (`ui::header::fit_chips`): every chip labelled, then only
the hidden ones labelled, then bare keys. While a facet is hidden its chip never disappears: when
even the keys do not fit, they replace the recap's trailing fields, cut at a field boundary.

`Loaded::value` is the single source of the numeric values. Every code column drops hidden tags,
and `lines` also drops hidden kinds. Share, sort, `z`, exclusion and the header totals all read
through it. `CodeData::counted_langs` applies the same rule per language, for the legend and the
header's language count.

A facet whose lines the collector cannot produce yet is not *live* (`Facet::is_live`). It has no
key, no chip, and no `--hide` value. Turning one on takes a detector that files lines under its
tag, plus flipping its `is_live` arm.

### Detecting tags (`collect::tags`)

`tags::Detector::tag` turns a file into a `Tagging`: tags on every line (`whole`), and tags on row
ranges (`regions`). The code collector then counts each run of equally-tagged lines separately
(`code::count_tagged`) and files that run under its tag set. Run boundaries are item boundaries, so
tokei usually starts each run outside a comment or string. The exception is a block comment or
string opened after an item, on that item's last line: there the seam can move lines between code
and comments. Line and blank totals never change.

**Test** code is found two ways:

- **By path** (`tags::paths`), for whole files: test directories (`tests`, `test`, `__tests__`,
  `spec`, `testdata`) and the runners' file-name patterns (`*_test.go`, `test_*.py`, `*.spec.ts`,
  `*Test.java`, …). A test path skips parsing. For Rust and Zig only Cargo's `tests/` counts,
  because a `pub mod test` or `mod spec` under `src/` is as likely shipped code, and those files
  are read in-file instead.
- Before parsing, a file must contain `test` (or `quickcheck`, the one marker without it), and be
  at most 1 MiB. Most source files fail the first check, and huge files are generated bindings.
- **In the file**, for Rust and Zig, through tree-sitter via karet's parse host. These are the
  languages that keep tests next to the code.
  - In Rust, an outer attribute marks its item when it is a test-harness attribute (`#[test]`,
    `#[…::test]`, `#[rstest]`, …), or a `cfg` whose predicate *requires* `test` (`tags::attr`).
  - The attribute pairs with the first item after it, but only if nothing besides whitespace,
    comments, and attributes lies between them. So an attribute on a field or a statement pairs
    with nothing.
  - The region also takes in attributes and comments attached directly above (no blank line
    between, each starting its own line).
  - `#![cfg(test)]` marks the enclosing item, or the whole file.
  - A top-level `#[cfg(test)] mod name;` names files (`name.rs`, or anything under `name/` in the
    declaring file's module directory). A second pass over the collected files tags those whole.
  - In Zig, `test "…" { }` blocks are marked.

**Generated** files are tagged whole (`tags::generated`). `.gitattributes` decides when it says
anything, including an explicit *unset* (`-linguist-generated`), which overrides the other checks
as it does on GitHub.
Otherwise a lockfile name or a header marker marks the file.

- **`.gitattributes`.** `linguist-generated`, or `=` any value but `false`, marks the file.
  `-linguist-generated` or `=false` unmarks it. `!linguist-generated` leaves it unspecified, as in
  git: it clears what earlier lines said, and the other checks decide. Each line's pattern is
  compiled with `ignore`'s gitignore matcher. The deepest file with a matching line decides, and
  within a file the last matching line decides.
  Directory patterns (`vendor/`) mark no files, as in git; `vendor/**` does.
- **Lockfile name.** `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `Cargo.lock`, `go.sum`,
  and others. tokei counts only some of them; the rest never reach the lens.
- **Header marker.** One of the first 10 lines is a comment line containing (case-insensitively)
  `@generated`, `DO NOT EDIT`, `auto-generated`/`autogenerated`/`auto generated`,
  `automatically generated`, or `this file is/was/has been generated`. In Go the window is
  instead every line before `package`.
  - A comment line starts with one of the language's comment markers, or with `*` in a language
    with block comments. It also counts if it sits inside a block comment or doc string opened on
    an earlier header line.
  - In a language with no comment syntax (JSON), any line counts.
  - Prose formats (tokei's literate languages, such as Markdown) are never read: their text talks
    about generated code without being generated.

A file can carry several tags (a generated test file). It is hidden once by either facet.

Known gaps (generated detection):

- Only `.gitattributes` inside the scan root is read. Scanning a subdirectory of a repository
  misses the repository's root rules, and a nested repository inherits its parent's.
- Attribute macros (`[attr]gen linguist-generated`), quoted patterns, and `core.ignorecase` are not
  supported.
- Output with no header and no attribute (GraphQL codegen's default, Django migrations) is not
  detected.

Known gaps (test detection):

- `#[path]` modules, and `mod` declarations nested in inline modules.
- Crate roots other than `lib.rs`/`main.rs`/`mod.rs` (`src/bin/x.rs`).
- Doctests, which stay comments.
- Tests generated by macros (`proptest! { #[test] … }` outside a `cfg(test)` module).
- Test functions in files not named as tests (Python, Go).
- A line shared by a test item and shipped code counts as test.

## How to add a lens

1. Add a variant to `Lens` in `model::lens` and to `Lens::ALL`.
2. Fill the `match` arms the compiler now flags: `label`, `sub_keys`, `has_layer`, `is_available`,
   `columns`, `primary` (and `SubKey::label` for any new sub-keys). A lens's `columns` list runs
   `Rank::Core` first (its own breakdown) then `Rank::Extra`; end it with `FILES_COL`, `SIZE_COL`,
   and `share(<the lens's primary key>)` so every lens carries the universals. Columns drop from
   the right as the pane narrows, and `Extra` ones also yield to the code lens's language legend.
3. If it needs new data: add fields to a per-node data struct (or a new one) in `model::node`, map
   the new `SubKey`s in `Loaded::value` (`app`), and add a collector (below). Add a `LayerResult`
   variant + `Loaded::apply_layer` arm + `Layer` field on `Loaded`.
4. Add any colors the new columns need in `ui::theme` (`Tint` → color).
5. Add tests (the model layer is pure): `value_of` mapping, sorting, aggregation.

## How to add a collector

1. Add a module under `collect/` exposing a function `(root) -> HashMap<PathBuf, T>` (keyed by path
   relative to the scan root). Keep all I/O and external-crate types inside it.
2. Wire it into `collect::compute` for the relevant `Lens` arm.
3. Aggregate its result with the generic `model::aggregate` (any `Copy + Default + AddAssign` type)
   or a dedicated fold if it carries non-`Copy` data (see `aggregate_code`).
4. Keep tests hermetic — unit-test the pure parts (parsing, path mapping), not live I/O.
