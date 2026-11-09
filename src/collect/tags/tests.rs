use std::path::{Path, PathBuf};

use tokei::LanguageType;

use super::{Detector, ModulePath, Tagging};
use crate::model::TagSet;

fn tag(lang: LanguageType, rel: &str, text: &str) -> Tagging {
    Detector::new().tag(lang, Path::new(rel), text.as_bytes())
}

/// The 1-based line numbers tagged test, which reads naturally against a
/// fixture written one line per item.
fn test_lines(lang: LanguageType, rel: &str, text: &str) -> Vec<usize> {
    let tagging = tag(lang, rel, text);
    (0..text.lines().count())
        .filter(|&row| tagging.at(row).intersects(TagSet::TEST))
        .map(|row| row + 1)
        .collect()
}

fn rust(text: &str) -> Vec<usize> {
    test_lines(LanguageType::Rust, "src/lib.rs", text)
}

#[test]
fn a_cfg_test_module_is_tagged_with_its_attribute() {
    let text = "\
fn shipped() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn works() {}
}

fn after() {}
";
    assert_eq!(rust(text), vec![3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn test_functions_outside_a_test_module() {
    let text = "\
fn shipped() {}
#[test]
fn a() {
    assert!(true);
}
#[tokio::test(flavor = \"multi_thread\")]
async fn b() {}
fn also_shipped() {}
";
    assert_eq!(rust(text), vec![2, 3, 4, 5, 6, 7]);
}

#[test]
fn cfg_predicates_that_do_not_require_test_tag_nothing() {
    let text = "\
#[cfg(not(test))]
fn real() {}
#[cfg(any(test, feature = \"x\"))]
fn maybe() {}
#[cfg(all(test, unix))]
fn unix_tests() {}
";
    assert_eq!(rust(text), vec![5, 6]);
}

#[test]
fn doc_comments_and_other_attributes_go_with_the_item() {
    let text = "\
fn shipped() {}

/// Helpers for the tests.
// (and a plain comment)
#[allow(dead_code)]
#[cfg(test)]
#[derive(Debug)]
struct Fixture;
";
    assert_eq!(rust(text), vec![3, 4, 5, 6, 7, 8]);
}

#[test]
fn a_blank_line_or_a_trailing_comment_ends_the_attached_run() {
    let text = "\
// Unrelated header comment.

#[cfg(test)]
fn a() {}
fn b() {} // trailing
#[test]
fn c() {}
";
    assert_eq!(rust(text), vec![3, 4, 6, 7]);
}

#[test]
fn an_attribute_then_a_comment_then_the_item() {
    let text = "\
#[cfg(test)]
// The tests.
/* block */
mod tests {}
";
    assert_eq!(rust(text), vec![1, 2, 3, 4]);
}

/// An attribute on something that is not an item — a field, a statement —
/// must not reach past it to the next item.
#[test]
fn an_attribute_on_a_non_item_does_not_escape() {
    let text = "\
struct S {
    #[cfg(test)]
    probe: u8,
}
fn after() {}
fn f() {
    #[cfg(test)]
    let x = 1;
}
fn later() {}
";
    assert_eq!(rust(text), Vec::<usize>::new());
}

#[test]
fn test_items_nested_in_shipped_ones() {
    let text = "\
impl S {
    fn real(&self) {}

    #[cfg(test)]
    fn probe(&self) {}
}
";
    assert_eq!(rust(text), vec![4, 5]);
}

#[test]
fn an_inner_cfg_test_tags_the_file_or_its_module() {
    let whole = tag(
        LanguageType::Rust,
        "src/helpers.rs",
        "//! Test helpers.\n#![cfg(test)]\nfn a() {}\n",
    );
    assert_eq!(whole.whole, TagSet::TEST);

    let text = "\
fn shipped() {}
mod probes {
    #![cfg(test)]
    fn a() {}
}
";
    assert_eq!(rust(text), vec![2, 3, 4, 5]);
}

#[test]
fn an_out_of_line_test_module_names_its_files() {
    let lib = tag(
        LanguageType::Rust,
        "src/lib.rs",
        "#[cfg(test)]\nmod tests;\nmod shipped;\n",
    );
    assert_eq!(
        lib.test_modules,
        vec![
            ModulePath::File(PathBuf::from("src/tests.rs")),
            ModulePath::Dir(PathBuf::from("src/tests")),
        ]
    );

    // A non-`mod.rs` file's modules live in a directory named after it.
    let nested = tag(
        LanguageType::Rust,
        "src/ui/view.rs",
        "#[cfg(test)]\nmod tests;\n",
    );
    assert_eq!(
        nested.test_modules,
        vec![
            ModulePath::File(PathBuf::from("src/ui/view/tests.rs")),
            ModulePath::Dir(PathBuf::from("src/ui/view/tests")),
        ]
    );

    // An inline module is tagged in place and names no files.
    let inline = tag(
        LanguageType::Rust,
        "src/lib.rs",
        "#[cfg(test)]\nmod tests {}\n",
    );
    assert!(inline.test_modules.is_empty());
}

