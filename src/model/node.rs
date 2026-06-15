//! Core tree types: nodes, aggregated stats, and sort keys.

use std::collections::BTreeMap;
use std::ops::AddAssign;
use std::path::PathBuf;

use tokei::LanguageType;

/// Index into [`Tree::nodes`]. The arena is never reordered (only children
/// lists are), so an id is a stable identity used to track selection.
pub type NodeId = usize;

/// Aggregated line counts plus a file tally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub code: usize,
    pub comments: usize,
    pub blanks: usize,
    pub files: usize,
}

impl Stats {
    /// Total physical lines (code + comments + blanks).
    pub fn lines(&self) -> usize {
        self.code + self.comments + self.blanks
    }
}

impl AddAssign for Stats {
    fn add_assign(&mut self, rhs: Self) {
        self.code += rhs.code;
        self.comments += rhs.comments;
        self.blanks += rhs.blanks;
        self.files += rhs.files;
    }
}

/// Whether a node is a directory or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
}

/// A single directory or file in the tree.
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
    /// Aggregated for directories; the file's own counts for files.
    pub stats: Stats,
    /// Per-language breakdown (aggregated for dirs, per-file for files).
    pub langs: BTreeMap<LanguageType, Stats>,
    /// The dominant language of a file (by line count).
    pub primary_lang: Option<LanguageType>,
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
            stats: Stats::default(),
            langs: BTreeMap::new(),
            primary_lang: None,
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
}

impl Tree {
    /// Grand totals (the root's aggregated stats).
    pub fn totals(&self) -> Stats {
        self.nodes[self.root].stats
    }
}

/// The metric used to order siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Lines,
    Code,
    Comments,
    Blanks,
    Files,
    Name,
}

impl SortKey {
    /// The next key in the cycle (driven by the `s` key).
    pub fn next(self) -> Self {
        match self {
            SortKey::Lines => SortKey::Code,
            SortKey::Code => SortKey::Comments,
            SortKey::Comments => SortKey::Blanks,
            SortKey::Blanks => SortKey::Files,
            SortKey::Files => SortKey::Name,
            SortKey::Name => SortKey::Lines,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Lines => "lines",
            SortKey::Code => "code",
            SortKey::Comments => "comments",
            SortKey::Blanks => "blanks",
            SortKey::Files => "files",
            SortKey::Name => "name",
        }
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
