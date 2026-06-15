//! Construct the directory [`Tree`] from tokei's per-file reports.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use tokei::{LanguageType, Languages};

use super::node::{Stats, Tree, TreeNode};

/// Build a directory tree of aggregated stats from tokei results.
///
/// The same file path can be reported under multiple languages (embedded
/// languages); such reports are merged into a single file node whose stats sum
/// across languages, with each language's contribution kept in `langs`.
pub fn build_tree(languages: &Languages, root: &Path, root_label: String) -> Tree {
    // 1. Merge reports by relative path.
    #[derive(Default)]
    struct FileAgg {
        stats: Stats,
        langs: BTreeMap<LanguageType, Stats>,
    }
    let mut files: HashMap<PathBuf, FileAgg> = HashMap::new();
    for (lang, language) in languages.iter() {
        for report in &language.reports {
            let rel = relative_path(&report.name, root);
            if rel.as_os_str().is_empty() {
                continue;
            }
            let s = &report.stats;
            let agg = files.entry(rel).or_default();
            agg.stats.code += s.code;
            agg.stats.comments += s.comments;
            agg.stats.blanks += s.blanks;
            let entry = agg.langs.entry(*lang).or_default();
            entry.code += s.code;
            entry.comments += s.comments;
            entry.blanks += s.blanks;
            entry.files = 1;
        }
    }

    // 2. Insert each file into the arena, creating directory nodes as needed.
    let root_id = 0usize;
    let mut nodes: Vec<TreeNode> = vec![TreeNode::dir(root_label, PathBuf::new(), None, 0)];
    let mut dir_ids: HashMap<PathBuf, usize> = HashMap::new();
    dir_ids.insert(PathBuf::new(), root_id);

    // Deterministic insertion order so the unsorted structure is stable.
    let mut rels: Vec<PathBuf> = files.keys().cloned().collect();
    rels.sort();
    for rel in &rels {
        let comps: Vec<OsString> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_os_string()),
                _ => None,
            })
            .collect();
        if comps.is_empty() {
            continue;
        }
        let mut parent = root_id;
        let mut acc = PathBuf::new();
        for (i, comp) in comps.iter().enumerate() {
            acc.push(comp);
            let name = comp.to_string_lossy().into_owned();
            let is_file = i + 1 == comps.len();
            if is_file {
                let agg = &files[rel];
                let id = nodes.len();
                let mut node =
                    TreeNode::file(name, acc.clone(), Some(parent), nodes[parent].depth + 1);
                node.stats = agg.stats;
                node.stats.files = 1;
                node.langs = agg.langs.clone();
                node.primary_lang = agg
                    .langs
                    .iter()
                    .max_by_key(|(_, s)| s.lines())
                    .map(|(k, _)| *k);
                nodes.push(node);
                nodes[parent].children.push(id);
            } else if let Some(&existing) = dir_ids.get(&acc) {
                parent = existing;
            } else {
                let id = nodes.len();
                let node = TreeNode::dir(name, acc.clone(), Some(parent), nodes[parent].depth + 1);
                nodes.push(node);
                nodes[parent].children.push(id);
                dir_ids.insert(acc.clone(), id);
                parent = id;
            }
        }
    }

    // 3. Aggregate stats and language breakdowns bottom-up (deepest first).
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&id| std::cmp::Reverse(nodes[id].depth));
    for id in order {
        let Some(parent) = nodes[id].parent else {
            continue;
        };
        let stats = nodes[id].stats;
        let langs = nodes[id].langs.clone();
        nodes[parent].stats += stats;
        for (lang, lang_stats) in langs {
            *nodes[parent].langs.entry(lang).or_default() += lang_stats;
        }
    }

    nodes[root_id].expanded = true;

    Tree {
        nodes,
        root: root_id,
    }
}

/// `path` relative to `root`, reduced to its normal components. Falls back to
/// the path's own normal components if it is not under `root`.
fn relative_path(path: &Path, root: &Path) -> PathBuf {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokei::{Language, Report};

    fn report(path: &str, code: usize, comments: usize, blanks: usize) -> Report {
        let mut r = Report::new(PathBuf::from(path));
        r.stats.code = code;
        r.stats.comments = comments;
        r.stats.blanks = blanks;
        r
    }

    fn language(reports: Vec<Report>) -> Language {
        let mut l = Language::new();
        l.reports = reports;
        l
    }

    #[test]
    fn aggregates_directories_bottom_up() {
        let mut languages = Languages::new();
        languages.insert(
            LanguageType::Rust,
            language(vec![
                report("/proj/src/main.rs", 100, 10, 5),
                report("/proj/src/lib.rs", 40, 4, 2),
            ]),
        );
        languages.insert(
            LanguageType::Markdown,
            language(vec![report("/proj/README.md", 8, 0, 3)]),
        );

        let tree = build_tree(&languages, Path::new("/proj"), "proj".into());

        let totals = tree.totals();
        assert_eq!(totals.files, 3);
        assert_eq!(totals.code, 148);
        assert_eq!(totals.comments, 14);
        assert_eq!(totals.blanks, 10);
        assert_eq!(totals.lines(), 172);

        let src = tree.nodes.iter().find(|n| n.name == "src").unwrap();
        assert_eq!(src.stats.files, 2);
        assert_eq!(src.stats.code, 140);
        assert_eq!(src.langs.get(&LanguageType::Rust).unwrap().files, 2);
    }

    #[test]
    fn merges_same_path_across_languages() {
        let mut languages = Languages::new();
        languages.insert(
            LanguageType::Html,
            language(vec![report("/p/index.html", 10, 1, 1)]),
        );
        languages.insert(
            LanguageType::JavaScript,
            language(vec![report("/p/index.html", 20, 2, 0)]),
        );

        let tree = build_tree(&languages, Path::new("/p"), "p".into());

        // One physical file, counted once, with two language contributions.
        assert_eq!(tree.totals().files, 1);
        assert_eq!(tree.totals().code, 30);
        let file = tree.nodes.iter().find(|n| n.name == "index.html").unwrap();
        assert_eq!(file.langs.len(), 2);
    }
}
