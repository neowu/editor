//! Syntax tree and highlight dumps for `edctl`, in place of in-app inspector views.

use std::ops::Range;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use control::Highlight;
use control::HighlightKind;
use control::Position;
use editor::Anchor;
use editor::Editor;
use editor::MultiBufferSnapshot;
use editor::ToPoint as _;
use gpui::App;
use gpui::Entity;
use gpui::HighlightStyle;
use gpui::Hsla;
use gpui::Rgba;
use gpui::Window;
use language::BufferSnapshot;
use language::Node;
use language::Point;
use theme::ActiveTheme as _;
use workspace::Workspace;

/// Zero-based inclusive rows; everything when unset.
type Rows = Option<(u32, u32)>;

pub(crate) fn syntax_tree(window: &Window, rows: Rows, cx: &App) -> Result<Vec<control::SyntaxLayer>> {
    let (_, buffer) = cursor_buffer(&active_editor(window, cx)?, cx)?;
    Ok(buffer
        .syntax_layers()
        .map(|layer| control::SyntaxLayer {
            language: layer.language.name().to_string(),
            nodes: syntax_nodes(layer.node(), rows),
        })
        // Injection layers outside the rows.
        .filter(|layer| !layer.nodes.is_empty())
        .collect())
}

pub(crate) fn highlights(window: &Window, rows: Rows, cx: &mut App) -> Result<Vec<Highlight>> {
    let editor = active_editor(window, cx)?;
    let (snapshot, buffer) = cursor_buffer(&editor, cx)?;
    let buffer_id = buffer.buffer_id();
    let syntax_theme = cx.theme().syntax();
    let mut highlights = Vec::new();
    let mut push = |range: Range<Point>, kind, name: String, theme_key: Option<String>, style: HighlightStyle| {
        if touches_rows(rows, range.start.row, range.end.row) {
            highlights.push(Highlight {
                start: Position { row: range.start.row, column: range.start.column },
                end: Position { row: range.end.row, column: range.end.column },
                kind,
                name,
                theme_key,
                style: format_style(style),
            });
        }
    };
    // Multibuffer anchors to points in the cursor's buffer; ranges in other excerpts' buffers are dropped.
    let buffer_range = |range: &Range<Anchor>| {
        let (start_buffer, start) = snapshot.point_to_buffer_point(range.start.to_point(&snapshot))?;
        let (_, end) = snapshot.point_to_buffer_point(range.end.to_point(&snapshot))?;
        (start_buffer.buffer_id() == buffer_id).then_some(start..end)
    };

    let editor = editor.read(cx);
    let project = editor.project().cloned();
    let display_map = editor.display_map.read(cx);
    let text_highlights: Vec<_> =
        display_map.all_text_highlights().map(|(key, highlights)| (*key, Arc::clone(highlights))).collect();
    let semantic_tokens = display_map
        .all_semantic_token_highlights()
        .find(|(id, _)| **id == buffer_id)
        .map(|(_, (tokens, interner))| (Arc::clone(tokens), Arc::clone(interner)));

    for (key, text_highlights) in text_highlights {
        for range in text_highlights.1.iter().filter_map(&buffer_range) {
            push(range, HighlightKind::Text, format!("{key:?}"), None, text_highlights.0);
        }
    }

    if let Some(((tokens, interner), project)) = semantic_tokens.zip(project) {
        let language_name = buffer.language().map(|language| language.name());
        project.read(cx).lsp_store().update(cx, |lsp_store, cx| {
            for token in tokens.iter() {
                let Some(range) = buffer_range(&token.range) else {
                    continue;
                };
                let Some(stylizer) =
                    lsp_store.get_or_create_token_stylizer(token.server_id, language_name.as_ref(), cx)
                else {
                    continue;
                };
                // The last matching rule with a style the theme knows wins, as when the token is styled.
                let theme_key = stylizer.rules_for_token(token.token_type).and_then(|rules| {
                    rules
                        .iter()
                        .filter(|rule| {
                            rule.token_modifiers
                                .iter()
                                .all(|modifier| stylizer.has_modifier(token.token_modifiers, modifier))
                        })
                        .fold(None, |theme_key, rule| {
                            rule.style
                                .iter()
                                .find(|style_name| syntax_theme.style_for_name(style_name).is_some())
                                .map(ToString::to_string)
                                .or(theme_key)
                        })
                });
                let name = semantic_token_name(
                    stylizer.token_type_name(token.token_type).map(AsRef::as_ref),
                    stylizer.token_modifiers(token.token_modifiers).as_deref(),
                );
                push(range, HighlightKind::Semantic, name, theme_key, interner[token.style]);
            }
        });
    }

    let offsets = match rows {
        Some((first, last)) => {
            let last = last.min(buffer.max_point().row);
            let first = first.min(last);
            buffer.point_to_offset(Point::new(first, 0))
                ..buffer.point_to_offset(Point::new(last, buffer.line_len(last)))
        }
        None => 0..buffer.len(),
    };
    let captures = buffer.captures(offsets, |grammar| grammar.highlights_config.as_ref().map(|config| &config.query));
    let grammars = captures.grammars().to_vec();
    let highlight_maps: Vec<_> = grammars.iter().map(|grammar| grammar.highlight_map()).collect();
    for capture in captures {
        let Some(highlight_id) = highlight_maps[capture.grammar_index].get(language::CaptureId(capture.index)) else {
            continue;
        };
        let Some(style) = syntax_theme.get(highlight_id).copied() else {
            continue;
        };
        let name = grammars[capture.grammar_index]
            .highlights_config
            .as_ref()
            .and_then(|config| config.query.capture_names().get(capture.index as usize))
            .map_or_else(|| "unknown".to_owned(), ToString::to_string);
        let range = buffer.offset_to_point(capture.node.start_byte())..buffer.offset_to_point(capture.node.end_byte());
        let theme_key = syntax_theme.get_capture_name(highlight_id).map(str::to_owned);
        push(range, HighlightKind::Syntax, name, theme_key, style);
    }

    highlights.sort();
    highlights.dedup();
    Ok(highlights)
}

