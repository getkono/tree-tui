//! Whole-file test detection by path: the directory and file-name conventions
//! test runners and build tools use to find test files.

use std::path::{Component, Path};

/// Directory names that hold only test code (or test data), wherever they sit.
/// `test` covers Maven/Gradle's `src/test/`; `testdata` is Go's fixture
/// directory.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec", "testdata"];

/// The test directory of the languages read in-file (Rust, Zig): Cargo's
/// integration-test directory. Their other modules are found through the
/// source, where a `pub mod test` or `mod spec` is as likely shipped code.
const IN_FILE_TEST_DIRS: &[&str] = &["tests"];

/// Whether `rel` (relative to the scan root) is a test file by convention:
/// it sits under a test directory, or its name is a test runner's pattern.
/// `in_file` languages (read by syntax) only honor [`IN_FILE_TEST_DIRS`].
///
/// | Ecosystem | File names |
/// | --- | --- |
/// | Go | `*_test.go` |
/// | Python (pytest) | `test_*.py`, `*_test.py`, `conftest.py` |
/// | JS / TS (Jest, Vitest, Mocha) | `*.test.*`, `*.spec.*` over `js jsx ts tsx mjs cjs` |
/// | Ruby (RSpec, Minitest) | `*_spec.rb`, `*_test.rb` |
/// | Java, Kotlin, PHP, Swift, C# | `*Test.*`, `*Tests.*` |
/// | Dart | `*_test.dart` |
/// | Elixir | `*_test.exs` |
/// | C / C++ (GoogleTest) | `*_test.{c,cc,cpp}`, `*_unittest.{cc,cpp}` |
pub fn is_test_path(rel: &Path, in_file: bool) -> bool {
    let mut components: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let Some(name) = components.pop() else {
        return false;
    };
    let dirs = if in_file {
        IN_FILE_TEST_DIRS
    } else {
        TEST_DIRS
    };
    components.iter().any(|dir| dirs.contains(dir)) || is_test_file_name(name)
}

/// Directory names that hold benchmarks or examples: Cargo's `benches/` and
/// `examples/`, and the same names elsewhere.
const BENCH_DIRS: &[&str] = &["benches", "bench", "benchmarks", "examples", "example"];

/// Whether `rel` sits under a benchmark or example directory.
///
/// Only directories above the first `src` count: below it, `example` is a
/// package name as often as not (`src/main/java/com/example/…` is every
/// Spring and Android starter), and a Rust `src/…/example/` is a module.
pub fn is_bench_path(rel: &Path) -> bool {
    let mut components: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    components.pop(); // the file name
    components
        .iter()
        .take_while(|&&dir| dir != "src")
        .any(|dir| BENCH_DIRS.contains(dir))
}

fn is_test_file_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    match ext {
        "go" => stem.ends_with("_test"),
        "py" => stem.starts_with("test_") || stem.ends_with("_test") || stem == "conftest",
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            stem.ends_with(".test") || stem.ends_with(".spec")
        }
        "rb" => stem.ends_with("_spec") || stem.ends_with("_test"),
        "java" | "kt" | "php" | "swift" | "cs" => {
            stem.len() > 4 && (stem.ends_with("Test") || stem.ends_with("Tests"))
        }
        "dart" => stem.ends_with("_test"),
        "exs" => stem.ends_with("_test"),
        "c" | "cc" | "cpp" => {
            stem.ends_with("_test") || (ext != "c" && stem.ends_with("_unittest"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_test_path;

    #[test]
    fn test_directories() {
        for path in [
            "tests/cli.rs",
            "crates/a/tests/common/mod.rs",
            "src/test/java/com/x/Foo.java",
            "web/__tests__/app.js",
            "spec/models/user.rb",
            "pkg/testdata/input.json",
        ] {
            assert!(is_test_path(Path::new(path), false), "{path}");
        }
    }

    #[test]
    fn test_file_names() {
        for path in [
            "pkg/walk_test.go",
            "app/test_models.py",
            "app/models_test.py",
            "conftest.py",
            "src/app.test.ts",
            "src/app.spec.jsx",
            "lib/user_spec.rb",
            "lib/user_test.rb",
            "src/main/java/FooTest.java",
            "src/FooTests.cs",
            "Sources/AppTests.swift",
            "lib/widget_test.dart",
            "test_helper_test.exs",
            "src/walk_test.cc",
            "src/walk_unittest.cpp",
        ] {
            assert!(is_test_path(Path::new(path), false), "{path}");
        }
    }

    #[test]
    fn not_test_paths() {
        for path in [
            "src/main.rs",
            "src/tests.rs", // a Rust module file: decided by its `mod` declaration
            "src/testing/mod.rs", // `testing` is not a test directory
            "contest.py",
            "latest.go",
            "src/Test.java", // just `Test`: a class named Test
            "src/attest.ts",
            "src/protest.rb",
            "tests", // a file named `tests`, not a directory
            "src/spec.md",
            "src/walk_unittest.c", // GoogleTest is C++
            "README.md",
        ] {
            assert!(!is_test_path(Path::new(path), false), "{path}");
        }
    }

    #[test]
    fn bench_and_example_directories() {
        use super::is_bench_path;
        for path in [
            "benches/walk.rs",
            "crates/a/examples/demo.rs",
            "bench/x.go",
            "benchmarks/run.py",
            "example/main.go",
        ] {
            assert!(is_bench_path(Path::new(path)), "{path}");
        }
        for path in [
            "benches",
            "src/bench.rs",
            "src/examples.rs",
            "src/main.rs",
            // Package names and modules below `src`.
            "src/main/java/com/example/demo/DemoApplication.java",
            "app/src/main/java/com/example/myapplication/MainActivity.kt",
            "src/lr1/example/mod.rs",
        ] {
            assert!(!is_bench_path(Path::new(path)), "{path}");
        }
        // Above `src` still counts: a crate under an examples directory.
        assert!(is_bench_path(Path::new("examples/demo/src/main.rs")));
    }

    /// For the languages read in-file only Cargo's `tests/` counts: a Rust
    /// `src/test/` or `src/spec/` is as likely a shipped `pub mod`.
    #[test]
    fn in_file_languages_only_honor_tests() {
        assert!(is_test_path(Path::new("tests/cli.rs"), true));
        assert!(is_test_path(
            Path::new("crates/a/tests/common/mod.rs"),
            true
        ));
        for path in ["src/test/mod.rs", "src/spec/eval.rs", "src/testdata/x.rs"] {
            assert!(!is_test_path(Path::new(path), true), "{path}");
            assert!(is_test_path(Path::new(path), false), "{path}");
        }
    }
}
