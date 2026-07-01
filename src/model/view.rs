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

/// Order every node's children with directories first, then alphabetically
/// (case-insensitive) within each group, then by path. Directories always lead
/// files: only the name ordering flips with `dir`, never the dir/file grouping.
pub fn sort_by_name(tree: &mut Tree, dir: SortDir) {
    sort_children(tree, |a, b, nodes| {
        kind_rank(&nodes[a])
            .cmp(&kind_rank(&nodes[b]))
            .then_with(|| {
                let primary = name_key(&nodes[a]).cmp(&name_key(&nodes[b]));
                orient(primary, dir).then_with(|| nodes[a].rel_path.cmp(&nodes[b].rel_path))
            })
    });
}

/// Sort rank that lifts directories above files, independent of direction.
fn kind_rank(node: &TreeNode) -> u8 {
    match node.kind {
        NodeKind::Dir => 0,
        NodeKind::File => 1,
    }
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

/// The single child of `id` when that child is a directory — the link that
/// folds a sole sub-directory into its parent's display row. `None` if `id` has
/// no children, several children, or a single *file* child.
fn single_dir_child(tree: &Tree, id: NodeId) -> Option<NodeId> {
    match tree.nodes[id].children.as_slice() {
        [only] if tree.nodes[*only].kind == NodeKind::Dir => Some(*only),
        _ => None,
    }
}

/// Whether `id` is a directory folded into its parent's row: the sole child of a
/// *displayed* directory. The root is never displayed, so top-level entries
/// always begin their own row rather than concatenating with the root.
fn is_absorbed(tree: &Tree, id: NodeId) -> bool {
    let node = &tree.nodes[id];
    node.kind == NodeKind::Dir
        && matches!(node.parent, Some(p) if p != tree.root && tree.nodes[p].children.len() == 1)
}

/// The last directory in `head`'s chain of sole sub-directories — the node whose
/// children appear when the chained row is expanded. For a row that is not a
/// chain (a file, or a branching directory) this is `head` itself.
pub fn segment_tail(tree: &Tree, head: NodeId) -> NodeId {
    let mut tail = head;
    while let Some(child) = single_dir_child(tree, tail) {
        tail = child;
    }
    tail
}

/// The display row `id` belongs to: walk up through absorbed sole sub-directory
/// links to the chain head — the node that actually owns a row.
pub fn segment_head(tree: &Tree, mut id: NodeId) -> NodeId {
    while is_absorbed(tree, id) {
        id = tree.nodes[id]
            .parent
            .expect("an absorbed node has a parent");
    }
    id
}

/// The concatenated display name for the row headed by `id`: a chain of sole
/// sub-directories joined with `/` (e.g. `src/main/java`). Files and branching
/// directories render as just their own name.
pub fn row_name(tree: &Tree, id: NodeId) -> String {
    let mut name = tree.nodes[id].name.clone();
    let mut tail = id;
    while let Some(child) = single_dir_child(tree, tail) {
        name.push('/');
        name.push_str(&tree.nodes[child].name);
        tail = child;
    }
    name
}

/// A row's indentation level, counted in *displayed* ancestors rather than raw
/// path depth, so a chained child sits one level under its `a/b/c` row instead
/// of three.
pub fn display_depth(tree: &Tree, id: NodeId) -> usize {
    let mut depth = 0;
    let mut cur = tree.nodes[id].parent;
    while let Some(p) = cur {
        if p == tree.root {
            break;
        }
        if !is_absorbed(tree, p) {
            depth += 1;
        }
        cur = tree.nodes[p].parent;
    }
    depth
}

/// Whether any directory in the row headed by `id` has a name containing
/// `query` (already lower-cased) — a match on `main` reveals all of `src/main`.
fn segment_name_matches(tree: &Tree, id: NodeId, query: &str) -> bool {
    let mut cur = id;
    loop {
        if tree.nodes[cur].name.to_lowercase().contains(query) {
            return true;
        }
        match single_dir_child(tree, cur) {
            Some(child) => cur = child,
            None => return false,
        }
    }
}

/// Depth-first list of currently visible rows — the root's descendants, honoring
/// each row's `expanded` flag. A chain of sole sub-directories is one row, so an
/// expanded chain reveals its *tail's* children; the root itself is not emitted.
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
            let tail = segment_tail(tree, id);
            stack.extend(tree.nodes[tail].children.iter().rev().copied());
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
    // A chain is one row: its children live under the tail, and a match on any
    // segment of the chain reveals the whole subtree.
    let tail = segment_tail(tree, id);
    let own = segment_name_matches(tree, id, query);
    let child_under = under_match || own;
    for &child in &tree.nodes[tail].children {
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

    /// A directory (`mid`) alphabetically after some files and before others, so
    /// only a dirs-first rule — not the name key — can lift it above them all.
    fn dir_amid_files() -> Tree {
        let files = vec![
            (rel("apple.rs"), 1),
            (rel("mid/inner.rs"), 1),
            (rel("zebra.rs"), 1),
        ];
        let dirs = vec![rel("mid")];
        build_skeleton(&files, &dirs, "p".into())
    }

    #[test]
    fn sort_by_name_lifts_dirs_above_files_ascending() {
        let mut tree = dir_amid_files();
        sort_by_name(&mut tree, SortDir::Asc);
        assert_eq!(names(&tree), ["mid", "apple.rs", "zebra.rs"]);
    }

    #[test]
    fn sort_by_name_keeps_dirs_first_when_reversed() {
        let mut tree = dir_amid_files();
        sort_by_name(&mut tree, SortDir::Desc);
        // Names flip within each group, but the directory still leads.
        assert_eq!(names(&tree), ["mid", "zebra.rs", "apple.rs"]);
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

    fn row_names(tree: &Tree) -> Vec<String> {
        flatten_visible(tree)
            .iter()
            .map(|&id| row_name(tree, id))
            .collect()
    }

    /// `a/b/c/deep.rs`: a is a top-level head, b and c are sole sub-directories
    /// folded into it, and the chain stops at `c` (whose only child is a file).
    fn chained() -> Tree {
        let files = vec![(rel("a/b/c/deep.rs"), 1)];
        let dirs = vec![rel("a"), rel("a/b"), rel("a/b/c")];
        build_skeleton(&files, &dirs, "p".into())
    }

    #[test]
    fn chain_concatenates_sole_subdirs_into_one_row() {
        let tree = chained();
        let a = tree.index[&rel("a")];
        assert_eq!(row_name(&tree, a), "a/b/c");
        assert_eq!(segment_tail(&tree, a), tree.index[&rel("a/b/c")]);
        // b and c are folded into a's row, not rows of their own.
        assert_eq!(segment_head(&tree, tree.index[&rel("a/b")]), a);
        assert_eq!(segment_head(&tree, tree.index[&rel("a/b/c")]), a);
    }

    #[test]
    fn chain_expands_as_one_unit_with_compact_indent() {
        let mut tree = chained();
        let a = tree.index[&rel("a")];
        // Collapsed: just the single concatenated row.
        assert_eq!(row_names(&tree), ["a/b/c"]);
        // Expanding the head reveals the tail's child, indented one level under it.
        tree.nodes[a].expanded = true;
        assert_eq!(row_names(&tree), ["a/b/c", "deep.rs"]);
        assert_eq!(display_depth(&tree, a), 0);
        assert_eq!(display_depth(&tree, tree.index[&rel("a/b/c/deep.rs")]), 1);
    }

    #[test]
    fn branching_or_file_only_dirs_do_not_chain() {
        // Two sub-directories under `a` → not a sole-child chain.
        let branching = build_skeleton(
            &[(rel("a/x/one.rs"), 1), (rel("a/y/two.rs"), 1)],
            &[rel("a"), rel("a/x"), rel("a/y")],
            "p".into(),
        );
        assert_eq!(row_name(&branching, branching.index[&rel("a")]), "a");

        // A single *file* child does not concatenate either.
        let file_only = build_skeleton(&[(rel("src/main.rs"), 1)], &[rel("src")], "p".into());
        assert_eq!(row_name(&file_only, file_only.index[&rel("src")]), "src");
    }

    #[test]
    fn filter_reveals_a_chained_row_by_inner_segment() {
        // Match on `b`, an inner segment of the `a/b/c` chain, reveals the row
        // and (since the segment matched) its subtree.
        let names = filtered_names(&chained(), "b");
        assert_eq!(names, ["a", "deep.rs"]);
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
