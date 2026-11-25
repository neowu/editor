//! A `TODO` that opens a comment gets its own highlight. This works on
//! highlight runs, so it applies to every language, with or without a
//! tree-sitter grammar.

use std::ops::Range;
use std::sync::LazyLock;

use language_core::CaptureId;
use language_core::HighlightId;
use language_core::HighlightMap;
use text::BufferSnapshot as TextBufferSnapshot;

use crate::build_highlight_map;

pub(crate) const CAPTURE_NAME: &str = "comment.todo";
const KEYWORD: &[u8] = b"TODO";
/// How much of a comment is read to find the keyword: the comment marker,
/// spaces and the keyword itself must fit.
const PREFIX_LEN: usize = 16;

/// The theme's comment capture, followed by the keyword capture. Doc comments
/// (`comment.doc`) are left alone.
static HIGHLIGHT_MAP: LazyLock<HighlightMap> = LazyLock::new(|| build_highlight_map(&["comment", CAPTURE_NAME]));

fn keyword_id() -> Option<HighlightId> {
    HIGHLIGHT_MAP.get(CaptureId(1))
}

fn is_comment(id: HighlightId) -> bool {
    Some(id) == HIGHLIGHT_MAP.get(CaptureId(0))
}

/// Gives the keyword opening a comment run its own highlight. Only the start
/// of each comment run is read, and `runs` is returned as is without a match.
pub(crate) fn highlight(
    text: &TextBufferSnapshot,
    runs: Vec<(Range<usize>, HighlightId)>,
) -> Vec<(Range<usize>, HighlightId)> {
    let Some(keyword_id) = keyword_id() else {
        return runs;
    };
    let mut keywords = Vec::new();
    let mut prefix = [0; PREFIX_LEN];
    for (ix, (range, highlight_id)) in runs.iter().enumerate() {
        if !is_comment(*highlight_id) {
            continue;
        }
        let mut len = 0;
        for bytes in text.bytes_in_range(range.start..range.end.min(range.start + PREFIX_LEN)) {
            prefix[len..len + bytes.len()].copy_from_slice(bytes);
            len += bytes.len();
        }
        if let Some(keyword) = leading_keyword(&prefix[..len]) {
            keywords.push((ix, range.start + keyword.start..range.start + keyword.end));
        }
    }
    if keywords.is_empty() {
        return runs;
    }
    let mut pieces = Vec::with_capacity(runs.len() + 2 * keywords.len());
    let mut keywords = keywords.into_iter().peekable();
    for (ix, (range, highlight_id)) in runs.into_iter().enumerate() {
        match keywords.next_if(|(keyword_ix, _)| *keyword_ix == ix) {
            Some((_, keyword)) => split(range, keyword, |range, is_keyword| {
                pieces.push((range, if is_keyword { keyword_id } else { highlight_id }));
            }),
            None => pieces.push((range, highlight_id)),
        }
    }
    pieces
}

/// The range of the keyword in a comment that starts with `comment`, if only
/// the comment marker and spaces come before it and it is a whole word.
pub(crate) fn leading_keyword(comment: &[u8]) -> Option<Range<usize>> {
    let start = comment
        .iter()
        .position(|byte| !(matches!(byte, b' ' | b'\t') || byte.is_ascii_punctuation() && *byte != b'_'))?;
    let rest = comment[start..].strip_prefix(KEYWORD)?;
    if rest.first().is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_') {
        return None;
    }
    Some(start..start + KEYWORD.len())
}

