use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::sync::Arc;

use anyhow::Context as _;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncWindowContext;
use gpui::Context;
use gpui::Entity;
use gpui::TaskExt as _;
use gpui::Window;
use itertools::Itertools as _;
use language::Capability;
use language::Language;
use language::OutlineItem;
use lsp_types::request::Request;
use multi_buffer::MultiBuffer;
use project::Project;
use project::lsp_store::lsp_ext_command;
use project::lsp_store::lsp_ext_command::ExpandMacro;
use project::lsp_store::lsp_ext_command::MemoryLayoutNode;
use project::lsp_store::lsp_ext_command::PositionRequest;
use project::lsp_store::rust_analyzer_ext::RUST_ANALYZER_NAME;
use text::ToPointUtf16 as _;
use url::Url;
use workspace::Toast;
use workspace::Workspace;
use workspace::notifications::NotificationId;

use crate::Editor;
use crate::ExpandMacroRecursively;
use crate::GoToRelatedTests;
use crate::OpenDocs;
use crate::actions::ViewHir;
use crate::actions::ViewMemoryLayout;
use crate::actions::ViewMir;
use crate::hover_links::HoverLink;
use crate::lsp_ext::find_specific_language_server_in_selection;

pub(crate) fn is_rust_language(language: &Language) -> bool {
    language.name() == "Rust"
}

pub(crate) fn has_rust_buffer(editor: &Editor, cx: &App) -> bool {
    editor
        .buffer()
        .read(cx)
        .all_buffers_iter()
        .any(|buffer| buffer.read(cx).language().is_some_and(|language| is_rust_language(language)))
}

