//! Build the directory [`Tree`] skeleton from a filesystem walk, and aggregate
//! lazily-collected per-file metrics up the tree.
//!
//! The skeleton (structure + on-disk size + file tally) comes from the walk, so
//! *every* non-ignored file appears — not just files a language counter
//! recognizes. Expensive metrics arrive later as per-file maps and are folded
//! bottom-up into a per-node [`Layer`](super::Layer) via [`aggregate`] /
//! [`aggregate_code`].

use std::cmp::Reverse;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::ops::AddAssign;
use std::path::{Component, Path, PathBuf};

use super::node::{CodeData, NodeId, Tree, TreeNode};

/// Build the tree skeleton from walked `files` (relative path + size in bytes)
/// and `dirs` (relative directory paths, so empty directories appear too).
pub fn build_skeleton(files: &[(PathBuf, u64)], dirs: &[PathBuf], root_label: String) -> Tree {
    let root_id = 0usize;
    let mut nodes: Vec<TreeNode> = vec![TreeNode::dir(root_label, PathBuf::new(), None, 0)];
    let mut dir_ids: HashMap<PathBuf, NodeId> = HashMap::new();
    dir_ids.insert(PathBuf::new(), root_id);

    // Directories first (deterministic order) so empty dirs get nodes.
    let mut dir_list: Vec<&PathBuf> = dirs.iter().collect();
    dir_list.sort();
    for dir in dir_list {
        ensure_dir_chain(dir, &mut nodes, &mut dir_ids);
    }

    // Then files, deterministic order.
    let mut file_list: Vec<&(PathBuf, u64)> = files.iter().collect();
    file_list.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel, bytes) in file_list {
        let comps = normal_components(rel);
        let Some((name_os, parents)) = comps.split_last() else {
            continue;
        };
        let mut parent = root_id;
        let mut acc = PathBuf::new();
        for comp in parents {
            acc.push(comp);
            parent = match dir_ids.get(&acc) {
                Some(&id) => id,
                None => insert_dir(comp.as_os_str(), &acc, parent, &mut nodes, &mut dir_ids),
            };
        }
        let name = name_os.to_string_lossy().into_owned();
        let id = nodes.len();
        let mut node = TreeNode::file(name, rel.clone(), Some(parent), nodes[parent].depth + 1);
        node.bytes = *bytes;
        node.files = 1;
        nodes.push(node);
        nodes[parent].children.push(id);
    }

    // Aggregate the always-on metrics (bytes, files) bottom-up.
    for id in depth_desc(&nodes) {
        if let Some(parent) = nodes[id].parent {
            let (bytes, files) = (nodes[id].bytes, nodes[id].files);
            nodes[parent].bytes += bytes;
            nodes[parent].files += files;
        }
    }

    nodes[root_id].expanded = true;

    let index = nodes
        .iter()
        .enumerate()
        .map(|(id, node)| (node.rel_path.clone(), id))
        .collect();
    Tree {
        nodes,
        root: root_id,
        index,
    }
}

/// Fold a per-file metric map into one aggregated value per node (summed
/// bottom-up). File values come from `per_file` keyed by relative path; every
/// directory ends up holding its subtree's total.
pub fn aggregate<T>(tree: &Tree, per_file: &HashMap<PathBuf, T>) -> Box<[T]>
where
    T: Copy + Default + AddAssign,
{
    let mut vals = vec![T::default(); tree.nodes.len()];
    for (path, &value) in per_file {
        if let Some(&id) = tree.index.get(path) {
            vals[id] = value;
        }
    }
    for id in depth_desc(&tree.nodes) {
        if let Some(parent) = tree.nodes[id].parent {
            let value = vals[id];
            vals[parent] += value;
        }
    }
    vals.into_boxed_slice()
}

/// Like [`aggregate`], but for the code layer: sums line counts *and* merges the
/// per-language breakdown up the tree. `primary_lang` stays per-file (directories
/// are colored by kind, not language).
pub fn aggregate_code(tree: &Tree, per_file: &HashMap<PathBuf, CodeData>) -> Box<[CodeData]> {
    let mut vals: Vec<CodeData> = vec![CodeData::default(); tree.nodes.len()];
    for (path, data) in per_file {
        if let Some(&id) = tree.index.get(path) {
            vals[id] = data.clone();
        }
    }
    for id in depth_desc(&tree.nodes) {
        if let Some(parent) = tree.nodes[id].parent {
            let num = vals[id].num;
            let langs = vals[id].langs.clone();
            vals[parent].num += num;
            for (lang, stats) in langs {
                *vals[parent].langs.entry(lang).or_default() += stats;
            }
        }
    }
    vals.into_boxed_slice()
}

