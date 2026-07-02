# tree-tui

A ratatui-based terminal **directory visualizer**: one navigable tree, viewed through swappable
**lenses** (code lines, on-disk size, git churn, git status). See `docs/ARCHITECTURE.md` for the
full design.

## Packages

- **ratatui** — TUI widget, layout, and rendering framework (the app's view layer).
- **crossterm** — terminal backend: raw mode, input events, alternate screen.
- **tokio** — async runtime for the input/event loop and concurrent I/O.
- **ignore** — `.gitignore`-aware filesystem walk; builds the tree skeleton + per-file size.
- **tokei** — code line counting (the `code` lens collector).
- **gix** — pure-Rust git; the `churn`/`status` lens collectors.
- **karet-fileview** — read-only "render any file" widget (tree-sitter-highlighted code, inline
  images, hex dumps, placeholders) behind one dispatch; powers the preview pane and the
  full-screen reader (`src/ui/{preview,reader}.rs`). Git-pinned until the karet chain is on
  crates.io — see the dependency note in `Cargo.toml`.
- **color-eyre** / **eyre** — colorful error reports and panic hooks at the app boundary.
- **thiserror** — derive typed error enums for the app's own error types.
- **tracing** + **tracing-subscriber** — structured logging; pair with **tracing-appender**
  to log to a file, since the TUI owns stdout.

## Architecture

A shared, metric-agnostic core + modular per-tool pieces. The full design, data flow, and recipes
for adding a lens or collector are in **`docs/ARCHITECTURE.md`**. Key modules:

- `model::node` — arena `Tree`/`TreeNode` skeleton, cached `Layer<T>`, per-lens data structs.
- `model::lens` — the `Lens`/`SubKey` enums (what each lens shows and how).
- `model::build` — `build_skeleton` + the bottom-up `aggregate` folds.
- `collect/` — the modular collectors (`walk`, `code`, `git`) and the lazy `compute` entry point.
- `app` / `event` — state + reducer, and the lazy-compute engine (request → background collector →
  cache).

Metrics are computed **lazily** (the first time a lens is opened, on a blocking thread) and **cached**
for the session — only the cheap walk runs at startup.

## Quality

Validate changes:

```bash
mise run test        # correctness
mise run fmt-check   # formatting
mise run lint        # Clippy (deny warnings)
```

Or run all three with `mise run check`. Hooks (via hk) and CI run the same `mise` tasks,
so anything green locally is green in CI.

## Conventions

- Edition 2024. Keep `cargo clippy --all-targets -- -D warnings` clean — warnings are errors.
- The TUI owns stdout/stderr: route all diagnostics through `tracing` (to a file via
  tracing-appender), never `println!`/`eprintln!` once the alternate screen is active.
- Restore the terminal (leave raw mode + alternate screen) on every exit path, including
  panics. `ratatui::init()` already installs a terminal-restoring panic hook, so just call
  `color_eyre::install()` **first** and `ratatui::init()` after — ratatui chains the prior
  hook, so the terminal is restored before color-eyre prints its report. Pair every init with
  `ratatui::restore()` on the way out (see `src/tui.rs`, `src/main.rs`).
- Use typed errors (`thiserror`) inside modules; use `color_eyre::Result` at the application
  boundary (`main`, top-level handlers).
