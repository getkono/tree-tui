//! Lenses: the swappable "tools" the tree is viewed through.
//!
//! A [`Lens`] selects *which* metric drives the view and *how* it is presented
//! (columns, the primary value, the sortable sub-keys). It is an exhaustive enum
//! on purpose: with `clippy -D warnings` and no `_` arms, adding a variant turns
//! every site that must handle it into a compile error — a checklist for adding a
//! tool. The data each lens reads lives in a [`super::Layer`]; the wiring that
//! resolves a [`SubKey`] to a value is in `app` (it owns the layers).

/// The active tool. Cycled with `m`, jumped to with the digit keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lens {
    Code,
    Size,
    Churn,
    Status,
}

/// A sortable / displayable scalar. Not every lens exposes every key;
/// [`Lens::sub_keys`] is the source of truth, and `app` maps each key to the
/// node field or cached layer it reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubKey {
    // code
    Lines,
    Code,
    Comments,
    Blanks,
    // universal
    Files,
    Name,
    // size
    Bytes,
    // churn
    Added,
    Deleted,
    Churn,
    Commits,
    // status
    StatusAdded,
    StatusModified,
    StatusDeleted,
    StatusTotal,
}

/// A theme-agnostic color tag for a column, resolved to a real color by
/// `ui::theme` (so this module needs no `ratatui` types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Code,
    Comments,
    Blanks,
    Size,
    Add,
    Del,
    Status,
    /// No color (rendered bold/default — used for the primary total).
    Plain,
}

/// How a column renders its key's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFormat {
    /// The value itself: a thousands-grouped count, or human bytes for a byte
    /// key.
    Value,
    /// The value as a percentage of the root's total for the same key.
    Share,
}

/// How hard a column fights for width.
///
/// `Core` columns are the lens's own breakdown — the reason you opened it.
/// `Extra` columns are the universals every lens carries (`files`, `size`,
/// `share`) plus per-lens extras that are context rather than headline; they
/// yield to the code lens's language legend before the legend yields to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Core,
    Extra,
}

/// One numeric column in the tree table (right-aligned).
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpec {
    pub header: &'static str,
    pub key: SubKey,
    pub tint: Tint,
    pub format: ColumnFormat,
    pub rank: Rank,
}

/// The universals every lens ends with: the file count and on-disk size of the
/// row, then its share of the whole tree under that lens's primary key. They
/// are what the row would otherwise only say in a side panel, and they are the
/// first columns to go when the terminal narrows.
const fn extra(header: &'static str, key: SubKey, tint: Tint, format: ColumnFormat) -> ColumnSpec {
    ColumnSpec {
        header,
        key,
        tint,
        format,
        rank: Rank::Extra,
    }
}

const fn core(header: &'static str, key: SubKey, tint: Tint) -> ColumnSpec {
    ColumnSpec {
        header,
        key,
        tint,
        format: ColumnFormat::Value,
        rank: Rank::Core,
    }
}

const FILES_COL: ColumnSpec = extra("files", SubKey::Files, Tint::Plain, ColumnFormat::Value);
const SIZE_COL: ColumnSpec = extra("size", SubKey::Bytes, Tint::Size, ColumnFormat::Value);

/// The `share` column for a lens, as a percentage of the root's total under
/// `key` — the lens's own primary key, so `share` always answers "how much of
/// the number in the header is this row?".
const fn share(key: SubKey) -> ColumnSpec {
    extra("share", key, Tint::Plain, ColumnFormat::Share)
}

// Core columns first, then the extras — `Columns::choose` drops from the right,
// so the supplementary numbers go before a lens loses its own breakdown.
const CODE_COLS: &[ColumnSpec] = &[
    core("code", SubKey::Code, Tint::Code),
    core("comments", SubKey::Comments, Tint::Comments),
    core("blanks", SubKey::Blanks, Tint::Blanks),
    FILES_COL,
    SIZE_COL,
    share(SubKey::Lines),
];
const SIZE_COLS: &[ColumnSpec] = &[FILES_COL, share(SubKey::Bytes)];
const CHURN_COLS: &[ColumnSpec] = &[
    core("added", SubKey::Added, Tint::Add),
    core("deleted", SubKey::Deleted, Tint::Del),
    extra("commits", SubKey::Commits, Tint::Plain, ColumnFormat::Value),
    FILES_COL,
    SIZE_COL,
    share(SubKey::Churn),
];
const STATUS_COLS: &[ColumnSpec] = &[
    core("added", SubKey::StatusAdded, Tint::Add),
    core("modified", SubKey::StatusModified, Tint::Status),
    core("deleted", SubKey::StatusDeleted, Tint::Del),
    FILES_COL,
    SIZE_COL,
    share(SubKey::StatusTotal),
];

