//! The code lens collector: per-file line counts, keyed by relative path.
//!
//! The file set is the walk's, not tokei's: every file the tree shows is
//! offered here, and tokei is used per file — [`LanguageType::from_path`] to
//! pick the language and [`LanguageType::parse_from_slice`] to split code from
//! comments. Two of tokei's own choices are corrected on the way, because the
//! counts it reports are not what the code lens means by them:
//!
//! - **Blank lines.** tokei calls a line blank only outside comments and
//!   strings, and only when it is ASCII whitespace. A whitespace-only line in a
//!   `/* */` block or a docstring therefore counts as a comment or as code, and a
//!   line of nothing but a no-break space or a byte-order mark counts as code.
//!   Here a line is blank by [`is_blank`], whatever state it sits in.
//! - **Embedded languages.** tokei parses Markdown code fences (and HTML
//!   script blocks, and the like) as child-language *blobs*, filed apart from
//!   the parent file's own counts; reading only the parent's counts drops every
//!   such line. The blobs are folded back in here, under their own language.
//!   Rust doc comments, which tokei also lifts out as Markdown, are kept as
//!   the file's own comments instead (see [`plain_rust_comment`]).
//!
//! The result keeps one invariant per counted file: `code + comments + blanks`
//! is its physical line count (see [`lines`]), and `blanks` is exactly the
//! number of lines [`is_blank`] accepts. Jupyter notebooks are exempt: their
//! lines are tokei's count of the cells, not of the JSON.
//!
//! Only regular files are counted: symlinks, FIFOs and devices are skipped,
//! as tokei's own walk skips them.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokei::{CodeStats, Config, LanguageType};

use crate::model::{CodeData, CodeNum};

/// One file's line counts: the totals, and how they split across the
/// languages the file holds (its own, plus any embedded ones).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCounts {
    pub total: CodeNum,
    pub langs: BTreeMap<LanguageType, CodeNum>,
}

/// Count the lines of every file in `files` (relative to `root`) that tokei
/// recognizes, returning per-file code data and whether any recognized file
/// could not be read.
pub fn collect_code(root: &Path, files: &[PathBuf]) -> (HashMap<PathBuf, CodeData>, bool) {
    let config = Config::default();
    let next = AtomicUsize::new(0);
    let inaccurate = AtomicBool::new(false);
    let merged: Mutex<HashMap<PathBuf, CodeData>> = Mutex::new(HashMap::new());
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(files.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(rel) = files.get(i) else { break };
                    let path = root.join(rel);
                    // Regular files only, as tokei's own walk has it: a symlink
                    // would count its target twice (or, dangling, flag the
                    // tree inaccurate), and a FIFO or device could block the
                    // read forever. A file gone since the walk is skipped; the
                    // watcher's rescan catches up.
                    if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
                        continue;
                    }
                    let Some(lang) = LanguageType::from_path(&path, &config) else {
                        continue;
                    };
                    match read_decoded(&path) {
                        Ok(text) => {
                            let counts = count_file(lang, &text, &config);
                            local.push((rel.clone(), code_data(lang, counts)));
                        }
                        Err(error) => {
                            tracing::debug!(path = %path.display(), %error, "unreadable source file");
                            inaccurate.store(true, Ordering::Relaxed);
                        }
                    }
                }
                merged
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .extend(local);
            });
        }
    });

    let files = merged
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (files, inaccurate.into_inner())
}

/// Read a file the way tokei does: BOM-sniffed, so UTF-16 sources decode to
/// UTF-8 and a UTF-8 byte-order mark is dropped.
fn read_decoded(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut text = Vec::new();
    encoding_rs_io::DecodeReaderBytesBuilder::new()
        .build(file)
        .read_to_end(&mut text)?;
    Ok(text)
}

/// A file's code-layer entry. Its glyph is colored by the language it was
/// detected as, never by an embedded one — a README full of fenced Rust is
/// still Markdown.
fn code_data(lang: LanguageType, counts: FileCounts) -> CodeData {
    CodeData {
        num: counts.total,
        langs: counts.langs,
        primary_lang: Some(lang),
    }
}

