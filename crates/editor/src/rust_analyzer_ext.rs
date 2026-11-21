use std::fs;

use anyhow::Context as _;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::TaskExt as _;
use gpui::Window;
use language::Capability;
use language::Language;
use multi_buffer::MultiBuffer;
use project::lsp_store::lsp_ext_command;
use project::lsp_store::lsp_ext_command::ExpandMacro;
use project::lsp_store::rust_analyzer_ext::RUST_ANALYZER_NAME;
use text::ToPointUtf16 as _;
use url::Url;

use crate::Editor;
use crate::ExpandMacroRecursively;
use crate::GoToParentModule;
use crate::GotoDefinitionKind;
use crate::OpenDocs;
use crate::element::register_action;
use crate::hover_links::HoverLink;
use crate::lsp_ext::find_specific_language_server_in_selection;

fn is_rust_language(language: &Language) -> bool {
    language.name() == "Rust"
}

pub(crate) fn apply_related_actions(editor: &Entity<Editor>, window: &mut Window, cx: &App) {
    if editor
        .read(cx)
        .buffer()
        .read(cx)
        .all_buffers()
        .into_iter()
        .filter_map(|buffer| buffer.read(cx).language())
        .any(|language| is_rust_language(language))
    {
        register_action(editor, window, go_to_parent_module);
        register_action(editor, window, expand_macro_recursively);
        register_action(editor, window, open_docs);
    }
}

pub(crate) fn go_to_parent_module(
    editor: &mut Editor,
    _: &GoToParentModule,
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
                        lsp_ext_command::GoToParentModule { position, server_id: server_to_query },
                        cx,
                    )
                })
                .await
                .context("go to parent module")?
        };

        editor
            .update_in(cx, |editor, window, cx| {
                editor.navigate_to_hover_links(
                    Some(GotoDefinitionKind::Declaration),
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
            log::info!("Empty macro expansion for position {trigger_anchor:?}");
            return Ok(());
        }

        let buffer = project.update(cx, |project, cx| project.create_buffer(Some(rust_language), false, cx)).await?;
        workspace.update_in(cx, |workspace, window, cx| {
            buffer.update(cx, |buffer, cx| {
                buffer.set_text(macro_expansion.expansion, cx);
                buffer.set_capability(Capability::ReadOnly, cx);
            });
            let multibuffer = cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title(macro_expansion.name));
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
    })
    .detach_and_log_err(cx);
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
