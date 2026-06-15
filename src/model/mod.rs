//! Directory-tree data model built from tokei results.

mod build;
mod node;
pub mod view;

pub use build::build_tree;
pub use node::{NodeId, NodeKind, SortDir, SortKey, Stats, Tree, TreeNode};
