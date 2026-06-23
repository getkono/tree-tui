# tree

An interactive terminal directory visualizer for large, polyglot repos. `tree` walks a directory
once into a navigable **directory tree**, then lets you view it through swappable **lenses** — lines
of code, on-disk size, git churn, git working-tree status — each aggregated up every folder so you
can see *where* things actually concentrate.

```
tree [dir]   # dir defaults to the current directory
```

Press `m` to cycle lenses (or `1`–`4` to jump). Every file shows up — source, binaries, images,
lockfiles — not just code, so the size and git lenses are meaningful too.

## Lenses

| Key | Lens | Shows |
| --- | --- | --- |
| `1` | **code** | lines of code / comments / blanks, with a per-language breakdown (via tokei) |
| `2` | **size** | on-disk size in bytes, human-readable — find what's bloating the repo |
| `3` | **churn** | lines added/deleted and how often files change, over recent git history |
| `4` | **status** | uncommitted working-tree changes (added / modified / deleted), rolled up per folder |

The git lenses (`churn`, `status`) appear only inside a git repository; elsewhere they're skipped
when cycling.

### Lazy + cached

Opening a directory does only the cheap filesystem walk (structure + size), so it's instant even on
huge trees. Each lens's data is computed the **first time you open that lens**, on a background
thread (a brief `computing …` shows in the footer), then **cached** for the session — switching back
is instant, and you never pay for git history unless you ask for it.

## Features

- **One tree, many lenses** — the same directory tree, re-measured on demand; switch with a keypress.
- **Everything appears** — an `ignore`-based walk (honoring `.gitignore`) lists all files, not only
  code, so size and git data have something to attach to.
- **Aggregated bottom-up** — every directory totals its subtree under the active lens.
- **Navigate & drill in** — expand/collapse, jump to parent/child, page, go to top/bottom.
- **Sort** — by the active lens's columns (or by name / file count); reverse on demand.
- **Declutter** — `z` hides rows that are zero under the active lens (e.g. non-code files in `code`).
- **Filter** — live name filter that reveals matches together with their parent path.
- **Detail panel** — a per-lens breakdown for the selected node with proportion bars and percentages.
- **Responsive** — columns drop gracefully as the terminal narrows; works on any Unicode terminal.

## Install

Homebrew (macOS and Linux):

```bash
brew install getkono/tap/tree-tui   # installs the `tree` binary
```

From source (with [mise](https://mise.jdx.dev)):

```bash
mise run install   # cargo install --path . --force  →  installs the `tree` binary
```

> **Note:** the binary is named `tree`, so once installed it shadows the classic `tree` command on
> your `PATH`. That's intentional (`tree [dir]` is the spec); rename the binary in `Cargo.toml`
> (`[[bin]]`) if you'd rather keep both. The Homebrew formula declares `conflicts_with "tree"` for
> the same reason.

## Usage

```bash
tree [dir]          # explore [dir] (default: .) through swappable lenses
tree -V, --version  # print version + build info (commit, build time, profile, rustc, target)
tree -h, --help     # print usage
```

The syntax is strict: at most one directory, no unknown flags. Anything else prints usage and exits 2.

### Keybindings

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | move selection |
| `g` / `G` | jump to top / bottom |
| `Ctrl-d` / `Ctrl-u`, `PgDn` / `PgUp` | page down / up |
| `l` / `→` | expand a directory, or descend into it |
| `Enter` | open the selected file in `$EDITOR` (`$VISUAL`, then `vi`), or expand a directory |
| `h` / `←` | collapse a directory, or jump to its parent |
| `Space` | toggle the selected directory |
| `E` / `C` | expand all / collapse all |
| `m` | cycle the active lens |
| `1` – `4` | jump to a lens (code / size / churn / status) |
| `s` | cycle the sort column (within the lens) |
| `r` | reverse the sort order |
| `z` | hide rows that are zero under the active lens |
| `d` / `Tab` | toggle the detail panel |
| `/` | filter by name (`Esc` clears) |
| `?` | toggle help |
| `q` / `Ctrl-c` | quit |

### Logging

The TUI owns the terminal, so logs go to a file and only when asked. Set `TREE_LOG=path.log`
(and optionally `RUST_LOG=debug`) to enable file logging.

## How it works

`tree` separates a **shared, metric-agnostic core** from **modular per-lens tools**:

1. **Walk** — an `ignore`-based filesystem walk (the same crate tokei uses) builds the arena-backed
   tree skeleton and records each file's size. This runs once, eagerly.
2. **Lenses** — a `Lens` is an exhaustive enum that decides *what* is shown and *how* (columns, the
   primary value, sortable sub-keys). Sorting reads a precomputed per-node value slice, so one
   routine serves every lens.
3. **Collectors** — each expensive metric has an independent collector (`tokei` for code, `gix` for
   churn/status). When a lens is first opened, the event loop runs its collector on a blocking thread
   and reports the per-file result over a channel.
4. **Aggregate + cache** — the result is folded bottom-up into a per-node *layer* and cached; the
   active lens re-sorts and re-renders. The `tokio::select!` event loop redraws only on change.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full design and recipes for adding a lens
or a collector.

## Tech Stack

- **Language:** Rust (edition 2024)
- **TUI:** ratatui + crossterm · **Async:** tokio
- **Walk:** ignore · **Code stats:** tokei · **Git:** gix (pure-Rust)
- **Errors:** color-eyre / eyre, thiserror · **Logging:** tracing + tracing-appender
- **Tooling & tasks:** mise · **Git hooks:** hk

## Development

| Command             | Description                          |
| ------------------- | ------------------------------------ |
| `mise run dev`      | Build and run (`cargo run`)          |
| `mise run install`  | Install the `tree` binary            |
| `mise run test`     | Run the test suite                   |
| `mise run fmt`      | Format code                          |
| `mise run lint`     | Lint with Clippy (deny warnings)     |
| `mise run lint-fix` | Lint and auto-fix                    |
| `mise run check`    | Format check + lint + test           |

### Prerequisites

- [Rust (rustup)](https://rustup.rs) — toolchain, pinned via `rust-toolchain.toml`
- [mise](https://mise.jdx.dev) — manages dev tools and tasks
- [hk](https://hk.jdx.dev) — git hooks manager (`mise install` then `hk install`)

## Git Hooks

This project uses [hk](https://hk.jdx.dev). The pre-commit hook auto-fixes formatting and Clippy
lints on staged Rust files and re-stages them; the pre-push hook runs format checks, Clippy (deny
warnings), and the test suite. Run `hk install` once after cloning to activate them.

## CI/CD

GitHub Actions runs format checks, Clippy, and tests on pushes to `master` and pull requests.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