/// Count one file's lines as `lang`.
///
/// Blank lines are decided here, by [`is_blank`]; tokei classifies only what is
/// left, with the blank lines removed. Removing a line that holds nothing but
/// whitespace or a bare comment marker leaves tokei's comment and string
/// tracking where it was, so the remaining lines are classified as they would
/// have been in place.
///
/// Rust doc-comment lines reach tokei as plain `//` comments (see
/// [`plain_rust_comment`]).
///
/// Jupyter notebooks are the exception: tokei counts the cells inside the JSON,
/// not the file's own lines, so its figures are taken as they are.
pub fn count_file(lang: LanguageType, text: &[u8], config: &Config) -> FileCounts {
    if lang == LanguageType::Jupyter {
        return from_stats(lang, &lang.parse_from_slice(text, config));
    }

    let mut blanks = 0;
    let mut physical = 0;
    let mut kept = Vec::with_capacity(text.len());
    for line in lines(text) {
        physical += 1;
        if is_blank(lang, line) {
            blanks += 1;
        } else {
            kept.extend_from_slice(&plain_rust_comment(lang, line));
            kept.push(b'\n');
        }
    }

    let mut counts = from_stats(lang, &lang.parse_from_slice(&kept, config));
    // Everything tokei saw is non-blank by our rule; any line it still calls
    // blank (an embedded block whose content is empty) is a comment.
    for num in counts.langs.values_mut() {
        num.comments += std::mem::take(&mut num.blanks);
    }
    reconcile(lang, &mut counts.langs, physical - blanks);
    counts.langs.entry(lang).or_default().blanks += blanks;
    counts.langs.retain(|_, num| num.lines() > 0);
    counts.total = counts
        .langs
        .values()
        .fold(CodeNum::default(), |mut acc, num| {
            acc += *num;
            acc
        });
    counts
}

/// `line` with a leading Rust doc-comment marker (`///`, `//!`, or the `////`
/// tokei mistakes for one) turned into a plain `//` comment.
///
/// tokei lifts a doc block into a Markdown blob, and the lookahead that finds
/// one trims across line ends: a code line holding a string literal, directly
/// above a doc comment, drops out of its counts. A plain `//` comment takes no
/// such path, yet stays a comment to tokei — or code, inside a string literal.
///
/// Only the marker itself is touched: a space goes in after its first two
/// characters (`///* x` becomes `// /* x`), and every other byte stays where
/// it was. The marker's third character can open a token — the `/` of a
/// nested `/*`, or of a `*/` — so it is kept, and the comment and string
/// state carries on as it would have.
fn plain_rust_comment(lang: LanguageType, line: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let indent = line.len() - line.trim_ascii_start().len();
    let body = &line[indent..];
    let is_doc = body.starts_with(b"///") || body.starts_with(b"//!");
    if lang != LanguageType::Rust || !is_doc {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut plain = Vec::with_capacity(line.len() + 1);
    plain.extend_from_slice(&line[..indent + 2]);
    plain.push(b' ');
    plain.extend_from_slice(&body[2..]);
    std::borrow::Cow::Owned(plain)
}

/// Make tokei's non-blank lines add up to `nonblank`.
///
/// tokei can miscount at an embedded-language seam: a Markdown fence closed
/// with an info string (```` ```rust ````) is counted both as a fence and as
/// content. The difference lands on the file's own language: a shortfall as
/// comments, a surplus taken from comments first (where tokei files fences),
/// then code.
fn reconcile(lang: LanguageType, langs: &mut BTreeMap<LanguageType, CodeNum>, nonblank: usize) {
    let counted: usize = langs.values().map(|num| num.code + num.comments).sum();
    if counted == nonblank {
        return;
    }
    tracing::debug!(?lang, counted, nonblank, "reconciling the line count");
    if counted < nonblank {
        langs.entry(lang).or_default().comments += nonblank - counted;
        return;
    }
    // A surplus comes out of the own language first, then the embedded ones;
    // it can never exceed what was counted, so it is always taken in full.
    let mut surplus = counted - nonblank;
    let others: Vec<LanguageType> = langs.keys().copied().filter(|&l| l != lang).collect();
    for key in std::iter::once(lang).chain(others) {
        let Some(num) = langs.get_mut(&key) else {
            continue;
        };
        for field in [&mut num.comments, &mut num.code] {
            let taken = surplus.min(*field);
            *field -= taken;
            surplus -= taken;
        }
    }
}

/// Flatten tokei's stats into per-language counts: the parent's own lines under
/// `lang`, each blob (recursively) under its own language.
fn from_stats(lang: LanguageType, stats: &CodeStats) -> FileCounts {
    let mut langs = BTreeMap::new();
    add_stats(lang, stats, &mut langs);
    let total = langs.values().fold(CodeNum::default(), |mut acc, num| {
        acc += *num;
        acc
    });
    FileCounts { total, langs }
}

fn add_stats(lang: LanguageType, stats: &CodeStats, langs: &mut BTreeMap<LanguageType, CodeNum>) {
    *langs.entry(lang).or_default() += CodeNum {
        code: stats.code,
        comments: stats.comments,
        blanks: stats.blanks,
    };
    for (child, blob) in &stats.blobs {
        add_stats(*child, blob, langs);
    }
}

/// The physical lines of `text`, without their `\n` terminators: one per `\n`,
/// plus a final unterminated line if the text does not end in `\n`. Empty text
/// has no lines.
pub fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    let empty = text.is_empty();
    body.split(|&b| b == b'\n').filter(move |_| !empty)
}

