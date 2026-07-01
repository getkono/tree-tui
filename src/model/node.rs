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

impl Tree {
    /// Total on-disk size (the root's aggregated bytes).
    pub fn total_bytes(&self) -> u64 {
        self.nodes[self.root].bytes
    }
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

/// A node's code layer entry: totals, the per-language breakdown, and (for files)
/// the dominant language used to color the file glyph.
#[derive(Debug, Clone, Default)]
pub struct CodeData {
    pub num: CodeNum,
    /// Per-language line counts (aggregated for dirs, per-file for files).
    pub langs: BTreeMap<LanguageType, CodeNum>,
    /// The dominant language of a file by line count (files only).
    pub primary_lang: Option<LanguageType>,
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
