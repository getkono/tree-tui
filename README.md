# tree

An interactive, aesthetically-complete terminal UI for [tokei](https://github.com/XAMPPRocky/tokei).
Where `tokei` prints code statistics grouped *by language*, `tree` shows your project as a
navigable **directory tree** with code / comment / blank counts aggregated up every folder — so you
can see *where* the code actually lives.

```
tree <dir>
```

## Features

- **Directory tree of stats** — per-file and per-directory code/comments/blanks/total lines and file
  counts, aggregated bottom-up, powered by tokei's own counting (respects `.gitignore`).
- **Navigate & drill in** — expand/collapse directories, jump to parent/child, page, go to top/bottom.
- **Sort** — by lines, code, comments, blanks, file count, or name; reverse on demand.
- **Filter** — live name filter that reveals matches together with their parent path.
- **Detail panel** — per-language breakdown with proportion bars and percentages, plus a
  code/comment/blank composition bar for the selected node.
- **Language distribution** — a responsive language column that lists each language and its
  percentage (e.g. `Rust (96.5%), C++ (3.4%), Other (0.1%)`), collapsing tail languages into
  `Other` (and finally an `N languages` count) as the terminal narrows.
- **Responsive** — columns drop gracefully as the terminal narrows; works on any Unicode terminal.
- **Async & responsive** — the scan runs off-thread with a spinner; the UI never blocks.

## Install

```bash
mise run install   # cargo install --path . --force  →  installs the `tree` binary
```

> **Note:** the binary is named `tree`, so once installed it shadows the classic `tree` command on
> your `PATH`. That's intentional (`tree <dir>` is the spec); rename the binary in `Cargo.toml`
> (`[[bin]]`) if you'd rather keep both.

## Usage

```bash
tree <dir>          # scan <dir> and explore its code statistics
tree -V, --version  # print version + build info (commit, build time, profile, rustc, target)
tree -h, --help     # print usage
```

The syntax is strict: exactly one directory, no unknown flags. Anything else prints usage and exits 2.

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
| `s` | cycle the sort column |
| `r` | reverse the sort order |
| `d` / `Tab` | toggle the detail panel |
| `/` | filter by name (`Esc` clears) |
| `?` | toggle help |
| `q` / `Ctrl-c` | quit |

### Logging

The TUI owns the terminal, so logs go to a file and only when asked. Set `TREE_LOG=path.log`
(and optionally `RUST_LOG=debug`) to enable file logging.

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

## How it works

`tree` calls `tokei::Languages::get_statistics` on a blocking task, then folds the per-file
`Report`s into an arena-backed directory tree — merging reports that share a path across languages
(embedded languages) and aggregating stats up to the root. The view layer sorts siblings (stable,
total order) and flattens the visible nodes into rows for a ratatui `Table`. The event loop is a
`tokio::select!` over crossterm input, the scan result, and a spinner tick, redrawing only on change.

## Tech Stack

- **Language:** Rust (edition 2024) · **Counting:** tokei (as a library)
- **TUI:** ratatui + crossterm · **Async:** tokio
- **Errors:** color-eyre / eyre, thiserror · **Logging:** tracing + tracing-appender
- **Tooling & tasks:** mise · **Git hooks:** hk

## Git Hooks

This project uses [hk](https://hk.jdx.dev). The pre-commit hook auto-fixes formatting and Clippy
lints on staged Rust files and re-stages them; the pre-push hook runs format checks, Clippy (deny
warnings), and the test suite. Run `hk install` once after cloning to activate them.

## CI/CD

GitHub Actions runs format checks, Clippy, and tests on pushes to `master` and pull requests.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