/// The number of physical lines in `text` (see [`lines`]).
#[cfg(test)]
fn physical_lines(text: &[u8]) -> usize {
    lines(text).count()
}

/// Characters that take no space but are not `char::is_whitespace`: zero-width
/// space, zero-width (non-)joiner, word joiner, and the byte-order mark.
const ZERO_WIDTH: [char; 5] = ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}'];

/// Whether `line` is blank in `lang`.
///
/// A line is blank when, once whitespace and zero-width characters are
/// removed, it is
///
/// - empty;
/// - a bare line-comment marker: one of `lang`'s symbolic line-comment markers
///   followed only by characters of that marker, `/`, or `!` (`//`, `///`,
///   `//!`, `#`, `##`, `--`). Markers spelled with letters or digits (Fortran's
///   `c`, Batch's `REM`) do not count: a lone `c` is as likely a statement;
/// - nothing but `*`, in a C-family language — one with both `/* */` and `//`
///   comments — where it is the bare continuation line of a doc block. (In
///   SQL, also `/* */`, a lone `*` is a select list.)
///
/// This holds in any comment or string state. Block-comment delimiters (`/*`,
/// `*/`) and punctuation (`}`) are not blank.
pub fn is_blank(lang: LanguageType, line: &[u8]) -> bool {
    let line = String::from_utf8_lossy(line);
    let mut content = line
        .chars()
        .filter(|c| !c.is_whitespace() && !ZERO_WIDTH.contains(c))
        .peekable();
    let Some(&first) = content.peek() else {
        return true;
    };
    let content: String = content.collect();
    if first == '*' && content.chars().all(|c| c == '*') && is_c_family(lang) {
        return true;
    }
    lang.line_comments().iter().any(|marker| {
        !marker.is_empty()
            && !marker.chars().any(|c| c.is_alphanumeric())
            && content.strip_prefix(marker).is_some_and(|rest| {
                rest.chars()
                    .all(|c| c == '/' || c == '!' || marker.contains(c))
            })
    })
}

