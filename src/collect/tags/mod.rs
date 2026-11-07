//! Line tags: which lines of a file are test code (and, later, generated or
//! benchmark code).
//!
//! A file is tagged two ways. **By path**, a whole file at once, from the test
//! runners' naming conventions ([`paths`]). **In the file**, by syntax: the
//! file is parsed with tree-sitter (through karet's parse host) and the items
//! a test attribute or a `test` block marks become line regions. Rust and Zig
//! keep tests next to the code they test, so they are the languages read
//! in-file; elsewhere tests live in test-named files, and the path rules carry
//! them.
//!
//! The result is a per-line [`TagSet`]; the code collector counts each run of
//! equally-tagged lines on its own and files it under that tag set.

mod attr;
pub mod generated;
pub mod paths;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use karet_treesitter::{ParserPool, Query, QueryMatch, SyntaxTree};
use tokei::LanguageType;

use crate::model::TagSet;

/// How one file's lines are tagged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tagging {
    /// Tags on every line of the file.
    pub whole: TagSet,
    /// Tags on line ranges (0-based rows, inclusive), possibly overlapping.
    pub regions: Vec<(usize, usize, TagSet)>,
    /// Where the Rust modules this file declares test-only (`#[cfg(test)] mod
    /// name;`) may live, relative to the scan root.
    pub test_modules: Vec<ModulePath>,
}

/// A place a declared module's source can be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModulePath {
    /// Exactly this file.
    File(PathBuf),
    /// Any file under this directory.
    Dir(PathBuf),
}

impl ModulePath {
    pub fn contains(&self, path: &Path) -> bool {
        match self {
            ModulePath::File(file) => path == file,
            ModulePath::Dir(dir) => path.starts_with(dir),
        }
    }
}

impl Tagging {
    /// The tags on 0-based `row`.
    #[cfg(test)]
    pub fn at(&self, row: usize) -> TagSet {
        self.regions
            .iter()
            .filter(|&&(first, last, _)| first <= row && row <= last)
            .fold(self.whole, |acc, &(_, _, tags)| acc.union(tags))
    }
}

/// Per-thread detection state: the tree-sitter parsers and the compiled
/// queries, built on first use.
#[derive(Default)]
pub struct Detector {
    pool: ParserPool,
    queries: HashMap<LanguageType, Option<Query>>,
}

/// The Rust query. Captures: `@attr` outer attributes, `@inner` inner
/// attributes, `@trivia` comments, `@item` items an attribute can sit on, and
/// `@name` the name of a top-level `mod` declaration.
const RUST_QUERY: &str = r"
(attribute_item) @attr
(inner_attribute_item) @inner
(line_comment) @trivia
(block_comment) @trivia
[
  (function_item) (function_signature_item) (mod_item) (impl_item)
  (struct_item) (enum_item) (union_item) (trait_item) (type_item)
  (const_item) (static_item) (use_declaration) (extern_crate_declaration)
  (macro_definition) (macro_invocation) (foreign_mod_item)
] @item
(source_file (mod_item name: (identifier) @name) @topmod)
";

/// The Zig query: `test "name" { … }` blocks, and comments for the doc lines
/// above them.
const ZIG_QUERY: &str = r"
(test_declaration) @test
(comment) @trivia
";

impl Detector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tag the lines of `text`, the file `rel` detected as `lang`.
    pub fn tag(&mut self, lang: LanguageType, rel: &Path, text: &[u8]) -> Tagging {
        let grammar = match lang {
            LanguageType::Rust => Some("rust"),
            LanguageType::Zig => Some("zig"),
            _ => None,
        };
        if paths::is_test_path(rel, grammar.is_some()) {
            return Tagging {
                whole: TagSet::TEST,
                ..Tagging::default()
            };
        }
        let Some(grammar) = grammar else {
            return Tagging::default();
        };
        // Every marker read here has `test` in it but `#[quickcheck]`, so a
        // file holding neither has nothing to find and skips the parse. A
        // huge file (generated bindings, mostly) is not read in-file at all.
        if !contains(text, b"test") && !contains(text, b"quickcheck") {
            return Tagging::default();
        }
        if text.len() > MAX_PARSE_BYTES {
            tracing::debug!(path = %rel.display(), bytes = text.len(), "too large to read for tests");
            return Tagging::default();
        }
        let Ok(text) = std::str::from_utf8(text) else {
            tracing::debug!(path = %rel.display(), "not UTF-8: not read for tests");
            return Tagging::default();
        };
        let Some(id) = karet_treesitter::language_id_from_injection_name(grammar) else {
            return Tagging::default();
        };
        let query = self.queries.entry(lang).or_insert_with(|| {
            let source = if lang == LanguageType::Rust {
                RUST_QUERY
            } else {
                ZIG_QUERY
            };
            Query::compile(id, source)
                .inspect_err(|error| tracing::warn!(%error, ?lang, "test query failed to compile"))
                .ok()
        });
        let Some(query) = query.as_ref() else {
            return Tagging::default();
        };
        let Ok(tree) = SyntaxTree::parse(&mut self.pool, id, text) else {
            return Tagging::default();
        };
        let matches = tree.matches(query, text);
        let names = query.capture_names();
        let rows = Rows::new(text);
        if lang == LanguageType::Rust {
            rust(text, rel, &matches, names, &rows)
        } else {
            zig(text, &matches, names, &rows)
        }
    }
}