/// Node ids ordered deepest-first, so children are visited before their parents.
fn depth_desc(nodes: &[TreeNode]) -> Vec<NodeId> {
    let mut order: Vec<NodeId> = (0..nodes.len()).collect();
    order.sort_by_key(|&id| Reverse(nodes[id].depth));
    order
}

/// The `Component::Normal` parts of a path (drops `.`/`..`/root prefixes).
fn normal_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect()
}

/// Ensure a directory node (and all its ancestors) exists; return its id.
fn ensure_dir_chain(
    rel: &Path,
    nodes: &mut Vec<TreeNode>,
    dir_ids: &mut HashMap<PathBuf, NodeId>,
) -> NodeId {
    let mut parent = 0usize;
    let mut acc = PathBuf::new();
    for comp in normal_components(rel) {
        acc.push(&comp);
        parent = match dir_ids.get(&acc) {
            Some(&id) => id,
            None => insert_dir(comp.as_os_str(), &acc, parent, nodes, dir_ids),
        };
    }
    parent
}

fn insert_dir(
    name_os: &OsStr,
    acc: &Path,
    parent: NodeId,
    nodes: &mut Vec<TreeNode>,
    dir_ids: &mut HashMap<PathBuf, NodeId>,
) -> NodeId {
    let id = nodes.len();
    let name = name_os.to_string_lossy().into_owned();
    let depth = nodes[parent].depth + 1;
    nodes.push(TreeNode::dir(name, acc.to_path_buf(), Some(parent), depth));
    nodes[parent].children.push(id);
    dir_ids.insert(acc.to_path_buf(), id);
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::node::{ChurnData, CodeNum};
    use tokei::LanguageType;

    fn rel(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    /// `/proj` with `src/main.rs`, `src/lib.rs`, `README.md`, and a binary
    /// `assets/logo.png` (size only, no code).
    fn skeleton() -> Tree {
        let files = vec![
            (rel("src/main.rs"), 1000),
            (rel("src/lib.rs"), 400),
            (rel("README.md"), 80),
            (rel("assets/logo.png"), 5000),
        ];
        let dirs = vec![rel("src"), rel("assets")];
        build_skeleton(&files, &dirs, "proj".into())
    }

    #[test]
    fn skeleton_aggregates_bytes_and_files() {
        let tree = skeleton();
        assert_eq!(tree.total_files(), 4);
        assert_eq!(tree.total_bytes(), 6480);

        let src = tree.nodes.iter().find(|n| n.name == "src").unwrap();
        assert_eq!(src.files, 2);
        assert_eq!(src.bytes, 1400);
    }

    #[test]
    fn non_code_file_appears_with_size_only() {
        let tree = skeleton();
        let logo = tree.nodes.iter().find(|n| n.name == "logo.png");
        assert!(logo.is_some(), "binary file should appear in the tree");
        assert_eq!(logo.unwrap().bytes, 5000);

        // The code layer is empty for it; aggregation leaves it at zero.
        let code = aggregate_code(&tree, &HashMap::new());
        let logo_id = tree.index[&rel("assets/logo.png")];
        assert_eq!(code[logo_id].num.lines(), 0);
    }

    #[test]
    fn aggregate_sums_a_layer_bottom_up() {
        let tree = skeleton();
        let mut per_file = HashMap::new();
        per_file.insert(
            rel("src/main.rs"),
            ChurnData {
                added: 10,
                deleted: 2,
                commits: 3,
            },
        );
        per_file.insert(
            rel("src/lib.rs"),
            ChurnData {
                added: 5,
                deleted: 1,
                commits: 1,
            },
        );
        let churn = aggregate(&tree, &per_file);

        let src_id = tree.index[&rel("src")];
        assert_eq!(churn[src_id].added, 15);
        assert_eq!(churn[src_id].deleted, 3);
        assert_eq!(churn[tree.root].commits, 4);
    }

    #[test]
    fn aggregate_code_merges_languages() {
        let tree = skeleton();
        let mut per_file = HashMap::new();
        let num = CodeNum {
            code: 100,
            comments: 10,
            blanks: 5,
        };
        let mut main = CodeData {
            num,
            ..Default::default()
        };
        main.langs.insert(LanguageType::Rust, num);
        per_file.insert(rel("src/main.rs"), main);

        let code = aggregate_code(&tree, &per_file);
        let src_id = tree.index[&rel("src")];
        assert_eq!(code[src_id].num.code, 100);
        assert_eq!(
            code[src_id].langs.get(&LanguageType::Rust).unwrap().code,
            100
        );
        assert_eq!(code[tree.root].num.lines(), 115);
    }
}
