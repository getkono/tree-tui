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
    // tokei honors .gitignore/.ignore and walks in parallel.
    let ignored: &[&str] = &[];
    languages.get_statistics(&[root], ignored, &Config::default());
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
