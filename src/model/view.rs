//! View-layer transforms over a built [`Tree`]: sorting and flattening.

use std::cmp::Ordering;

use super::node::{NodeId, NodeKind, SortDir, SortKey, Tree, TreeNode};

/// Order every node's children by `key`/`dir` with a deterministic tie-break.
pub fn sort(tree: &mut Tree, key: SortKey, dir: SortDir) {
    for id in 0..tree.nodes.len() {
        // Take the children out so the comparator can borrow the arena.
        let mut children = std::mem::take(&mut tree.nodes[id].children);
        children.sort_by(|&a, &b| cmp(&tree.nodes[a], &tree.nodes[b], key, dir));
        tree.nodes[id].children = children;
    }
}

fn metric(node: &TreeNode, key: SortKey) -> usize {
    match key {
        SortKey::Lines => node.stats.lines(),
        SortKey::Code => node.stats.code,
        SortKey::Comments => node.stats.comments,
        SortKey::Blanks => node.stats.blanks,
        SortKey::Files => node.stats.files,
        SortKey::Name => 0,
    }
}

fn cmp(a: &TreeNode, b: &TreeNode, key: SortKey, dir: SortDir) -> Ordering {
    let primary = match key {
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        _ => metric(a, key).cmp(&metric(b, key)),
    };
    let primary = match dir {
        SortDir::Desc => primary.reverse(),
        SortDir::Asc => primary,
    };
    // Tie-break by name then unique relative path for a stable, total order.
    primary
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
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
    use crate::model::build_tree;
    use std::path::{Path, PathBuf};
    use tokei::{Language, LanguageType, Languages, Report};

    fn tree() -> Tree {
        let mk = |path: &str, code: usize| {
            let mut r = Report::new(PathBuf::from(path));
            r.stats.code = code;
            r
        };
        let mut rust = Language::new();
        rust.reports = vec![mk("/p/big.rs", 500), mk("/p/small.rs", 5)];
        let mut languages = Languages::new();
        languages.insert(LanguageType::Rust, rust);
        build_tree(&languages, Path::new("/p"), "p".into())
    }

    #[test]
    fn sort_desc_then_reverse_is_deterministic() {
        let mut t = tree();
        sort(&mut t, SortKey::Code, SortDir::Desc);
        let order: Vec<&str> = flatten_visible(&t)
            .iter()
            .map(|&id| t.nodes[id].name.as_str())
            .collect();
        assert_eq!(order, ["big.rs", "small.rs"]);

        sort(&mut t, SortKey::Code, SortDir::Asc);
        let order: Vec<&str> = flatten_visible(&t)
            .iter()
            .map(|&id| t.nodes[id].name.as_str())
            .collect();
        assert_eq!(order, ["small.rs", "big.rs"]);
    }

    fn nested_tree() -> Tree {
        let mut rust = Language::new();
        rust.reports = vec![
            Report::new(PathBuf::from("/p/src/main.rs")),
            Report::new(PathBuf::from("/p/src/lib.rs")),
            Report::new(PathBuf::from("/p/docs/guide.md")),
        ];
        let mut languages = Languages::new();
        languages.insert(LanguageType::Rust, rust);
        build_tree(&languages, Path::new("/p"), "p".into())
    }

    fn filtered_names(tree: &Tree, query: &str) -> Vec<String> {
        flatten_filtered(tree, query)
            .iter()
            .map(|&id| tree.nodes[id].name.clone())
            .collect()
    }

    #[test]
    fn filter_keeps_matches_and_ancestors() {
        let names = filtered_names(&nested_tree(), "lib");
        assert!(names.contains(&"src".to_string())); // ancestor of the match
        assert!(names.contains(&"lib.rs".to_string())); // the match
        assert!(!names.contains(&"main.rs".to_string())); // sibling, no match
        assert!(!names.contains(&"docs".to_string())); // unrelated subtree
    }

    #[test]
    fn filter_reveals_subtree_of_a_matching_dir() {
        let names = filtered_names(&nested_tree(), "src");
        assert!(names.contains(&"src".to_string()));
        assert!(names.contains(&"main.rs".to_string()));
        assert!(names.contains(&"lib.rs".to_string()));
        assert!(!names.contains(&"docs".to_string()));
    }
}