/// Whether `lang` comments like C: `/* */` blocks and `//` lines.
fn is_c_family(lang: LanguageType) -> bool {
    lang.multi_line_comments()
        .iter()
        .any(|(open, _)| *open == "/*")
        && lang.line_comments().contains(&"//")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proptest::prelude::*;
    use tokei::{Config, LanguageType};

    use super::{FileCounts, count_file, is_blank, physical_lines};
    use crate::model::CodeNum;

    /// A scratch directory for collector tests.
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

        fn write(&self, rel: &str, body: impl AsRef<[u8]>) {
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

    fn num(code: usize, comments: usize, blanks: usize) -> CodeNum {
        CodeNum {
            code,
            comments,
            blanks,
        }
    }

    fn count(lang: LanguageType, text: &str) -> FileCounts {
        count_file(lang, text.as_bytes(), &Config::default())
    }

    fn tokei(lang: LanguageType, text: &str) -> CodeNum {
        let stats = lang.parse_from_str(text, &Config::default());
        num(stats.code, stats.comments, stats.blanks)
    }

    // --- What tokei itself does. These pin the behavior this module corrects;
    // if a tokei upgrade changes any of them, the correction needs revisiting.

    /// A whitespace-only line inside a block comment is a comment to tokei.
    #[test]
    fn tokei_counts_a_blank_line_in_a_block_comment_as_a_comment() {
        let text = "/*\n   \n*/\nfn main() {}\n";
        assert_eq!(tokei(LanguageType::Rust, text), num(1, 3, 0));
    }

    /// A blank line inside a Python docstring is code to tokei (docstrings are
    /// strings unless configured otherwise).
    #[test]
    fn tokei_counts_a_blank_line_in_a_docstring_as_code() {
        let text = "def f():\n    \"\"\"Doc.\n\n    More.\n    \"\"\"\n    return 1\n";
        assert_eq!(tokei(LanguageType::Python, text), num(6, 0, 0));
    }

    /// tokei's whitespace is ASCII only: a no-break space is code.
    #[test]
    fn tokei_counts_a_no_break_space_line_as_code() {
        let text = "fn main() {}\n\u{a0}\u{a0}\nfn f() {}\n";
        assert_eq!(tokei(LanguageType::Rust, text), num(3, 0, 0));
    }

    /// Rust doc comments go to a Markdown blob, outside the file's own counts.
    #[test]
    fn tokei_files_rust_doc_comments_under_a_markdown_blob() {
        let text = "/// One.\n/// Two.\nfn main() {}\n";
        let stats = LanguageType::Rust.parse_from_str(text, &Config::default());
        assert_eq!(num(stats.code, stats.comments, stats.blanks), num(1, 0, 0));
        assert!(stats.blobs.contains_key(&LanguageType::Markdown));
    }

    /// A Rust code line that holds a string literal, directly above a doc
    /// comment, vanishes from tokei's counts: the doc-block lookahead trims
    /// across the line end and skips the code line with it.
    #[test]
    fn tokei_drops_a_string_line_above_a_doc_comment() {
        let text = "let s = \"a\";\n//! d\n";
        let stats = LanguageType::Rust.parse_from_str(text, &Config::default());
        let all = stats.summarise();
        assert_eq!(num(all.code, all.comments, all.blanks), num(0, 1, 0));
    }

    // --- The blank rule.

    #[test]
    fn blank_lines_by_rule() {
        let rust = LanguageType::Rust;
        let python = LanguageType::Python;
        for line in [
            "", "   ", "\t", "\r", "\u{a0}", "\u{feff}", "\u{200b}", "\u{3000}", "//", "  // ",
            "///", "//!", "*", " * ", "**",
        ] {
            assert!(
                is_blank(rust, line.as_bytes()),
                "{line:?} should be blank in Rust"
            );
        }
        for line in ["#", "##", " # ", "#!"] {
            assert!(
                is_blank(python, line.as_bytes()),
                "{line:?} should be blank in Python"
            );
        }
        for line in ["}", "/*", "*/", "/**", "// x", "/// x", "x", "#", ";"] {
            assert!(
                !is_blank(rust, line.as_bytes()),
                "{line:?} is not blank in Rust"
            );
        }
        for line in ["*", "//", "# x", "pass"] {
            assert!(
                !is_blank(python, line.as_bytes()),
                "{line:?} is not blank in Python"
            );
        }
        assert!(is_blank(LanguageType::Lua, b"--"));
        assert!(!is_blank(LanguageType::Lua, b"-- x"));
        for zero_width in ["\u{200c}", "\u{200d}", "\u{2060}", " \u{200b}// \u{feff}"] {
            assert!(is_blank(rust, zero_width.as_bytes()), "{zero_width:?}");
        }
        assert!(!is_blank(rust, b"---"), "a marker followed by other text");
        assert!(!is_blank(rust, b"//x"));
        // A lone `*` is a select list in SQL, which has `/* */` but no `//`.
        assert!(!is_blank(LanguageType::Sql, b"    *"));
        assert!(is_blank(LanguageType::Sql, b"--"));
        // Markers spelled with letters are not bare markers.
        assert!(!is_blank(LanguageType::FortranLegacy, b"c"));
    }

    #[test]
    fn physical_line_counting() {
        assert_eq!(physical_lines(b""), 0);
        assert_eq!(physical_lines(b"\n"), 1);
        assert_eq!(physical_lines(b"a"), 1);
        assert_eq!(physical_lines(b"a\n"), 1);
        assert_eq!(physical_lines(b"a\nb"), 2);
        assert_eq!(physical_lines(b"a\n\n"), 2);
        assert_eq!(physical_lines(b"a\r\nb\r\n"), 2);
    }

    // --- Our counts.

    #[test]
    fn blank_lines_inside_comments_and_strings_are_blank() {
        let rust = count(LanguageType::Rust, "/*\n   \n*/\nfn main() {}\n");
        assert_eq!(rust.total, num(1, 2, 1));

        let python = count(
            LanguageType::Python,
            "def f():\n    \"\"\"Doc.\n\n    More.\n    \"\"\"\n    return 1\n",
        );
        assert_eq!(python.total, num(5, 0, 1));
    }

    #[test]
    fn unicode_whitespace_lines_are_blank() {
        let text = "fn main() {}\n\u{a0}\u{a0}\n\u{feff}\n\u{200b}\nfn f() {}\n";
        assert_eq!(count(LanguageType::Rust, text).total, num(2, 0, 3));
    }

    #[test]
    fn doc_comments_are_counted() {
        let text = "//! Crate.\n\n/// One.\n///\n/// Two.\nfn main() {}\n";
        let counts = count(LanguageType::Rust, text);
        // Three doc lines with content, one bare `///` and one empty line.
        assert_eq!(counts.total, num(1, 3, 2));
        assert_eq!(counts.total.lines(), 6);
    }

    #[test]
    fn javadoc_continuation_and_bare_markers_are_blank() {
        let text = "/**\n * Doc.\n *\n */\nclass A {\n  //\n}\n";
        let counts = count(LanguageType::Java, text);
        assert_eq!(counts.total, num(2, 3, 2));
    }

    #[test]
    fn punctuation_stays_code() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        assert_eq!(count(LanguageType::Rust, text).total, num(3, 0, 0));
    }

    #[test]
    fn crlf_and_a_missing_final_newline() {
        let text = "fn main() {}\r\n\r\n// c\r\nfn f() {}";
        assert_eq!(count(LanguageType::Rust, text).total, num(2, 1, 1));
    }

    #[test]
    fn the_empty_file_has_no_lines() {
        assert_eq!(count(LanguageType::Rust, "").total, CodeNum::default());
        assert!(count(LanguageType::Rust, "").langs.is_empty());
    }

    #[test]
    fn markdown_fenced_code_is_counted_under_its_language() {
        let text = "# Title\n\n```rust\nfn main() {}\n\n// c\n```\n";
        let counts = count(LanguageType::Markdown, text);
        assert_eq!(counts.total.lines(), physical_lines(text.as_bytes()));
        assert_eq!(counts.total.blanks, 2);
        assert_eq!(
            counts.langs.get(&LanguageType::Rust).map(|n| n.code),
            Some(1)
        );
    }

    /// One line of a generated source file, drawn from what tends to break line
    /// counters: comment openers and closers, doc comments, string delimiters,
    /// and every flavor of blank.
    fn vocabulary(lang: LanguageType) -> Vec<&'static str> {
        let blanks = vec!["", "   ", "\t", "\u{a0}", "\u{feff}", "\u{200b}"];
        let own: Vec<&'static str> = match lang {
            LanguageType::Python => vec![
                "x = 1",
                "# c",
                "#",
                "\"\"\"",
                "\"\"\"doc",
                "s = \"\"\"a",
                "'''",
                "def f():",
                "    return 1",
                "\"#\"",
            ],
            LanguageType::C => vec![
                "int x;",
                "// c",
                "//",
                "/*",
                "*/",
                " * d",
                " *",
                "/* c */",
                "}",
                "char *s = \"/*\";",
                "#include <a.h>",
            ],
            LanguageType::Markdown => vec![
                "# T",
                "text",
                "```",
                "```rust",
                "```python",
                "fn main() {}",
                "// c",
                "# c",
                "<!-- c -->",
                "- item",
                "~~~",
            ],
            _ => vec![
                "fn main() {",
                "}",
                "// c",
                "//",
                "/// d",
                "///",
                "//! d",
                "/*",
                "*/",
                " *",
                "/* c */",
                "let s = \"a",
                "b\";",
                "let r = r#\"x",
                "\"#;",
                "```",
                "```rust",
            ],
        };
        blanks.into_iter().chain(own).collect()
    }

    fn source(lang: LanguageType) -> impl Strategy<Value = String> {
        let words = vocabulary(lang);
        (
            prop::collection::vec(prop::sample::select(words), 0..40),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(lines, crlf, final_newline)| {
                let sep = if crlf { "\r\n" } else { "\n" };
                let mut text = lines.join(sep);
                if final_newline && !text.is_empty() {
                    text.push_str(sep);
                }
                text
            })
    }

    fn check_invariant(lang: LanguageType, text: &str) -> Result<(), TestCaseError> {
        let counts = count(lang, text);
        let bytes = text.as_bytes();
        prop_assert_eq!(counts.total.lines(), physical_lines(bytes));
        let blank = super::lines(bytes).filter(|l| is_blank(lang, l)).count();
        prop_assert_eq!(counts.total.blanks, blank);
        let summed = counts
            .langs
            .values()
            .fold(CodeNum::default(), |mut acc, n| {
                acc += *n;
                acc
            });
        prop_assert_eq!(summed, counts.total);
        Ok(())
    }

    proptest! {
        #[test]
        fn rust_counts_add_up(text in source(LanguageType::Rust)) {
            check_invariant(LanguageType::Rust, &text)?;
        }

        #[test]
        fn python_counts_add_up(text in source(LanguageType::Python)) {
            check_invariant(LanguageType::Python, &text)?;
        }

        #[test]
        fn c_counts_add_up(text in source(LanguageType::C)) {
            check_invariant(LanguageType::C, &text)?;
        }

        #[test]
        fn markdown_counts_add_up(text in source(LanguageType::Markdown)) {
            check_invariant(LanguageType::Markdown, &text)?;
        }
    }

    #[test]
    fn a_string_line_above_a_doc_comment_is_code() {
        let text = "let s = \"a\";\n//! d\n//// plain\nfn f() {}\n";
        let counts = count(LanguageType::Rust, text);
        assert_eq!(counts.total, num(2, 2, 0));
        assert_eq!(
            counts.langs.len(),
            1,
            "doc lines are the file's own comments"
        );
    }

    /// A doc-comment line that closes a block comment still closes it.
    #[test]
    fn a_doc_line_keeps_the_block_comment_state() {
        let counts = count(LanguageType::Rust, "/*\n/// */\nfn main() {}\n");
        assert_eq!(counts.total, num(1, 2, 0));
    }

    /// A `///*` line inside a block comment keeps the nested `/*` it opens, so
    /// the comment needs two closers.
    #[test]
    fn a_doc_line_keeps_a_nested_block_comment_opener() {
        let text = "/*\n///* inner\n*/\n*/\nfn main() {}\n";
        assert_eq!(count(LanguageType::Rust, text).total, num(1, 4, 0));
        let text = "/*\n//!/* inner\n*/\n*/\nfn main() {}\n";
        assert_eq!(count(LanguageType::Rust, text).total, num(1, 4, 0));
    }

    /// A doc-comment marker is classified as tokei classifies a plain `//` in
    /// the same place — here, inside a string literal that closes on the line.
    #[test]
    fn a_doc_marker_counts_as_a_plain_comment_would() {
        let counts = count(LanguageType::Rust, "let s = \"a\n/// b\";\n");
        assert_eq!(
            counts.total,
            tokei(LanguageType::Rust, "let s = \"a\n// b\";\n")
        );
    }

    #[test]
    fn jupyter_is_counted_as_tokei_counts_it() {
        let text = r##"{"cells":[{"cell_type":"code","source":["x = 1\n","\n","# c\n"]}],
            "metadata":{"kernelspec":{"language":"python"}}}"##;
        let ours = count(LanguageType::Jupyter, text);
        let theirs = LanguageType::Jupyter
            .parse_from_str(text, &Config::default())
            .summarise();
        assert_eq!(ours.total, num(theirs.code, theirs.comments, theirs.blanks));
    }

    /// tokei counts a fenced block as two fence lines plus its content, even
    /// when the closing fence carries an info string and so is also read as
    /// content: one line too many, which reconciling takes back out.
    #[test]
    fn a_fence_closed_with_an_info_string_still_adds_up() {
        let text = "```rust\n```rust\n";
        let counts = count(LanguageType::Markdown, text);
        assert_eq!(counts.total.lines(), 2);
    }

    /// The fallback for any line tokei still loses or invents: a shortfall
    /// becomes comments, a surplus comes out of comments, then code.
    #[test]
    fn reconcile_lands_the_difference_on_the_own_language() {
        let rust = LanguageType::Rust;
        let mut langs = std::collections::BTreeMap::from([(rust, num(3, 1, 0))]);
        super::reconcile(rust, &mut langs, 6);
        assert_eq!(langs[&rust], num(3, 3, 0));
        super::reconcile(rust, &mut langs, 2);
        assert_eq!(langs[&rust], num(2, 0, 0));
    }

    // --- The collector.

    #[test]
    fn dot_entries_are_counted() {
        let root = TempRoot::new("dotfiles");
        root.write(".github/workflows/ci.yml", "on: push\njobs:\n  a:\n");
        root.write("src/main.rs", "fn main() {}\n");
        let files = [
            PathBuf::from(".github/workflows/ci.yml"),
            PathBuf::from("src/main.rs"),
        ];

        let (counted, inaccurate) = super::collect_code(&root.0, &files);
        let ci = counted
            .get(Path::new(".github/workflows/ci.yml"))
            .expect("a file under a dot-directory is counted");
        assert!(ci.num.code > 0, "and it has real line counts");
        assert!(!inaccurate);
    }

    /// The collector counts exactly the files it is handed: a gitignored file
    /// the walk unions in from the index is counted, and nothing it was not
    /// handed (like `.git`) is.
    #[test]
    fn counts_exactly_the_walked_files() {
        let root = TempRoot::new("walked");
        root.write(".gitignore", "gen/\n");
        root.write("gen/forced.rs", "fn forced() {}\n");
        root.write("src/main.rs", "fn main() {}\n");
        root.write(".git/hooks/pre-commit", "#!/bin/sh\necho hi\n");
        let files = [PathBuf::from("gen/forced.rs"), PathBuf::from("src/main.rs")];

        let (counted, _) = super::collect_code(&root.0, &files);
        let mut keys: Vec<_> = counted.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, files.to_vec());
        assert_eq!(counted[Path::new("gen/forced.rs")].num, num(1, 0, 0));
    }

    #[test]
    fn utf16_files_are_decoded() {
        let root = TempRoot::new("utf16");
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "fn main() {}\n\n// c\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        root.write("a.rs", bytes);
        let (counted, _) = super::collect_code(&root.0, &[PathBuf::from("a.rs")]);
        assert_eq!(counted[Path::new("a.rs")].num, num(1, 1, 1));
    }

    /// A file removed since the walk is skipped, not an error: the watcher's
    /// rescan is already on its way.
    #[test]
    fn a_file_gone_since_the_walk_is_skipped() {
        let root = TempRoot::new("missing");
        let (counted, inaccurate) = super::collect_code(&root.0, &[PathBuf::from("gone.rs")]);
        assert!(counted.is_empty());
        assert!(!inaccurate);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_marks_the_result_inaccurate() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempRoot::new("unreadable");
        root.write("locked.rs", "fn main() {}\n");
        let path = root.0.join("locked.rs");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            return; // running as root: permissions do not bind, nothing to test
        }
        let (counted, inaccurate) = super::collect_code(&root.0, &[PathBuf::from("locked.rs")]);
        assert!(counted.is_empty());
        assert!(inaccurate);
    }

    /// Symlinks are skipped, as tokei's own walk skips them: a link to a
    /// counted file would count it twice, and a dangling one is not an error.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_counted() {
        let root = TempRoot::new("symlinks");
        root.write("real.rs", "fn main() {}\n");
        std::os::unix::fs::symlink(root.0.join("real.rs"), root.0.join("link.rs")).unwrap();
        std::os::unix::fs::symlink(root.0.join("missing/x.json"), root.0.join("dangling.json"))
            .unwrap();
        let files = [
            PathBuf::from("real.rs"),
            PathBuf::from("link.rs"),
            PathBuf::from("dangling.json"),
        ];
        let (counted, inaccurate) = super::collect_code(&root.0, &files);
        let keys: Vec<_> = counted.keys().cloned().collect();
        assert_eq!(keys, vec![PathBuf::from("real.rs")]);
        assert!(!inaccurate);
    }

    /// A file is colored by the language it was detected as, even when an
    /// embedded language holds more of its lines.
    #[test]
    fn the_primary_language_is_the_detected_one() {
        let root = TempRoot::new("primary");
        root.write(
            "r.md",
            "# T\n\n```rust\nfn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\n```\n",
        );
        let (counted, _) = super::collect_code(&root.0, &[PathBuf::from("r.md")]);
        let data = &counted[Path::new("r.md")];
        assert_eq!(data.primary_lang, Some(LanguageType::Markdown));
        assert!(data.langs[&LanguageType::Rust].code > data.langs[&LanguageType::Markdown].lines());
    }
}
