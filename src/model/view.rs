//! View-layer transforms over a built [`Tree`]: sorting and flattening.
//!
//! Sorting is driven by a precomputed per-node value slice (indexed by
//! [`NodeId`]) rather than reading the node directly, so the same routine serves
//! every lens — the caller decides what each value means (see `app`).

use std::cmp::Ordering;

use super::node::{NodeId, NodeKind, SortDir, Tree, TreeNode};

/// Order every node's children by `values[id]` (with a stable name/path
/// tie-break). `values` must have one entry per node.
pub fn sort_by_values(tree: &mut Tree, values: &[u128], dir: SortDir) {
    sort_children(tree, |a, b, nodes| {
        let primary = values[a].cmp(&values[b]);
        orient(primary, dir).then_with(|| tie_break(&nodes[a], &nodes[b]))
    });
}

/// Order every node's children alphabetically (case-insensitive), then by path.
pub fn sort_by_name(tree: &mut Tree, dir: SortDir) {
    sort_children(tree, |a, b, nodes| {
        let primary = name_key(&nodes[a]).cmp(&name_key(&nodes[b]));
        orient(primary, dir).then_with(|| nodes[a].rel_path.cmp(&nodes[b].rel_path))
    });
}

/// Shared driver: take each node's children out so the comparator can borrow the
/// arena, sort, then put them back.
fn sort_children(tree: &mut Tree, cmp: impl Fn(NodeId, NodeId, &[TreeNode]) -> Ordering) {
    for id in 0..tree.nodes.len() {
        let mut children = std::mem::take(&mut tree.nodes[id].children);
        children.sort_by(|&a, &b| cmp(a, b, &tree.nodes));
        tree.nodes[id].children = children;
    }
}

fn orient(ordering: Ordering, dir: SortDir) -> Ordering {
    match dir {
        SortDir::Desc => ordering.reverse(),
        SortDir::Asc => ordering,
    }
}

fn name_key(node: &TreeNode) -> String {
    node.name.to_lowercase()
}

fn tie_break(a: &TreeNode, b: &TreeNode) -> Ordering {
    name_key(a)
        .cmp(&name_key(b))
        .then_with(|| a.rel_path.cmp(&b.rel_path))
}

/// Depth-first list of currently visible nodes — the root's descendants,
/// honoring each directory's `expanded` flag. The root itself is not emitted.
pub fn flatten_visible(tree: &Tree) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = tree.nodes[tree.root]
        .children
        .iter()
        .rev()
        .copied()
        .collect();
    while let Some(id) = stack.pop() {
        out.push(id);
        let node = &tree.nodes[id];
        if node.kind == NodeKind::Dir && node.expanded {
            stack.extend(node.children.iter().rev().copied());
        }
    }
    out
}

/// Visible nodes when a case-insensitive name `query` is active.
///
/// Shows the path to every match (ancestors of matches) and the full subtree
/// beneath any matching directory, ignoring the `expanded` flags so matches are
/// always revealed. Falls back to [`flatten_visible`] for an empty query.
pub fn flatten_filtered(tree: &Tree, query: &str) -> Vec<NodeId> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return flatten_visible(tree);
    }

    // `has_match[id]` = this node or any descendant matches. Computed deepest
    // first so children are decided before their parent.
    let mut has_match = vec![false; tree.nodes.len()];
    let mut order: Vec<NodeId> = (0..tree.nodes.len()).collect();
    order.sort_by_key(|&id| std::cmp::Reverse(tree.nodes[id].depth));
    for id in order {
        let own = tree.nodes[id].name.to_lowercase().contains(&query);
        let child = tree.nodes[id].children.iter().any(|&c| has_match[c]);
        has_match[id] = own || child;
    }

    let mut out = Vec::new();
    for &child in &tree.nodes[tree.root].children {
        push_filtered(tree, child, false, &has_match, &query, &mut out);
    }
    out
}

fn push_filtered(
    tree: &Tree,
    id: NodeId,
    under_match: bool,
    has_match: &[bool],
    query: &str,
    out: &mut Vec<NodeId>,
) {
    if !(under_match || has_match[id]) {
        return;
    }
    out.push(id);
    let own = tree.nodes[id].name.to_lowercase().contains(query);
    let child_under = under_match || own;
    for &child in &tree.nodes[id].children {
        push_filtered(tree, child, child_under, has_match, query, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::build::build_skeleton;
    use std::path::PathBuf;

    fn rel(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    fn byte_values(tree: &Tree) -> Vec<u128> {
        tree.nodes.iter().map(|n| n.bytes as u128).collect()
    }

    fn names(tree: &Tree) -> Vec<String> {
        flatten_visible(tree)
            .iter()
            .map(|&id| tree.nodes[id].name.clone())
            .collect()
    }

    #[test]
    fn sort_by_value_desc_then_asc_is_deterministic() {
        let files = vec![(rel("big.rs"), 500), (rel("small.rs"), 5)];
        let mut tree = build_skeleton(&files, &[], "p".into());

        let values = byte_values(&tree);
        sort_by_values(&mut tree, &values, SortDir::Desc);
        assert_eq!(names(&tree), ["big.rs", "small.rs"]);

        sort_by_values(&mut tree, &values, SortDir::Asc);
        assert_eq!(names(&tree), ["small.rs", "big.rs"]);
    }

    #[test]
    fn sort_by_value_ranks_a_big_binary_over_small_source() {
        let files = vec![(rel("main.rs"), 1_200), (rel("logo.png"), 900_000)];
        let mut tree = build_skeleton(&files, &[], "p".into());
        let values = byte_values(&tree);
        sort_by_values(&mut tree, &values, SortDir::Desc);
        assert_eq!(names(&tree), ["logo.png", "main.rs"]);
    }

    fn nested() -> Tree {
        let files = vec![
            (rel("src/main.rs"), 1),
            (rel("src/lib.rs"), 1),
            (rel("docs/guide.md"), 1),
        ];
        let dirs = vec![rel("src"), rel("docs")];
        build_skeleton(&files, &dirs, "p".into())
    }

    fn filtered_names(tree: &Tree, query: &str) -> Vec<String> {
        flatten_filtered(tree, query)
            .iter()
            .map(|&id| tree.nodes[id].name.clone())
            .collect()
    }

    #[test]
    fn filter_keeps_matches_and_ancestors() {
        let names = filtered_names(&nested(), "lib");
        assert!(names.contains(&"src".to_string()));
        assert!(names.contains(&"lib.rs".to_string()));
        assert!(!names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&"docs".to_string()));
    }

    #[test]
    fn filter_reveals_subtree_of_a_matching_dir() {
        let names = filtered_names(&nested(), "src");
        assert!(names.contains(&"src".to_string()));
        assert!(names.contains(&"main.rs".to_string()));
        assert!(names.contains(&"lib.rs".to_string()));
        assert!(!names.contains(&"docs".to_string()));
    }
}
