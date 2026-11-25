use std::ops::Range;

use tree_sitter::Node;

/// Whether a project-relative path is test code by convention: under a `tests` or
/// `benches` folder, or a `test(s).*` / `*_test(s).*` / `*.test.*` / `*.spec.*` file.
pub fn is_test_path(path: &str) -> bool {
    let mut components = path.split('/').rev();
    let Some(file_name) = components.next() else {
        return false;
    };
    if components.any(|dir| matches!(dir, "tests" | "test" | "benches" | "__tests__")) {
        return true;
    }
    let mut parts = file_name.split('.');
    let stem = parts.next().unwrap_or_default();
    matches!(stem, "test" | "tests")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || parts.any(|part| matches!(part, "test" | "spec"))
}

/// Whether a Rust attribute marks its item as test-only: `#[test]`, `#[cfg(test)]`,
/// `#[gpui::test]`, `#[bench]`, a `test-support` feature gate, ... Purely textual.
pub fn is_test_attribute(attribute: &str) -> bool {
    !attribute.contains("not(test")
        && attribute
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|token| matches!(token, "test" | "bench" | "rstest"))
}

/// Whether `range` (byte offsets into the tree `root` was parsed from) sits inside a Rust
/// item carrying a test attribute. `text_for` returns the source text of a byte range.
pub fn in_test_item(root: Node, range: Range<usize>, text_for: impl Fn(Range<usize>) -> String) -> bool {
    let mut node = root.descendant_for_byte_range(range.start, range.end);
    while let Some(item) = node {
        if item.kind().ends_with("_item") {
            let mut sibling = item.prev_sibling();
            while let Some(previous) = sibling {
                match previous.kind() {
                    "attribute_item" if is_test_attribute(&text_for(previous.byte_range())) => return true,
                    "attribute_item" | "line_comment" | "block_comment" => sibling = previous.prev_sibling(),
                    _ => break,
                }
            }
        }
        node = item.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_test_path_follows_conventions() {
        for path in [
            "tests/it.rs",
            "crates/a/tests/it.rs",
            "crates/a/benches/b.rs",
            "src/foo/tests.rs",
            "src/foo_test.rs",
            "src/foo_tests.rs",
            "pkg/foo_test.go",
            "py/test_foo.py",
            "web/foo.test.ts",
            "web/foo.spec.tsx",
            "web/__tests__/foo.ts",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in ["src/foo.rs", "src/latest.rs", "src/testing.rs", "src/contest.rs", "attest/foo.rs", ""] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    #[test]
    fn is_test_attribute_matches_test_tokens() {
        for attribute in [
            "#[test]",
            "#[cfg(test)]",
            "#[tokio::test]",
            "#[gpui::test(iterations = 10)]",
            "#[bench]",
            "#[rstest]",
            r#"#[cfg(any(test, feature = "test-support"))]"#,
        ] {
            assert!(is_test_attribute(attribute), "{attribute}");
        }
        for attribute in ["#[cfg(not(test))]", "#[inline]", "#[derive(Debug)]", r#"#[cfg(feature = "latest")]"#] {
            assert!(!is_test_attribute(attribute), "{attribute}");
        }
    }
}