/// The largest file read in-file for tests. Parsing is linear but not cheap
/// (a 5.5 MB generated binding takes seconds), and files that large are
/// generated rather than where tests live.
const MAX_PARSE_BYTES: usize = 1 << 20;

/// A byte span.
type Span = (usize, usize);

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Every capture named `name` across `matches`, as byte spans.
fn spans(matches: &[QueryMatch], names: &[&str], name: &str) -> Vec<Span> {
    let mut out: Vec<Span> = matches
        .iter()
        .flat_map(|m| m.captures.iter())
        .filter(|c| names.get(c.capture as usize) == Some(&name))
        .map(|c| (c.span.start.0, c.span.end.0))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Byte offset → 0-based row, by the same line split the counter uses.
struct Rows {
    newlines: Vec<usize>,
}

impl Rows {
    fn new(text: &str) -> Self {
        Self {
            newlines: text
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i)
                .collect(),
        }
    }

    /// The row holding byte `pos`.
    fn of(&self, pos: usize) -> usize {
        self.newlines.partition_point(|&nl| nl < pos)
    }

    /// The rows a non-empty span covers, first to last.
    fn of_span(&self, (start, end): Span) -> (usize, usize) {
        (self.of(start), self.of(end.saturating_sub(1).max(start)))
    }
}

/// Trivia (attributes and comments) indexed for the two walks a region needs:
/// forward over an attribute's gap to its item, and upward over the
/// attributes and doc comments attached above.
struct Trivia {
    by_start: BTreeMap<usize, usize>,
    by_end: BTreeMap<usize, usize>,
}

impl Trivia {
    fn new(spans: impl IntoIterator<Item = Span>) -> Self {
        let mut by_start = BTreeMap::new();
        let mut by_end = BTreeMap::new();
        for (start, end) in spans {
            by_start.insert(start, end);
            by_end.insert(end, start);
        }
        Self { by_start, by_end }
    }

    /// Whether `text[from..to]` holds only whitespace and trivia.
    fn only_trivia_between(&self, text: &str, mut from: usize, to: usize) -> bool {
        let bytes = text.as_bytes();
        while from < to {
            if bytes[from].is_ascii_whitespace() {
                from += 1;
            } else if let Some(&end) = self.by_start.get(&from) {
                from = end.max(from + 1);
            } else {
                return false;
            }
        }
        from == to
    }

    /// Walk `start` up over trivia attached directly above it: trivia that
    /// begins its own line (so not a trailing comment on the previous item)
    /// and is separated from `start` by whitespace holding at most one line
    /// break (so a blank line ends the run).
    fn extend_up(&self, text: &str, mut start: usize) -> usize {
        loop {
            let Some((&end, &above)) = self.by_end.range(..=start).next_back() else {
                return start;
            };
            // A doc line comment's node takes its line break with it; count
            // the gap from before it, or one blank line would look attached.
            let content_end = if text.as_bytes()[..end].ends_with(b"\n") {
                end - 1
            } else {
                end
            };
            let gap = &text[content_end..start];
            let attached = gap.bytes().all(|b| b.is_ascii_whitespace())
                && gap.bytes().filter(|&b| b == b'\n').count() <= 1;
            let line_start = text[..above].rfind('\n').map_or(0, |nl| nl + 1);
            let own_line = text[line_start..above]
                .bytes()
                .all(|b| b.is_ascii_whitespace());
            // Inner doc comments (`//!`, `/*!`) document the enclosing module,
            // not the item below them.
            let inner_doc = text[above..].starts_with("//!") || text[above..].starts_with("/*!");
            if !attached || !own_line || inner_doc || above >= start {
                return start;
            }
            start = above;
        }
    }
}