fn active_editor(window: &Window, cx: &App) -> Result<Entity<Editor>> {
    let workspace = window.root::<Workspace>().flatten().context("window has no workspace")?;
    workspace.read(cx).active_item(cx).and_then(|item| item.act_as::<Editor>(cx)).context("no active editor")
}

/// The buffer holding the newest cursor, since a multibuffer shows several.
fn cursor_buffer(editor: &Entity<Editor>, cx: &App) -> Result<(MultiBufferSnapshot, BufferSnapshot)> {
    let editor = editor.read(cx);
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let cursor = editor.selections.newest_anchor().head().to_point(&snapshot);
    let (buffer, _) = snapshot.point_to_buffer_point(cursor).context("cursor is not in a buffer")?;
    let buffer = buffer.clone();
    Ok((snapshot, buffer))
}

const fn touches_rows(rows: Rows, start_row: u32, end_row: u32) -> bool {
    match rows {
        Some((first, last)) => start_row <= last && end_row >= first,
        None => true,
    }
}

/// The nodes under `root` touching `rows`, parents first. Subtrees outside the rows are skipped.
fn syntax_nodes(root: Node, rows: Rows) -> Vec<control::SyntaxNode> {
    let mut nodes = Vec::new();
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let start = node.start_position();
        let end = node.end_position();
        let included = touches_rows(rows, start.row as u32, end.row as u32);
        if included {
            nodes.push(control::SyntaxNode {
                depth: cursor.depth(),
                field: cursor.field_name().map(str::to_owned),
                kind: node.kind().to_owned(),
                named: node.is_named(),
                start: Position { row: start.row as u32, column: start.column as u32 },
                end: Position { row: end.row as u32, column: end.column as u32 },
            });
        }
        if included && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return nodes;
            }
        }
    }
}

fn semantic_token_name(token_type: Option<&str>, modifiers: Option<&str>) -> String {
    match (token_type, modifiers) {
        (Some(token_type), Some(modifiers)) => format!("{token_type} [{modifiers}]"),
        (Some(token_type), None) => token_type.to_owned(),
        (None, Some(modifiers)) => format!("[{modifiers}]"),
        (None, None) => String::new(),
    }
}

