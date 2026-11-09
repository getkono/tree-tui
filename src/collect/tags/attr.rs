//! Rust attribute meaning: whether an attribute marks the item it sits on as
//! test code.
//!
//! An attribute's text (`#[cfg(all(test, unix))]`) is parsed into a small
//! meta tree — a path, then an optional parenthesized list or `= value` — and
//! read by two rules: test-harness attributes (`#[test]` and its kin) mark
//! their item, and a `cfg` whose predicate *requires* `test` does.

/// A parsed attribute meta item: `path`, `path(list…)`, or `path = value`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    /// The path, `::`-joined (`tokio::test`).
    path: String,
    /// The parenthesized arguments, when present.
    list: Option<Vec<Meta>>,
}

/// The attribute paths that mark a test function on their own, besides any
/// path ending in `::test` (`tokio::test`, `async_std::test`, `test_log::test`).
const TEST_ATTRS: &[&str] = &[
    "test",
    "rstest",
    "test_case",
    "quickcheck",
    "proptest",
    "wasm_bindgen_test",
];

/// Whether the attribute `text` (an outer `#[…]` or inner `#![…]`) marks its
/// item as test code.
///
/// - A test-harness attribute: `#[test]`, `#[tokio::test]` and other
///   `…::test` paths, `#[rstest]`, `#[test_case(…)]`, `#[quickcheck]`,
///   `#[proptest]`, `#[wasm_bindgen_test]`.
/// - `#[cfg(P)]` where `P` *requires* `test` — the item exists only in test
///   builds: `test` does; `all(…)` does if any argument does; `any(…)` does
///   only if every argument does; `not(…)` never does.
///
/// A harness attribute is known by its path alone — its arguments are the
/// harness's own syntax (`#[test_case(1 => 2 ; "named")]`). A `cfg` is parsed
/// in full; one that does not parse marks nothing.
pub fn marks_test(text: &str) -> bool {
    let Some(path) = attribute_path(text) else {
        return false;
    };
    if path == "cfg" {
        return match parse_attribute(text).and_then(|meta| meta.list) {
            Some(list) => matches!(list.as_slice(), [predicate] if requires_test(predicate)),
            None => false,
        };
    }
    TEST_ATTRS.contains(&path.as_str()) || path.ends_with("::test")
}

/// An attribute's path (`tokio::test`), if its text is `#[path…]` or
/// `#![path…]` with the path followed by nothing, `(`, or `=`.
pub fn attribute_path(text: &str) -> Option<String> {
    let body = text.trim().strip_prefix('#')?.trim_start();
    let body = body.strip_prefix('!').unwrap_or(body).trim_start();
    let body = body.strip_prefix('[')?.strip_suffix(']')?;
    let end = body
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c.is_whitespace()))
        .unwrap_or(body.len());
    let rest = &body[end..];
    if !(rest.is_empty() || rest.starts_with('(') || rest.starts_with('=')) {
        return None;
    }
    let path: String = body[..end].chars().filter(|c| !c.is_whitespace()).collect();
    let well_formed = !path.is_empty()
        && path
            .split("::")
            .all(|seg| !seg.is_empty() && !seg.starts_with(|c: char| c.is_ascii_digit()));
    well_formed.then_some(path)
}

/// Whether the `cfg` predicate holds only when `test` is set.
fn requires_test(predicate: &Meta) -> bool {
    match (predicate.path.as_str(), predicate.list.as_deref()) {
        ("test", None) => true,
        ("all", Some(args)) => args.iter().any(requires_test),
        ("any", Some(args)) => !args.is_empty() && args.iter().all(requires_test),
        _ => false,
    }
}

/// Parse `#[meta]` or `#![meta]`.
fn parse_attribute(text: &str) -> Option<Meta> {
    let body = text.trim().strip_prefix('#')?.trim_start();
    let body = body.strip_prefix('!').unwrap_or(body).trim_start();
    let body = body.strip_prefix('[')?.strip_suffix(']')?;
    let tokens = tokenize(body)?;
    let mut pos = 0;
    let meta = parse_meta(&tokens, &mut pos)?;
    (pos == tokens.len()).then_some(meta)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    PathSep,
    Open,
    Close,
    Comma,
    Eq,
    /// A literal (string, number, char) — only ever a `= value`.
    Literal,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            // Comments inside an attribute: `// …` to the line end, `/* … */`.
            '/' => {
                chars.next();
                match chars.next()?.1 {
                    '/' => {
                        for (_, c) in chars.by_ref() {
                            if c == '\n' {
                                break;
                            }
                        }
                    }
                    '*' => {
                        let mut star = false;
                        loop {
                            let (_, c) = chars.next()?;
                            if star && c == '/' {
                                break;
                            }
                            star = c == '*';
                        }
                    }
                    _ => return None,
                }
            }
            '(' => {
                chars.next();
                tokens.push(Token::Open);
            }
            ')' => {
                chars.next();
                tokens.push(Token::Close);
            }
            ',' => {
                chars.next();
                tokens.push(Token::Comma);
            }
            '=' => {
                chars.next();
                tokens.push(Token::Eq);
            }
            ':' => {
                chars.next();
                if chars.next()?.1 != ':' {
                    return None;
                }
                tokens.push(Token::PathSep);
            }
            '"' => {
                chars.next();
                let mut escaped = false;
                loop {
                    let (_, c) = chars.next()?;
                    match (escaped, c) {
                        (false, '\\') => escaped = true,
                        (false, '"') => break,
                        _ => escaped = false,
                    }
                }
                tokens.push(Token::Literal);
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                let mut end = i;
                while let Some(&(j, c)) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        end = j + c.len_utf8();
                        chars.next();
                    } else {
                        break;
                    }
                }
                let word = &text[start..end];
                if word.starts_with(|c: char| c.is_ascii_digit()) {
                    tokens.push(Token::Literal);
                } else {
                    tokens.push(Token::Ident(word.to_string()));
                }
            }
            _ => return None,
        }
    }
    Some(tokens)
}

