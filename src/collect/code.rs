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
    //
    // One gap remains: tokei keeps its gitignore rules, so a *force-added*
    // ignored file — the only thing the walk's index union contributes — has a
    // node but no report, and renders as zero lines like any file tokei can't
    // classify. Closing it would mean feeding tokei the tracked set as
    // whitelist overrides.
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
    use std::path::{Path, PathBuf};

    /// A scratch directory holding a fake `.git` whose contents tokei *would*
    /// classify. A real `.git` is all loose objects and `.sample` files, none
    /// of which tokei recognizes, so asserting against one proves nothing.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tree-tui-code-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the temp root");
            Self(dir)
        }

        fn write(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create a fixture dir");
            }
            std::fs::write(path, body).expect("write a fixture file");
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// tokei skips hidden files by default, which would report every dot-entry
    /// the walk now shows as zero lines.
    #[test]
    fn dot_entries_are_counted() {
        let root = TempRoot::new("dotfiles");
        root.write(".github/workflows/ci.yml", "on: push\njobs:\n  a:\n");
        root.write("src/main.rs", "fn main() {}\n");

        let (files, _) = super::collect_code(&root.0);
        let ci = files
            .get(Path::new(".github/workflows/ci.yml"))
            .expect("a file under a dot-directory is counted");
        assert!(ci.num.code > 0, "and it has real line counts");
    }

    /// Turning tokei's hidden filter off makes it descend `.git`, where it
    /// opens every extensionless file to sniff a shebang. The hook below is
    /// exactly such a file, so this fails without the explicit exclusion.
    #[test]
    fn the_git_directory_is_not_counted() {
        let root = TempRoot::new("gitdir");
        root.write("src/main.rs", "fn main() {}\n");
        root.write(".git/hooks/pre-commit", "#!/bin/sh\necho hi\nexit 0\n");
        root.write(".git/config", "[core]\n\trepositoryformatversion = 0\n");

        let (files, _) = super::collect_code(&root.0);
        assert!(
            !files.keys().any(|p| p.starts_with(".git")),
            "tokei must not walk the git directory: {:?}",
            files.keys().collect::<Vec<_>>()
        );
    }
}