pub(crate) fn go_to_related_tests(
    editor: &mut Editor,
    _: &GoToRelatedTests,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if editor.selections.count() == 0 {
        return;
    }
    let Some(project) = &editor.project else {
        return;
    };

    let Some((trigger_anchor, _, server_to_query, buffer)) =
        find_specific_language_server_in_selection(editor, cx, is_rust_language, RUST_ANALYZER_NAME)
    else {
        return;
    };

    let nav_entry = editor.navigation_entry(editor.selections.newest_anchor().head(), cx);

    let project = project.clone();
    cx.spawn_in(window, async move |editor, cx| {
        let location_links = {
            let buffer_snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
            let position = trigger_anchor.to_point_utf16(&buffer_snapshot);
            project
                .update(cx, |project, cx| {
                    project.request_lsp(
                        buffer,
                        project::LanguageServerToQuery::Other(server_to_query),
                        lsp_ext_command::RelatedTests { position, server_id: server_to_query },
                        cx,
                    )
                })
                .await
                .context("related tests")?
        };

        if location_links.is_empty() {
            if let Some(workspace) = editor.read_with(cx, |editor, _| editor.workspace())? {
                show_nothing_found(&workspace, "related tests", cx);
            }
            return Ok(());
        }

        editor
            .update_in(cx, |editor, window, cx| {
                editor.navigate_to_hover_links(
                    None,
                    location_links.into_iter().map(HoverLink::Text).collect(),
                    nav_entry,
                    false,
                    window,
                    cx,
                )
            })?
            .await?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

pub(crate) fn expand_macro_recursively(
    editor: &mut Editor,
    _: &ExpandMacroRecursively,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(project) = &editor.project else {
        return;
    };
    let Some(workspace) = editor.workspace() else {
        return;
    };

    let Some((trigger_anchor, rust_language, server_to_query, buffer)) =
        find_specific_language_server_in_selection(editor, cx, is_rust_language, RUST_ANALYZER_NAME)
    else {
        return;
    };
    let project = project.clone();
    cx.spawn_in(window, async move |_editor, cx| {
        let macro_expansion = {
            let buffer_snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
            let position = trigger_anchor.to_point_utf16(&buffer_snapshot);
            project
                .update(cx, |project, cx| {
                    project.request_lsp(
                        buffer,
                        project::LanguageServerToQuery::Other(server_to_query),
                        ExpandMacro { position, server_id: server_to_query },
                        cx,
                    )
                })
                .await
                .context("expand macro")?
        };

        if macro_expansion.is_empty() {
            show_nothing_found(&workspace, "macro expansion", cx);
            return Ok(());
        }

        open_read_only(&project, &workspace, macro_expansion.name, macro_expansion.expansion, Some(rust_language), cx)
            .await
    })
    .detach_and_log_err(cx);
}

pub(crate) fn view_hir(editor: &mut Editor, _: &ViewHir, window: &mut Window, cx: &mut Context<Editor>) {
    view_text::<lsp_ext_command::ViewHir>(editor, true, |hir, symbol| (view_title("HIR", symbol), hir), window, cx);
}

pub(crate) fn view_mir(editor: &mut Editor, _: &ViewMir, window: &mut Window, cx: &mut Context<Editor>) {
    view_text::<lsp_ext_command::ViewMir>(editor, true, |mir, symbol| (view_title("MIR", symbol), mir), window, cx);
}

pub(crate) fn view_memory_layout(
    editor: &mut Editor,
    _: &ViewMemoryLayout,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    view_text::<lsp_ext_command::ViewRecursiveMemoryLayout>(
        editor,
        false,
        |layout, _| {
            let nodes = layout.map_or_default(|layout| layout.nodes);
            let typename = nodes.first().map_or("", |root| root.typename.as_str());
            (view_title("Memory Layout", typename), memory_layout_text(&nodes))
        },
        window,
        cx,
    );
}

/// Asks rust-analyzer about the cursor position and shows the answer in a read-only buffer.
fn view_text<R>(
    editor: &Editor,
    highlight_as_rust: bool,
    // Gets the response and the path of the symbols around the cursor, returns the tab title and the text.
    to_view: fn(R::Result, &str) -> (String, String),
    window: &Window,
    cx: &mut Context<Editor>,
) where
    R: 'static + Send + Request<Params = lsp_types::TextDocumentPositionParams>,
    R::Result: 'static + Default + Send + fmt::Debug,
{
    let Some(project) = &editor.project else {
        return;
    };
    let Some(workspace) = editor.workspace() else {
        return;
    };

    let Some((trigger_anchor, rust_language, server_to_query, buffer)) =
        find_specific_language_server_in_selection(editor, cx, is_rust_language, RUST_ANALYZER_NAME)
    else {
        return;
    };
    let project = project.clone();
    cx.spawn_in(window, async move |_editor, cx| {
        let (position, symbol) = buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            (trigger_anchor.to_point_utf16(&snapshot), symbol_path(&snapshot.symbols_containing(trigger_anchor, None)))
        });
        let response = project
            .update(cx, |project, cx| {
                project.request_lsp(
                    buffer,
                    project::LanguageServerToQuery::Other(server_to_query),
                    PositionRequest::<R>::new(position, server_to_query),
                    cx,
                )
            })
            .await
            .context(R::METHOD)?;

        let (title, text) = to_view(response, &symbol);
        if text.is_empty() {
            show_nothing_found(&workspace, &title, cx);
            return Ok(());
        }

        open_read_only(&project, &workspace, title, text, highlight_as_rust.then_some(rust_language), cx).await
    })
    .detach_and_log_err(cx);
}

/// Tells the user rust-analyzer had no answer, so the action doesn't look broken.
fn show_nothing_found(workspace: &Entity<Workspace>, what: &str, cx: &mut AsyncWindowContext) {
    struct NothingFound;
    workspace.update(cx, |workspace, cx| {
        let message = format!("No {what} from rust-analyzer at this position");
        workspace.show_toast(Toast::new(NotificationId::unique::<NothingFound>(), message).autohide(), cx);
    });
}

fn view_title(kind: &str, subject: &str) -> String {
    if subject.is_empty() { kind.to_owned() } else { format!("{kind} for {subject}") }
}

/// Names of nested outline symbols, outermost first, e.g. `Foo::new`.
fn symbol_path(symbols: &[OutlineItem<text::Anchor>]) -> String {
    // The last name of `impl Trait for Foo` is the type.
    symbols.iter().filter_map(|symbol| symbol.name_ranges.last().map(|range| &symbol.text[range.clone()])).join("::")
}

async fn open_read_only(
    project: &Entity<Project>,
    workspace: &Entity<Workspace>,
    title: String,
    text: String,
    language: Option<Arc<Language>>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    let buffer = project.update(cx, |project, cx| project.create_buffer(language, false, cx)).await?;
    workspace.update_in(cx, |workspace, window, cx| {
        buffer.update(cx, |buffer, cx| {
            buffer.set_text(text, cx);
            buffer.set_capability(Capability::ReadOnly, cx);
        });
        let multibuffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title(title));
        workspace.add_item_to_active_pane(
            Box::new(cx.new(|cx| {
                let mut editor = Editor::for_multibuffer(multibuffer, None, window, cx);
                editor.set_read_only(true);
                editor
            })),
            None,
            true,
            window,
            cx,
        );
    })
}

/// One line per node, children indented under their parent, offsets relative to the parent.
fn memory_layout_text(nodes: &[MemoryLayoutNode]) -> String {
    fn write_node(nodes: &[MemoryLayoutNode], index: usize, depth: usize, text: &mut String) {
        let Some(node) = nodes.get(index) else {
            return;
        };
        let indent = depth * 4;
        if depth == 0 {
            writeln!(text, "{}  (size {}, align {})", node.typename, node.size, node.alignment)
        } else {
            writeln!(
                text,
                "{:indent$}+{}  {}: {}  (size {}, align {})",
                "", node.offset, node.item_name, node.typename, node.size, node.alignment
            )
        }
        .expect("writing to a string never fails");
        let Ok(start) = usize::try_from(node.children_start) else {
            return;
        };
        // Children always follow their parent, which also guards against a cyclic response.
        for child in start.max(index + 1)..start.saturating_add(node.children_len as usize) {
            write_node(nodes, child, depth + 1, text);
        }
    }

    let mut text = String::new();
    write_node(nodes, 0, 0, &mut text);
    text
}

