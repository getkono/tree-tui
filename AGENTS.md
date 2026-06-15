# tree-tui

A ratatui-based terminal user interface (TUI).

## Packages

- **ratatui** — TUI widget, layout, and rendering framework (the app's view layer).
- **crossterm** — terminal backend: raw mode, input events, alternate screen.
- **tokio** — async runtime for the input/event loop and concurrent I/O.
- **color-eyre** / **eyre** — colorful error reports and panic hooks at the app boundary.
- **thiserror** — derive typed error enums for the app's own error types.
- **tracing** + **tracing-subscriber** — structured logging; pair with **tracing-appender**
  to log to a file, since the TUI owns stdout.

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
