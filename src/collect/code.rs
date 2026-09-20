//! The code lens collector: tokei line counts, keyed by relative path.
//!
//! tokei may report one physical file under several languages (embedded
//! languages); those reports are merged into a single [`CodeData`] whose totals
//! sum across languages, with each language's contribution kept in `langs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokei::{Config, Languages};

use super::relative_path;
use crate::model::CodeData;

/// Run tokei over `root`, returning per-file code data and whether any language
/// reported a parsing ambiguity.
pub fn collect_code(root: &Path) -> (HashMap<PathBuf, CodeData>, bool) {
    let mut languages = Languages::new();
    // Match the walk's file set: tokei honors .gitignore/.ignore and walks in
    // parallel, but its `hidden` defaults to "skip", which would report every
    // dot-entry the walk now shows as zero lines. Turning that off makes tokei
    // descend `.git` too, so it is excluded explicitly (these become negated
    // override globs inside tokei) — the same prune the walk does.
    let ignored: &[&str] = &[".git", ".jj"];
    let config = Config {
        hidden: Some(true),
        ..Config::default()
    };
    languages.get_statistics(&[root], ignored, &config);
    let inaccurate = languages.values().any(|language| language.inaccurate);

    let mut files: HashMap<PathBuf, CodeData> = HashMap::new();
    for (lang, language) in languages.iter() {
        for report in &language.reports {
            let rel = relative_path(&report.name, root);
            if rel.as_os_str().is_empty() {
                continue;
            }
            let stats = &report.stats;
            let data = files.entry(rel).or_default();
            data.num.code += stats.code;
            data.num.comments += stats.comments;
            data.num.blanks += stats.blanks;
            let entry = data.langs.entry(*lang).or_default();
            entry.code += stats.code;
            entry.comments += stats.comments;
            entry.blanks += stats.blanks;
        }
    }

    for data in files.values_mut() {
        data.primary_lang = data
            .langs
            .iter()
            .max_by_key(|(_, stats)| stats.lines())
            .map(|(lang, _)| *lang);
    }

    (files, inaccurate)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// The code lens must see the same files the walk does. tokei skips hidden
    /// files by default, and turning that off makes it descend `.git` — this
    /// pins both halves against this crate's own checkout. Inert in the
    /// published tarball, which excludes `/.github`.
    #[test]
    fn dot_entries_are_counted_and_the_git_dir_is_not() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        if !root.join(".github/workflows/ci.yml").exists() {
            return;
        }
        let (files, _) = super::collect_code(root);

        let ci = files
            .get(Path::new(".github/workflows/ci.yml"))
            .expect("a tracked file under a dot-directory is counted");
        assert!(ci.num.code > 0, "and it has real line counts");
        assert!(
            !files.keys().any(|p| p.starts_with(".git/")),
            "tokei must not walk the git object store"
        );
    }
}
