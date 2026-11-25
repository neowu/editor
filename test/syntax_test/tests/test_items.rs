//! `language::in_test_item` over a real Rust grammar parse.

// The whole file is a test module.
#![allow(clippy::tests_outside_test_module)]

use language::Parser;

#[test]
fn in_test_item_follows_enclosing_attributes() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/test_items.rs")).unwrap();
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let tree = parser.parse(&text, None).unwrap();

    let in_test = |name: &str| {
        let start = text.find(name).unwrap();
        language::in_test_item(tree.root_node(), start..start + name.len(), |range| text[range].to_owned())
    };

    assert!(!in_test("helper_prod"));
    assert!(!in_test("helper_not_test"));
    assert!(in_test("helper_support"));
    // Nested in a `#[cfg(test)]` module, with no attribute of its own.
    assert!(in_test("helper_fixture"));
    // Inside a macro call, under attributes separated by a comment.
    assert!(in_test("helper_macro"));
    // The module name itself belongs to the test module.
    assert!(in_test("mod tests"));
}