fn parse_meta(tokens: &[Token], pos: &mut usize) -> Option<Meta> {
    let mut path = match tokens.get(*pos)? {
        Token::Ident(word) => word.clone(),
        _ => return None,
    };
    *pos += 1;
    while tokens.get(*pos) == Some(&Token::PathSep) {
        let Some(Token::Ident(word)) = tokens.get(*pos + 1) else {
            return None;
        };
        path.push_str("::");
        path.push_str(word);
        *pos += 2;
    }
    match tokens.get(*pos) {
        Some(Token::Open) => {
            *pos += 1;
            let mut list = Vec::new();
            loop {
                if tokens.get(*pos) == Some(&Token::Close) {
                    *pos += 1;
                    break;
                }
                // Arguments are metas, or bare literals (`#[test_case(1, 2)]`).
                if tokens.get(*pos) == Some(&Token::Literal) {
                    *pos += 1;
                } else {
                    list.push(parse_meta(tokens, pos)?);
                }
                match tokens.get(*pos)? {
                    Token::Comma => *pos += 1,
                    Token::Close => {}
                    _ => return None,
                }
            }
            Some(Meta {
                path,
                list: Some(list),
            })
        }
        Some(Token::Eq) => {
            *pos += 1;
            match tokens.get(*pos)? {
                Token::Literal | Token::Ident(_) => *pos += 1,
                _ => return None,
            }
            Some(Meta { path, list: None })
        }
        _ => Some(Meta { path, list: None }),
    }
}

#[cfg(test)]
mod tests {
    use super::marks_test;

    #[test]
    fn harness_attributes_mark_test() {
        for attr in [
            "#[test]",
            "# [ test ]",
            "#[tokio::test]",
            "#[tokio::test(flavor = \"multi_thread\", worker_threads = 2)]",
            "#[async_std::test]",
            "#[test_log::test]",
            "#[rstest]",
            "#[test_case(1, 2 ; \"named\")]",
            "#[test_case(1 => 2)]",
            "#[test_case(-1)]",
            "#[test_case(1.5)]",
            "#[test_case(1, 2)]",
            "#[quickcheck]",
            "#[proptest]",
            "#[wasm_bindgen_test]",
        ] {
            assert!(marks_test(attr), "{attr}");
        }
    }

    #[test]
    fn comments_inside_an_attribute_are_skipped() {
        assert!(marks_test(
            "#[cfg(all(\n    test, // only in tests\n    unix))]"
        ));
        assert!(marks_test("#[cfg(/* x */ test)]"));
        assert!(!marks_test("#[cfg(not(/* x */ test))]"));
    }

    #[test]
    fn attribute_paths() {
        use super::attribute_path;
        assert_eq!(
            attribute_path("#[tokio :: test]").as_deref(),
            Some("tokio::test")
        );
        assert_eq!(attribute_path("#![cfg(test)]").as_deref(), Some("cfg"));
        assert_eq!(
            attribute_path("#[path = \"x.rs\"]").as_deref(),
            Some("path")
        );
        assert_eq!(attribute_path("#[1test]"), None);
        assert_eq!(attribute_path("#[a::]"), None);
        assert_eq!(attribute_path("#[a;b]"), None);
    }

    #[test]
    fn cfg_marks_test_only_when_it_requires_test() {
        for attr in [
            "#[cfg(test)]",
            "#![cfg(test)]",
            "#[cfg(all(test, unix))]",
            "#[cfg(all(unix, any(test, test)))]",
            "#[cfg(all(feature = \"x\", test))]",
            "#[cfg(any(test, all(test, unix)))]",
        ] {
            assert!(marks_test(attr), "{attr}");
        }
        for attr in [
            "#[cfg(not(test))]",
            "#[cfg(any(test, feature = \"x\"))]",
            "#[cfg(any())]",
            "#[cfg(all())]",
            "#[cfg(unix)]",
            "#[cfg(test, unix)]",
            "#[cfg_attr(test, derive(Debug))]",
            "#[allow(dead_code)]",
            "#[derive(Debug)]",
            "#[bench]",
            "#[testing]",
            "#[my::tested]",
            "#[doc = \"test\"]",
            "#[path = \"tests.rs\"]",
            "not an attribute",
            "#[cfg(test]",
        ] {
            assert!(!marks_test(attr), "{attr}");
        }
    }
}
