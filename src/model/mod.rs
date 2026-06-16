//! Directory-tree data model: a metric-agnostic skeleton plus lazily-computed,
//! cached per-lens metric layers.

mod build;
mod lens;
mod node;
pub mod view;

pub use build::{aggregate, aggregate_code, build_skeleton};
pub use lens::{ColumnSpec, Lens, SubKey, Tint};
pub use node::{
    ChurnData, CodeData, CodeNum, Layer, NodeId, NodeKind, SortDir, StatusData, Tree, TreeNode,
};