pub(crate) fn open_docs(editor: &mut Editor, _: &OpenDocs, window: &mut Window, cx: &mut Context<Editor>) {
    if editor.selections.count() == 0 {
        return;
    }
    let Some(project) = &editor.project else {
        return;
    };
    let Some(workspace) = editor.workspace() else {
        return;
    };

    let Some((trigger_anchor, _, server_to_query, buffer)) =
        find_specific_language_server_in_selection(editor, cx, is_rust_language, RUST_ANALYZER_NAME)
    else {
        return;
    };

    let project = project.clone();
    cx.spawn_in(window, async move |_editor, cx| {
        let docs_urls = {
            let buffer_snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
            let position = trigger_anchor.to_point_utf16(&buffer_snapshot);
            project
                .update(cx, |project, cx| {
                    project.request_lsp(
                        buffer,
                        project::LanguageServerToQuery::Other(server_to_query),
                        lsp_ext_command::OpenDocs { position, server_id: server_to_query },
                        cx,
                    )
                })
                .await
                .context("open docs")?
        };

        if docs_urls.is_empty() {
            log::debug!("Empty docs urls for position {trigger_anchor:?}");
            return Ok(());
        }

        // Check if the local document exists, otherwise fallback to the online document.
        let url = if let Some(local_url) = docs_urls.local
            && let Ok(url) = Url::parse(&local_url)
            && url.scheme() == "file"
            && let Ok(path) = url.to_file_path()
            && cx.background_spawn(async move { fs::metadata(path).is_ok() }).await
        {
            Some(local_url)
        } else {
            docs_urls.web
        };

        workspace.update(cx, |_workspace, cx| {
            if let Some(url) = url {
                cx.open_url(&url);
            }
        });
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

#[cfg(test)]
mod tests {
    use language::LanguageConfig;

    use super::*;

    fn language(name: &'static str) -> Arc<Language> {
        Arc::new(Language::new(LanguageConfig { name: name.into(), ..Default::default() }, None))
    }

    #[test]
    fn rust_language_is_matched_by_name() {
        assert!(is_rust_language(&language("Rust")));
        assert!(!is_rust_language(&language("Markdown")));
    }

    fn node(name: &str, typename: &str, size: u64, offset: u64, children: Option<(i64, u64)>) -> MemoryLayoutNode {
        let (children_start, children_len) = children.unwrap_or((-1, 0));
        MemoryLayoutNode {
            item_name: name.to_owned(),
            typename: typename.to_owned(),
            size,
            alignment: 8,
            offset,
            children_start,
            children_len,
        }
    }

    #[test]
    fn symbol_path_joins_the_last_name_of_each_symbol() {
        let symbol = |text: &'static str, name_ranges: &[std::ops::Range<usize>]| {
            let anchor = text::Anchor::min_for_buffer(text::BufferId::new(1).unwrap());
            OutlineItem {
                depth: 0,
                range: anchor..anchor,
                selection_range: anchor..anchor,
                source_range_for_text: anchor..anchor,
                text: text.into(),
                highlight_ranges: Vec::new(),
                name_ranges: name_ranges.to_vec(),
                body_range: None,
                annotation_range: None,
            }
        };
        let symbols =
            [symbol("impl Display for Foo", &[5..12, 17..20]), symbol("fn fmt", std::slice::from_ref(&(3..6)))];
        assert_eq!(symbol_path(&symbols), "Foo::fmt");
        assert_eq!(symbol_path(&[]), "");
    }

    #[test]
    fn view_title_names_its_subject() {
        assert_eq!(view_title("HIR", "Foo::new"), "HIR for Foo::new");
        assert_eq!(view_title("MIR", ""), "MIR");
    }

    #[test]
    fn memory_layout_is_an_indented_tree() {
        let nodes = [
            node("Foo", "Foo", 24, 0, Some((1, 2))),
            node("a", "u64", 8, 0, None),
            node("b", "Bar", 16, 8, Some((3, 1))),
            node("c", "usize", 8, 8, None),
        ];
        assert_eq!(
            memory_layout_text(&nodes),
            "Foo  (size 24, align 8)
    +0  a: u64  (size 8, align 8)
    +8  b: Bar  (size 16, align 8)
        +8  c: usize  (size 8, align 8)
"
        );
        assert_eq!(memory_layout_text(&[]), "");
        // A node listing itself as a child is not followed.
        assert_eq!(memory_layout_text(&[node("Foo", "Foo", 8, 0, Some((0, 1)))]), "Foo  (size 8, align 8)\n");
    }
}