fn format_style(style: HighlightStyle) -> String {
    let mut parts = Vec::new();
    if let Some(color) = style.color {
        parts.push(format_color(color));
    }
    if let Some(color) = style.background_color {
        parts.push(format!("bg {}", format_color(color)));
    }
    if style.font_weight.is_some() {
        parts.push("bold".to_owned());
    }
    if style.font_style.is_some() {
        parts.push("italic".to_owned());
    }
    if style.strikethrough.is_some() {
        parts.push("strike".to_owned());
    }
    if style.underline.is_some() {
        parts.push("underline".to_owned());
    }
    if parts.is_empty() { "none".to_owned() } else { parts.join(" ") }
}

fn format_color(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value * 255.).round() as u8;
    let (r, g, b, a) = (channel(rgba.r), channel(rgba.g), channel(rgba.b), channel(rgba.a));
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

#[cfg(test)]
mod tests {
    use gpui::FontStyle;
    use gpui::FontWeight;
    use gpui::HighlightStyle;
    use gpui::rgb;
    use gpui::rgba;
    use language::Parser;

    use super::format_color;
    use super::format_style;
    use super::semantic_token_name;
    use super::syntax_nodes;
    use super::touches_rows;

    #[test]
    fn touches_rows_is_inclusive() {
        assert!(touches_rows(None, 5, 9));
        assert!(touches_rows(Some((2, 3)), 0, 2));
        assert!(touches_rows(Some((2, 3)), 3, 8));
        assert!(!touches_rows(Some((2, 3)), 0, 1));
        assert!(!touches_rows(Some((2, 3)), 4, 4));
    }

    #[test]
    fn syntax_nodes_walks_parents_first_and_prunes_rows() {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        let tree = parser.parse("fn a() {}\nfn b() {}\n", None).unwrap();

        let nodes = syntax_nodes(tree.root_node(), None);
        let summary: Vec<_> = nodes
            .iter()
            .take(4)
            .map(|node| (node.depth, node.field.as_deref(), node.kind.as_str(), node.named))
            .collect();
        assert_eq!(
            summary,
            [
                (0, None, "source_file", true),
                (1, None, "function_item", true),
                (2, None, "fn", false),
                (2, Some("name"), "identifier", true),
            ]
        );
        assert_eq!(nodes.iter().filter(|node| node.kind == "function_item").count(), 2);

        let second_row = syntax_nodes(tree.root_node(), Some((1, 1)));
        let functions: Vec<_> = second_row.iter().filter(|node| node.kind == "function_item").collect();
        assert_eq!(functions.len(), 1);
        assert_eq!((functions[0].start.row, functions[0].start.column, functions[0].end.column), (1, 0, 9));
        assert!(second_row.iter().all(|node| node.end.row >= 1));
    }

    #[test]
    fn semantic_token_name_joins_type_and_modifiers() {
        assert_eq!(semantic_token_name(Some("function"), Some("async, declaration")), "function [async, declaration]");
        assert_eq!(semantic_token_name(Some("function"), None), "function");
        assert_eq!(semantic_token_name(None, Some("async")), "[async]");
        assert_eq!(semantic_token_name(None, None), "");
    }

    #[test]
    fn format_style_lists_colours_and_font_styles() {
        assert_eq!(format_style(HighlightStyle::default()), "none");
        let style = HighlightStyle {
            color: Some(rgb(0x88c0d0).into()),
            background_color: Some(rgba(0x2e344080).into()),
            font_weight: Some(FontWeight::BOLD),
            font_style: Some(FontStyle::Italic),
            ..HighlightStyle::default()
        };
        assert_eq!(format_style(style), "#88c0d0 bg #2e344080 bold italic");
    }

    #[test]
    fn format_color_drops_opaque_alpha() {
        assert_eq!(format_color(rgb(0xff0000).into()), "#ff0000");
        assert_eq!(format_color(rgba(0x00ff0080).into()), "#00ff0080");
    }
}