#[test]
fn module_paths_match_files_and_directories() {
    let file = ModulePath::File(PathBuf::from("src/tests.rs"));
    let dir = ModulePath::Dir(PathBuf::from("src/tests"));
    assert!(file.contains(Path::new("src/tests.rs")));
    assert!(!file.contains(Path::new("src/tests.rs.bak")));
    assert!(dir.contains(Path::new("src/tests/mod.rs")));
    assert!(dir.contains(Path::new("src/tests/deep/x.rs")));
    assert!(!dir.contains(Path::new("src/tests_extra.rs")));
}

#[test]
fn a_test_path_tags_the_whole_file_without_parsing() {
    let tagging = tag(LanguageType::Rust, "tests/cli.rs", "fn main() {}\n");
    assert_eq!(tagging.whole, TagSet::TEST);
    let go = tag(LanguageType::Go, "pkg/x_test.go", "package x\n");
    assert_eq!(go.whole, TagSet::TEST);
}

#[test]
fn other_languages_are_not_read_in_file() {
    let text = "def test_a():\n    pass\n";
    assert_eq!(
        tag(LanguageType::Python, "app/x.py", text),
        Tagging::default()
    );
}

#[test]
fn zig_test_blocks_with_their_doc_comments() {
    let text = "\
const std = @import(\"std\");
fn add(a: i32, b: i32) i32 {
    return a + b;
}

/// Adds.
test \"add\" {
    try std.testing.expect(add(1, 2) == 3);
}
";
    assert_eq!(
        test_lines(LanguageType::Zig, "src/add.zig", text),
        vec![6, 7, 8, 9]
    );
}

/// This repository's own sources: `src/app.rs` keeps its tests in an inline
/// `#[cfg(test)] mod tests`, which must come out tagged, and its shipped code
/// must not.
#[test]
fn this_repositorys_app_module() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app.rs");
    let text = std::fs::read_to_string(path).expect("read src/app.rs");
    let tagged = rust(&text);
    let module = text
        .lines()
        .position(|line| line.starts_with("mod tests {"))
        .expect("app.rs has an inline test module")
        + 1;
    assert!(tagged.contains(&module), "the module line is tagged");
    assert!(tagged.contains(&(module - 1)), "and its #[cfg(test)]");
    assert!(
        tagged.iter().all(|&line| line >= module - 1),
        "nothing before the test module is tagged"
    );
    assert_eq!(*tagged.last().unwrap(), text.lines().count());
}

/// A doc line comment's node includes its line break: one blank line after
/// it still separates it from the item. Inner docs never attach.
#[test]
fn a_blank_line_after_a_doc_comment_separates_it() {
    let text = "\
//! Crate docs.

#[cfg(test)]
mod tests {}
/// orphan

#[test]
fn t() {}
";
    assert_eq!(rust(text), vec![3, 4, 7, 8]);

    let inner = "\
mod m {
    //! Module docs.
    #[cfg(test)]
    fn probe() {}
}
";
    assert_eq!(rust(inner), vec![3, 4]);
}

/// Upward extension stops at code: a comment preceded by code on its line is
/// a trailing comment, even when it is a block comment.
#[test]
fn code_before_a_comment_stops_the_extension() {
    let text = "\
/* c */ fn b() {}
#[test]
fn t() {}
";
    assert_eq!(rust(text), vec![2, 3]);
}

/// `#![cfg(test)]` marks the innermost enclosing item, not an outer one.
#[test]
fn an_inner_cfg_marks_the_innermost_item() {
    let text = "\
mod outer {
    fn shipped() {}
    mod inner {
        #![cfg(test)]
        fn probe() {}
    }
}
";
    assert_eq!(rust(text), vec![3, 4, 5, 6]);
}

#[test]
fn harness_attributes_with_their_own_argument_syntax() {
    let text = "\
#[test_case(1 => 2 ; \"named\")]
#[test_case(-1)]
fn f(x: i32) {}
";
    assert_eq!(rust(text), vec![1, 2, 3]);
}