/// Splits the comment run `range` around the `keyword` in it. `push` gets
/// each non-empty piece and whether it is the keyword.
pub(crate) fn split(range: Range<usize>, keyword: Range<usize>, mut push: impl FnMut(Range<usize>, bool)) {
    if range.start < keyword.start {
        push(range.start..keyword.start, false);
    }
    let end = keyword.end;
    push(keyword, true);
    if end < range.end {
        push(end..range.end, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_keyword_needs_only_a_marker_before_it() {
        let keyword = |comment: &str| leading_keyword(comment.as_bytes()).map(|range| range.start);
        assert_eq!(keyword("// TODO: x"), Some(3));
        assert_eq!(keyword("//TODO(neo)"), Some(2));
        assert_eq!(keyword("/// TODO"), Some(4));
        assert_eq!(keyword("# TODO fix"), Some(2));
        assert_eq!(keyword("<!-- TODO -->"), Some(5));
        assert_eq!(keyword(" *\tTODO"), Some(3));
        assert_eq!(keyword("TODO"), Some(0));
        assert_eq!(keyword("// see TODO"), None);
        assert_eq!(keyword("// TODOs"), None);
        assert_eq!(keyword("// TODO_LIST"), None);
        assert_eq!(keyword("// MY_TODO"), None);
        assert_eq!(keyword("// _TODO"), None);
        assert_eq!(keyword("// todo"), None);
        assert_eq!(keyword("// é TODO"), None);
        assert_eq!(keyword("// "), None);
        assert_eq!(keyword(""), None);
    }

    #[test]
    fn split_pushes_the_non_empty_pieces() {
        let pieces = |range: Range<usize>, keyword: Range<usize>| {
            let mut pieces = Vec::new();
            split(range, keyword, |range, is_keyword| pieces.push((range, is_keyword)));
            pieces
        };
        assert_eq!(pieces(10..20, 13..17), [(10..13, false), (13..17, true), (17..20, false)]);
        assert_eq!(pieces(5..9, 5..9), [(5..9, true)]);
        assert_eq!(pieces(5..12, 8..12), [(5..8, false), (8..12, true)]);
    }

    #[test]
    fn highlight_splits_comment_runs_that_open_with_the_keyword() {
        let syntax = theme::theme().syntax();
        let id = |name| HighlightId::new(syntax.highlight_id(name).unwrap());
        let source = "let todo = \"TODO\"; // TODO: x\n/// TODO\n// later TODO\n";
        let buffer = text::Buffer::new(text::BufferId::new(1).unwrap(), source);
        let string = source.find('"').unwrap()..source.find(';').unwrap();
        let comment = source.find("//").unwrap()..source.find('\n').unwrap();
        let doc = source.find("///").unwrap()..source.find("\n// later").unwrap();
        let later = source.find("// later").unwrap()..source.len() - 1;
        let runs = vec![
            (string.clone(), id("string")),
            (comment, id("comment")),
            (doc, id("comment.doc")),
            (later.clone(), id("comment")),
        ];
        let named = |runs: Vec<(Range<usize>, HighlightId)>| {
            runs.into_iter()
                .map(|(range, id)| (&source[range], syntax.get_capture_name(id).unwrap()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            named(highlight(buffer.snapshot(), runs)),
            [
                ("\"TODO\"", "string"),
                ("// ", "comment"),
                ("TODO", "comment.todo"),
                (": x", "comment"),
                ("/// TODO", "comment.doc"),
                ("// later TODO", "comment"),
            ]
        );

        let untouched = vec![(string, id("string")), (later, id("comment"))];
        assert_eq!(highlight(buffer.snapshot(), untouched.clone()), untouched);
    }

    #[test]
    fn highlight_reads_only_the_start_of_a_comment() {
        let comment = HighlightId::new(theme::theme().syntax().highlight_id("comment").unwrap());
        let fits = format!("//{}TODO", " ".repeat(PREFIX_LEN - 6));
        let buffer = text::Buffer::new(text::BufferId::new(1).unwrap(), fits.as_str());
        assert_eq!(highlight(buffer.snapshot(), vec![(0..fits.len(), comment)]).len(), 2);

        let too_far = format!("//{}TODO", " ".repeat(PREFIX_LEN));
        let buffer = text::Buffer::new(text::BufferId::new(1).unwrap(), too_far.as_str());
        let runs = vec![(0..too_far.len(), comment)];
        assert_eq!(highlight(buffer.snapshot(), runs.clone()), runs);
    }

    #[test]
    fn the_theme_styles_the_keyword_apart_from_comments() {
        let syntax = theme::theme().syntax();
        let keyword_id = keyword_id().unwrap();
        assert_eq!(syntax.get_capture_name(keyword_id), Some(CAPTURE_NAME));
        assert!(!is_comment(keyword_id));
        assert!(is_comment(HighlightId::new(syntax.highlight_id("comment").unwrap())));
        assert!(!is_comment(HighlightId::new(syntax.highlight_id("comment.doc").unwrap())));
        assert!(!is_comment(HighlightId::new(syntax.highlight_id("string").unwrap())));
    }
}
