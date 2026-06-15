# tree-tui

A ratatui-based terminal user interface (TUI).

## Prerequisites

- [Rust (rustup)](https://rustup.rs) — toolchain, pinned via `rust-toolchain.toml`
- [mise](https://mise.jdx.dev) — manages dev tools and tasks
- [hk](https://hk.jdx.dev) — git hooks manager (installed by `mise install`)

## Quick Start

```bash
mise install   # install pinned tools (hk)
hk install     # activate git hooks (or: mise exec -- hk install)
mise run dev   # build and run the TUI
```

## Development

| Command             | Description                |
| ------------------- | -------------------------- |
| `mise run dev`      | Build and run the TUI      |
| `mise run test`     | Run the test suite         |
| `mise run fmt`      | Format code                |
| `mise run lint`     | Lint with Clippy           |
| `mise run lint-fix` | Lint and auto-fix          |
| `mise run check`    | Format check + lint + test |

## Tech Stack

- **Language:** Rust (edition 2024)
- **TUI:** ratatui + crossterm
- **Async runtime:** tokio
- **Errors:** color-eyre / eyre, thiserror
- **Logging:** tracing + tracing-subscriber
- **Formatter / Linter:** rustfmt + Clippy
- **Tooling & tasks:** mise
- **Git hooks:** hk

## Git Hooks

This project uses [hk](https://hk.jdx.dev). The pre-commit hook auto-fixes formatting
and Clippy lints on staged Rust files and re-stages them; the pre-push hook runs format
checks, Clippy (deny warnings), and the test suite. Run `hk install` once after cloning
to activate them.

## CI/CD

GitHub Actions runs format checks, Clippy, and tests on pushes to `master` and pull requests.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