const CODE_KEYS: &[SubKey] = &[
    SubKey::Lines,
    SubKey::Code,
    SubKey::Comments,
    SubKey::Blanks,
    SubKey::Files,
    SubKey::Name,
];
const SIZE_KEYS: &[SubKey] = &[SubKey::Bytes, SubKey::Files, SubKey::Name];
const CHURN_KEYS: &[SubKey] = &[
    SubKey::Churn,
    SubKey::Added,
    SubKey::Deleted,
    SubKey::Commits,
    SubKey::Files,
    SubKey::Name,
];
const STATUS_KEYS: &[SubKey] = &[SubKey::StatusTotal, SubKey::Files, SubKey::Name];

impl Lens {
    /// Every lens, in cycle order.
    pub const ALL: [Lens; 4] = [Lens::Code, Lens::Size, Lens::Churn, Lens::Status];

    /// The next lens in the cycle (ignores availability; callers skip).
    pub fn next(self) -> Lens {
        let i = Self::ALL.iter().position(|&l| l == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            Lens::Code => "code",
            Lens::Size => "size",
            Lens::Churn => "churn",
            Lens::Status => "status",
        }
    }

    /// The sort sub-keys this lens offers (first = its default), cycled by `s`.
    /// Always ends with `Files` and `Name`, so they are reachable everywhere.
    pub fn sub_keys(self) -> &'static [SubKey] {
        match self {
            Lens::Code => CODE_KEYS,
            Lens::Size => SIZE_KEYS,
            Lens::Churn => CHURN_KEYS,
            Lens::Status => STATUS_KEYS,
        }
    }

    pub fn default_sub_key(self) -> SubKey {
        self.sub_keys()[0]
    }

    /// The next sort sub-key within this lens, wrapping.
    pub fn next_sub_key(self, current: SubKey) -> SubKey {
        let keys = self.sub_keys();
        let i = keys.iter().position(|&k| k == current).unwrap_or(0);
        keys[(i + 1) % keys.len()]
    }

    /// Whether this lens reads a lazily-computed layer (and so must be computed
    /// before its data appears). `Size` reads the always-present node bytes.
    pub fn has_layer(self) -> bool {
        match self {
            Lens::Size => false,
            Lens::Code | Lens::Churn | Lens::Status => true,
        }
    }

    /// Whether this lens has data to show for the current tree. Git lenses need a
    /// repository.
    pub fn is_available(self, repo: bool) -> bool {
        match self {
            Lens::Code | Lens::Size => true,
            Lens::Churn | Lens::Status => repo,
        }
    }

    /// Optional numeric columns (besides name, the language legend, and the
    /// always-present primary column).
    ///
    /// Every list must run its [`Rank::Core`] columns before its
    /// [`Rank::Extra`] ones. `ui::tree_view` drops from the right and counts
    /// the cores to decide how much width yielding the extras could free;
    /// interleaving them would stop that count at the last core and spend
    /// columns on a legend it then could not show.
    pub fn columns(self) -> &'static [ColumnSpec] {
        match self {
            Lens::Code => CODE_COLS,
            Lens::Size => SIZE_COLS,
            Lens::Churn => CHURN_COLS,
            Lens::Status => STATUS_COLS,
        }
    }

    /// The always-present primary column: the headline value, also used for the
    /// per-row bar and the declutter zero-test.
    pub fn primary(self) -> ColumnSpec {
        match self {
            Lens::Code => core("lines", SubKey::Lines, Tint::Plain),
            Lens::Size => core("size", SubKey::Bytes, Tint::Size),
            Lens::Churn => core("churn", SubKey::Churn, Tint::Plain),
            Lens::Status => core("changes", SubKey::StatusTotal, Tint::Plain),
        }
    }
}

impl SubKey {
    pub fn label(self) -> &'static str {
        match self {
            SubKey::Lines => "lines",
            SubKey::Code => "code",
            SubKey::Comments => "comments",
            SubKey::Blanks => "blanks",
            SubKey::Files => "files",
            SubKey::Name => "name",
            SubKey::Bytes => "size",
            SubKey::Added => "added",
            SubKey::Deleted => "deleted",
            SubKey::Churn => "churn",
            SubKey::Commits => "commits",
            SubKey::StatusAdded => "added",
            SubKey::StatusModified => "modified",
            SubKey::StatusDeleted => "deleted",
            SubKey::StatusTotal => "changes",
        }
    }

    /// Whether values for this key are sizes in bytes (formatted human-readably)
    /// rather than plain counts.
    pub fn is_bytes(self) -> bool {
        matches!(self, SubKey::Bytes)
    }
}
