//! `language::walk_syntax` over a real Rust grammar parse.

use language::Node;
use language::Parser;

/// (depth, field, kind, named, start, end) of each visited node.
type Visit = (u32, Option<String>, String, bool, (usize, usize), (usize, usize));

fn walk(root: Node, rows: Option<(u32, u32)>) -> Vec<Visit> {
    let mut visits = Vec::new();
    language::walk_syntax(root, rows, |cursor| {
        let node = cursor.node();
        let (start, end) = (node.start_position(), node.end_position());
        visits.push((
            cursor.depth(),
            cursor.field_name().map(str::to_owned),
            node.kind().to_owned(),
            node.is_named(),
            (start.row, start.column),
            (end.row, end.column),
        ));
    });
    visits
}

#[test]
fn walk_syntax_visits_parents_first_and_prunes_rows() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/functions.rs")).unwrap();
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    let tree = parser.parse(text, None).unwrap();

    let all = walk(tree.root_node(), None);
    let summary: Vec<_> = all.iter().take(4).map(|v| (v.0, v.1.as_deref(), v.2.as_str(), v.3)).collect();
    assert_eq!(
        summary,
        [
            (0, None, "source_file", true),
            (1, None, "function_item", true),
            (2, None, "fn", false),
            (2, Some("name"), "identifier", true),
        ]
    );
    assert_eq!(all.iter().filter(|v| v.2 == "function_item").count(), 2);

    let second_row = walk(tree.root_node(), Some((1, 1)));
    let functions: Vec<_> = second_row.iter().filter(|v| v.2 == "function_item").collect();
    assert_eq!(functions.len(), 1);
    assert_eq!((functions[0].4, functions[0].5), ((1, 0), (1, 9)));
    assert!(second_row.iter().all(|v| v.5.0 >= 1));
}