#[test]
fn module_paths_from_main_and_mod_files_and_raw_names() {
    let main = tag(
        LanguageType::Rust,
        "src/main.rs",
        "#[cfg(test)]\nmod tests;\n",
    );
    assert_eq!(
        main.test_modules[0],
        ModulePath::File(PathBuf::from("src/tests.rs"))
    );
    let module = tag(
        LanguageType::Rust,
        "src/ui/mod.rs",
        "#[cfg(test)]\nmod r#async;\n",
    );
    assert_eq!(
        module.test_modules,
        vec![
            ModulePath::File(PathBuf::from("src/ui/async.rs")),
            ModulePath::Dir(PathBuf::from("src/ui/async")),
        ]
    );
}

/// A `#[path]` module lives somewhere this does not resolve; its name must not
/// tag the file it would otherwise point at.
#[test]
fn a_path_module_names_no_files() {
    let lib = tag(
        LanguageType::Rust,
        "src/lib.rs",
        "#[cfg(test)]\n#[path = \"t/x.rs\"]\nmod tests;\n",
    );
    assert!(lib.test_modules.is_empty());
    assert_eq!(lib.regions.len(), 1, "the declaration itself is still test");
}

/// For Rust, only Cargo's `tests/` is a test directory by path: a shipped
/// `pub mod test` under `src/test/` is read in-file like any module.
#[test]
fn a_rust_test_module_directory_is_read_in_file() {
    let shipped = tag(
        LanguageType::Rust,
        "src/test/mod.rs",
        "pub fn helper() {}\n",
    );
    assert_eq!(shipped.whole, TagSet::EMPTY);
    let java = tag(
        LanguageType::Java,
        "src/test/java/FooIT.java",
        "class FooIT {}\n",
    );
    assert_eq!(java.whole, TagSet::TEST);
}

#[test]
fn files_without_a_marker_or_too_large_are_not_parsed() {
    // `#[quickcheck]` is the one marker without `test` in it.
    assert_eq!(rust("#[quickcheck]\nfn prop() {}\n"), vec![1, 2]);
    let huge = format!(
        "#[test]\nfn t() {{}}\n{}",
        "// pad\n".repeat(super::MAX_PARSE_BYTES / 7 + 1)
    );
    assert!(
        tag(LanguageType::Rust, "src/big.rs", &huge)
            .regions
            .is_empty()
    );
}

#[test]
fn non_utf8_files_are_not_read_in_file() {
    let mut bytes = b"#[test]\nfn t() { let s = \"".to_vec();
    bytes.push(0xFF);
    bytes.extend_from_slice(b"\"; }\n");
    let tagging = Detector::new().tag(LanguageType::Rust, Path::new("src/x.rs"), &bytes);
    assert_eq!(tagging, Tagging::default());
}

/// The 1-based lines carrying `tags` (all of them).
fn lines_with(lang: LanguageType, rel: &str, text: &str, tags: TagSet) -> Vec<usize> {
    let tagging = tag(lang, rel, text);
    (0..text.lines().count())
        .filter(|&row| {
            let at = tagging.at(row);
            at.union(tags) == at
        })
        .map(|row| row + 1)
        .collect()
}

#[test]
fn a_bench_function_is_tagged_bench_not_test() {
    let text = "\
fn shipped() {}
#[bench]
fn bench_walk(b: &mut Bencher) {}
";
    assert_eq!(
        lines_with(LanguageType::Rust, "src/lib.rs", text, TagSet::BENCH),
        vec![2, 3]
    );
    assert!(rust(text).is_empty());
}

/// A `#[bench]` inside a `cfg(test)` module carries both tags.
#[test]
fn a_bench_inside_a_test_module_carries_both_tags() {
    let text = "\
#[cfg(test)]
mod tests {
    #[bench]
    fn b(b: &mut Bencher) {}
}
";
    let both = TagSet::TEST.union(TagSet::BENCH);
    assert_eq!(
        lines_with(LanguageType::Rust, "src/lib.rs", text, both),
        vec![3, 4]
    );
    assert_eq!(rust(text), vec![1, 2, 3, 4, 5]);
}

#[test]
fn bench_and_example_directories_tag_whole_files() {
    for rel in ["benches/walk.rs", "examples/demo.rs", "bench/x_test.go"] {
        let tagging = tag(LanguageType::Rust, rel, "fn main() {}\n");
        assert!(tagging.whole.intersects(TagSet::BENCH), "{rel}");
    }
    // A test file under a benchmark directory is both.
    let both = tag(LanguageType::Go, "bench/x_test.go", "package x\n");
    assert_eq!(both.whole, TagSet::TEST.union(TagSet::BENCH));
}
