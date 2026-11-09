//! Core tree types: the metric-agnostic skeleton plus the per-lens metric
//! payloads that ride alongside it.
//!
//! The tree skeleton ([`TreeNode`]/[`Tree`]) carries only what is cheap to learn
//! from a single filesystem walk: structure, on-disk `bytes`, and a `files`
//! tally. Everything more expensive (code lines, git churn, git status) is a
//! [`Layer`] computed lazily and cached by the application, never on the node.

use std::collections::BTreeMap;
use std::ops::AddAssign;
use std::path::PathBuf;

use tokei::LanguageType;

use super::facet::{Hidden, TagBuckets, TagSet};

/// Index into [`Tree::nodes`]. The arena is never reordered (only children
/// lists are), so an id is a stable identity used to track selection.
pub type NodeId = usize;

/// Whether a node is a directory or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
}

/// A single directory or file in the tree skeleton.
///
/// `bytes` and `files` are always populated (they come from the walk and are
/// aggregated bottom-up); all other metrics live in lazily-computed [`Layer`]s
/// keyed by this node's [`NodeId`].
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// The final path component (display name).
    pub name: String,
    /// Path relative to the root; unique, used as a stable tie-break/identity.
    pub rel_path: PathBuf,
    pub kind: NodeKind,
    /// Distance from the root (root = 0).
    pub depth: usize,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub expanded: bool,
    /// On-disk size in bytes (aggregated for directories).
    pub bytes: u64,
    /// File count (1 for a file; aggregated descendant count for a directory).
    pub files: usize,
}

impl TreeNode {
    pub fn dir(name: String, rel_path: PathBuf, parent: Option<NodeId>, depth: usize) -> Self {
        Self {
            name,
            rel_path,
            kind: NodeKind::Dir,
            depth,
            parent,
            children: Vec::new(),
            expanded: false,
            bytes: 0,
            files: 0,
        }
    }

    pub fn file(name: String, rel_path: PathBuf, parent: Option<NodeId>, depth: usize) -> Self {
        Self {
            kind: NodeKind::File,
            ..Self::dir(name, rel_path, parent, depth)
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }
}

/// An arena-backed directory tree. The root node's `name` is the display label.
#[derive(Debug, Clone)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
    pub root: NodeId,
    /// Maps a relative path to its node id, so collector results (keyed by path)
    /// can be merged into the arena in O(1) per file.
    pub index: std::collections::HashMap<PathBuf, NodeId>,
}

/// A lazily-computed, cached per-lens metric layer, indexed by [`NodeId`].
///
/// `Ready` holds one value per node, already aggregated bottom-up. A layer stays
/// `NotComputed` until its lens is first opened, becomes `Computing` while a
/// background collector runs, then `Ready` for the rest of the session.
#[derive(Debug, Clone, Default)]
pub enum Layer<T> {
    #[default]
    NotComputed,
    Computing,
    Ready(Box<[T]>),
}

impl<T> Layer<T> {
    /// The aggregated values, once computed.
    pub fn ready(&self) -> Option<&[T]> {
        match self {
            Layer::Ready(values) => Some(values),
            _ => None,
        }
    }

    pub fn is_computing(&self) -> bool {
        matches!(self, Layer::Computing)
    }
}

/// Code line counts (the tokei lens), summed for directories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeNum {
    pub code: usize,
    pub comments: usize,
    pub blanks: usize,
}

impl CodeNum {
    /// Total physical lines (code + comments + blanks).
    pub fn lines(&self) -> usize {
        self.code + self.comments + self.blanks
    }
}

impl AddAssign for CodeNum {
    fn add_assign(&mut self, rhs: Self) {
        self.code += rhs.code;
        self.comments += rhs.comments;
        self.blanks += rhs.blanks;
    }
}

/// A node's code layer entry: line counts per language, each split by the
/// tags its lines carry, and (for files) the language that colors its glyph.
///
/// The totals are derived rather than stored: a node holds a few languages at
/// most, and one source of truth is what keeps the `lines` column, the
/// language legend and the header in agreement under every facet filter.
#[derive(Debug, Clone, Default)]
pub struct CodeData {
    /// Per-language, per-tag-set line counts (aggregated for dirs).
    pub langs: BTreeMap<LanguageType, TagBuckets>,
    /// The language a file was detected as (files only).
    pub primary_lang: Option<LanguageType>,
}

impl CodeData {
    /// A file's entry from its per-language counts, none of them tagged.
    pub fn untagged(
        langs: impl IntoIterator<Item = (LanguageType, CodeNum)>,
        primary_lang: Option<LanguageType>,
    ) -> Self {
        Self {
            langs: langs
                .into_iter()
                .map(|(lang, num)| (lang, TagBuckets::of(TagSet::EMPTY, num)))
                .collect(),
            primary_lang,
        }
    }

    /// Every line, whatever its tags.
    pub fn total(&self) -> CodeNum {
        self.visible(TagSet::EMPTY)
    }

    /// The lines carrying none of the `hidden` tags.
    pub fn visible(&self, hidden: TagSet) -> CodeNum {
        self.langs
            .values()
            .fold(CodeNum::default(), |mut acc, buckets| {
                acc += buckets.visible(hidden);
                acc
            })
    }

    /// Per language, the lines that count toward `lines` under `hidden`: the
    /// hidden tags dropped and the hidden kinds zeroed. Languages left with no
    /// lines are omitted.
    pub fn counted_langs(&self, hidden: Hidden) -> BTreeMap<LanguageType, CodeNum> {
        self.langs
            .iter()
            .map(|(&lang, buckets)| (lang, hidden.counted(buckets.visible(hidden.tags()))))
            .filter(|(_, num)| num.lines() > 0)
            .collect()
    }

    /// Add `tags` to every line of this entry.
    pub fn retag(&mut self, tags: TagSet) {
        for buckets in self.langs.values_mut() {
            buckets.retag(tags);
        }
    }

    /// Add `other`'s lines to this entry (the bottom-up fold).
    pub fn merge(&mut self, other: &CodeData) {
        for (&lang, buckets) in &other.langs {
            self.langs.entry(lang).or_default().merge(buckets);
        }
    }
}

/// Git churn over a window of history: lines added/deleted and how many commits
/// touched a path. Summed for directories (see the `commits` caveat in
/// `collect::git`: a directory total double-counts commits touching siblings, so
/// it is a churn *weight*, not a precise commit count).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChurnData {
    pub added: u64,
    pub deleted: u64,
    pub commits: u64,
}

impl ChurnData {
    /// Combined churn weight (added + deleted lines).
    pub fn churn(&self) -> u64 {
        self.added + self.deleted
    }
}

impl AddAssign for ChurnData {
    fn add_assign(&mut self, rhs: Self) {
        self.added += rhs.added;
        self.deleted += rhs.deleted;
        self.commits += rhs.commits;
    }
}

/// Git working-tree status counts. Summed for directories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusData {
    /// New (untracked or added-to-index) files.
    pub added: usize,
    /// Modified / renamed / type-changed files.
    pub modified: usize,
    /// Deleted files.
    pub deleted: usize,
}

impl StatusData {
    /// All working-tree changes in this subtree.
    pub fn total(&self) -> usize {
        self.added + self.modified + self.deleted
    }
}

impl AddAssign for StatusData {
    fn add_assign(&mut self, rhs: Self) {
        self.added += rhs.added;
        self.modified += rhs.modified;
        self.deleted += rhs.deleted;
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn flip(self) -> Self {
        match self {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            SortDir::Asc => "↑",
            SortDir::Desc => "↓",
        }
    }
}
