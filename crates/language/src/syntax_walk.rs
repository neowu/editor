use tree_sitter::Node;
use tree_sitter::TreeCursor;

/// Whether `start_row..=end_row` overlaps `rows` (zero-based, inclusive); everything when unset.
pub const fn touches_rows(rows: Option<(u32, u32)>, start_row: u32, end_row: u32) -> bool {
    match rows {
        Some((first, last)) => start_row <= last && end_row >= first,
        None => true,
    }
}

/// Visits the nodes under `root` touching `rows`, parents first, with the cursor on each node.
/// Subtrees outside the rows are skipped.
pub fn walk_syntax(root: Node, rows: Option<(u32, u32)>, mut visit: impl FnMut(&TreeCursor)) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let included = touches_rows(rows, node.start_position().row as u32, node.end_position().row as u32);
        if included {
            visit(&cursor);
        }
        if included && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::touches_rows;

    #[test]
    fn touches_rows_is_inclusive() {
        assert!(touches_rows(None, 5, 9));
        assert!(touches_rows(Some((2, 3)), 0, 2));
        assert!(touches_rows(Some((2, 3)), 3, 8));
        assert!(!touches_rows(Some((2, 3)), 0, 1));
        assert!(!touches_rows(Some((2, 3)), 4, 4));
    }
}