/// Rust: items carrying a test attribute, `#![cfg(test)]` files and modules,
/// and the out-of-line `#[cfg(test)] mod name;` declarations.
fn rust(text: &str, rel: &Path, matches: &[QueryMatch], names: &[&str], rows: &Rows) -> Tagging {
    let attrs = spans(matches, names, "attr");
    let inners = spans(matches, names, "inner");
    let items = spans(matches, names, "item");
    let trivia = Trivia::new(
        attrs
            .iter()
            .chain(spans(matches, names, "trivia").iter())
            .copied(),
    );
    let mut tagging = Tagging::default();
    let mut test_items: Vec<Span> = Vec::new();
    let mut path_items: Vec<Span> = Vec::new();

    // The item an attribute sits on: the first item starting after it, with
    // only whitespace, comments and other attributes in between. Anything else
    // (a field, a statement) leaves the attribute unpaired.
    let item_of = |attr: Span| {
        let after = items.partition_point(|&(start, _)| start < attr.1);
        items
            .get(after)
            .copied()
            .filter(|item| trivia.only_trivia_between(text, attr.1, item.0))
    };

    for &attr in &attrs {
        let source = &text[attr.0..attr.1];
        if attr::attribute_path(source).as_deref() == Some("path") {
            path_items.extend(item_of(attr));
        }
        if !attr::marks_test(source) {
            continue;
        }
        let Some(item) = item_of(attr) else {
            continue;
        };
        test_items.push(item);
        let start = trivia.extend_up(text, attr.0);
        let (first, _) = rows.of_span((start, item.1));
        let (_, last) = rows.of_span(item);
        tagging.regions.push((first, last, TagSet::TEST));
    }

    for &inner in &inners {
        if !attr::marks_test(&text[inner.0..inner.1]) {
            continue;
        }
        // `#![cfg(test)]` applies to what encloses it: the innermost item, or
        // the whole file at the top level.
        let enclosing = items
            .iter()
            .filter(|&&(start, end)| start < inner.0 && inner.1 <= end)
            .min_by_key(|&&(start, end)| end - start);
        match enclosing {
            Some(&item) => {
                let (first, last) = rows.of_span(item);
                tagging.regions.push((first, last, TagSet::TEST));
            }
            None => tagging.whole = tagging.whole.union(TagSet::TEST),
        }
    }

    // Out-of-line test modules: a top-level `mod name;` that is a test item.
    // One with a `#[path]` lives somewhere this does not resolve, and is left
    // alone rather than tagging the file its name would otherwise point at.
    for m in matches {
        let capture = |name: &str| {
            m.captures
                .iter()
                .find(|c| names.get(c.capture as usize) == Some(&name))
                .map(|c| (c.span.start.0, c.span.end.0))
        };
        let (Some(decl), Some(name)) = (capture("topmod"), capture("name")) else {
            continue;
        };
        if test_items.contains(&decl)
            && !path_items.contains(&decl)
            && text[decl.0..decl.1].trim_end().ends_with(';')
        {
            let name = &text[name.0..name.1];
            let name = name.strip_prefix("r#").unwrap_or(name);
            tagging.test_modules.extend(module_paths(rel, name));
        }
    }
    tagging
}

/// Where `mod name;`, declared in the file `rel`, lives: `name.rs` in the
/// declaring file's module directory, or anything under `name/` there (its
/// `mod.rs` and its submodules). The module directory is the file's own
/// directory for `mod.rs`, `lib.rs` and `main.rs`, and `dir/stem/` otherwise.
fn module_paths(rel: &Path, name: &str) -> [ModulePath; 2] {
    let dir = rel.parent().unwrap_or(Path::new(""));
    let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let module_dir = if matches!(stem, "mod" | "lib" | "main") {
        dir.to_path_buf()
    } else {
        dir.join(stem)
    };
    [
        ModulePath::File(module_dir.join(format!("{name}.rs"))),
        ModulePath::Dir(module_dir.join(name)),
    ]
}

/// Zig: every `test "…" { … }` block, with the comments directly above it.
fn zig(text: &str, matches: &[QueryMatch], names: &[&str], rows: &Rows) -> Tagging {
    let trivia = Trivia::new(spans(matches, names, "trivia"));
    let regions = spans(matches, names, "test")
        .into_iter()
        .map(|block| {
            let start = trivia.extend_up(text, block.0);
            let (first, _) = rows.of_span((start, block.1));
            let (_, last) = rows.of_span(block);
            (first, last, TagSet::TEST)
        })
        .collect();
    Tagging {
        regions,
        ..Tagging::default()
    }
}

#[cfg(test)]
mod tests;
