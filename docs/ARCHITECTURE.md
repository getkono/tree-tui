# Architecture

`tree` is a directory visualizer: one navigable tree, viewed through swappable **lenses** (code,
size, churn, status). The design separates a **shared, metric-agnostic core** from **modular
per-tool pieces**, and computes expensive metrics **lazily** (on first use) and **caches** them.

## Data flow

```
walk (ignore)  ──►  build_skeleton  ──►  Tree (skeleton + bytes + files, + path index)
                                          │
   open a lens ──► request compute ──► collector (blocking thread) ──► per-file map
                                          │
                          aggregate (bottom-up) ──► cached Layer<T>
                                          │
                       value_of / sort ──► flatten ──► render (ratatui)
```

- The **walk** (`collect::walk`, via the `ignore` crate) runs once at startup and is cheap
  (structure + file sizes, no contents). It yields *every* non-ignored file, so non-code files
  appear too.
- A **lens** is opened on demand. If its data isn't cached, the event loop spawns the lens's
  **collector** on a blocking thread; the result comes back over an `mpsc` channel, is **aggregated**
  bottom-up into a per-node `Layer`, and cached for the session.

## Shared core vs modular tools

**Shared core (metric-agnostic):**

- `model::node` — the arena `Tree` / `TreeNode` skeleton, the always-on `bytes`/`files`, the cached
  `Layer<T>` type, and the per-lens data structs (`CodeData`, `ChurnData`, `StatusData`).
- `model::build` — `build_skeleton` plus the generic bottom-up folds `aggregate` / `aggregate_code`.
- `model::view` — `sort_by_values` / `sort_by_name` and the visible/filtered flattening.
- `app` — state, the reducer, the lazy-compute request/cache wiring, and `Loaded::value` (the one
  place that maps a `SubKey` to the node field or layer it reads).
- `event` — the `tokio::select!` loop: input, walk completion, lens results, spinner ticks.
- `ui` — the render scaffold (header / tree table / detail / footer / help) driven by the active lens.

**Modular tools:**

- **Collectors** (`collect::{walk, code, git}`) — independent data sources keyed by relative path.
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
  it and spawns `collect::compute(lens, root)` on a blocking thread.
- The result (`LayerResult`) returns over an `mpsc` channel; `Loaded::apply_layer` aggregates and
  caches it, then re-sorts if it's the active lens.
- `Size` needs no layer — it reads `TreeNode.bytes` directly, so it's instant.
- The spinner ticks while any layer is `Computing`; an idle, fully-loaded tree consumes no CPU.

## How to add a lens

1. Add a variant to `Lens` in `model::lens` and to `Lens::ALL`.
2. Fill the `match` arms the compiler now flags: `label`, `sub_keys`, `has_layer`, `is_available`,
   `columns`, `primary` (and `SubKey::label` for any new sub-keys).
3. If it needs new data: add fields to a per-node data struct (or a new one) in `model::node`, map
   the new `SubKey`s in `Loaded::value` (`app`), and add a collector (below). Add a `LayerResult`
   variant + `Loaded::apply_layer` arm + `Layer` field on `Loaded`.
4. Add a detail-panel section in `ui::detail` and any colors in `ui::theme` (`Tint` → color).
5. Add tests (the model layer is pure): `value_of` mapping, sorting, aggregation.

## How to add a collector

1. Add a module under `collect/` exposing a function `(root) -> HashMap<PathBuf, T>` (keyed by path
   relative to the scan root). Keep all I/O and external-crate types inside it.
2. Wire it into `collect::compute` for the relevant `Lens` arm.
3. Aggregate its result with the generic `model::aggregate` (any `Copy + Default + AddAssign` type)
   or a dedicated fold if it carries non-`Copy` data (see `aggregate_code`).
4. Keep tests hermetic — unit-test the pure parts (parsing, path mapping), not live I/O.
