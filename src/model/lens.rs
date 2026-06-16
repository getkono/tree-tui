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

/// One numeric column in the tree table (right-aligned).
#[derive(Debug, Clone, Copy)]
pub struct ColumnSpec {
    pub header: &'static str,
    pub key: SubKey,
    pub tint: Tint,
}

const CODE_COLS: &[ColumnSpec] = &[
    ColumnSpec {
        header: "code",
        key: SubKey::Code,
        tint: Tint::Code,
    },
    ColumnSpec {
        header: "comments",
        key: SubKey::Comments,
        tint: Tint::Comments,
    },
    ColumnSpec {
        header: "blanks",
        key: SubKey::Blanks,
        tint: Tint::Blanks,
    },
];
const SIZE_COLS: &[ColumnSpec] = &[];
const CHURN_COLS: &[ColumnSpec] = &[
    ColumnSpec {
        header: "added",
        key: SubKey::Added,
        tint: Tint::Add,
    },
    ColumnSpec {
        header: "deleted",
        key: SubKey::Deleted,
        tint: Tint::Del,
    },
];
const STATUS_COLS: &[ColumnSpec] = &[
    ColumnSpec {
        header: "added",
        key: SubKey::StatusAdded,
        tint: Tint::Add,
    },
    ColumnSpec {
        header: "modified",
        key: SubKey::StatusModified,
        tint: Tint::Status,
    },
    ColumnSpec {
        header: "deleted",
        key: SubKey::StatusDeleted,
        tint: Tint::Del,
    },
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
            Lens::Code => ColumnSpec {
                header: "lines",
                key: SubKey::Lines,
                tint: Tint::Plain,
            },
            Lens::Size => ColumnSpec {
                header: "size",
                key: SubKey::Bytes,
                tint: Tint::Size,
            },
            Lens::Churn => ColumnSpec {
                header: "churn",
                key: SubKey::Churn,
                tint: Tint::Plain,
            },
            Lens::Status => ColumnSpec {
                header: "changes",
                key: SubKey::StatusTotal,
                tint: Tint::Plain,
            },
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
