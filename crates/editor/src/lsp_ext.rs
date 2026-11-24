use std::sync::Arc;

use gpui::App;
use gpui::Entity;
use gpui::Task;
use language::Buffer;
use language::Language;
use lsp::LanguageServerId;
use lsp::LanguageServerName;
use project::LanguageServerToQuery;
use project::LocationLink;
use project::Project;
use project::lsp_store::lsp_ext_command::GetLspRunnables;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use task::SpawnInTerminal;
use text::BufferId;
use util::ResultExt as _;

use crate::Editor;

pub(crate) fn find_specific_language_server_in_selection<F>(
    editor: &Editor,
    cx: &mut App,
    filter_language: F,
    language_server_name: LanguageServerName,
) -> Option<(text::Anchor, Arc<Language>, LanguageServerId, Entity<Buffer>)>
where
    F: Fn(&Language) -> bool,
{
    let project = editor.project.clone()?;
    let multi_buffer = editor.buffer();
    let mut seen_buffer_ids = FxHashSet::default();
    editor.selections.disjoint_anchors_arc().iter().find_map(|selection| {
        let multi_buffer = multi_buffer.read(cx);
        let multi_buffer_snapshot = multi_buffer.snapshot(cx);
        let (position, buffer) = multi_buffer_snapshot
            .anchor_to_buffer_anchor(selection.head())
            .and_then(|(anchor, _)| Some((anchor, multi_buffer.buffer(anchor.buffer_id)?)))?;
        if !seen_buffer_ids.insert(buffer.read(cx).buffer_id()) {
            return None;
        }

        let language = buffer.read(cx).language_at(position)?;
        if filter_language(&language) {
            let server_id = buffer.update(cx, |buffer, cx| {
                project.read(cx).language_server_id_for_name(buffer, &language_server_name, cx)
            })?;
            Some((position, language, server_id, buffer))
        } else {
            None
        }
    })
}

/// Runnables from the language servers in `task_sources`, for their buffers (or only `for_position`'s).
pub fn lsp_tasks(
    project: Entity<Project>,
    task_sources: &FxHashMap<LanguageServerName, Vec<BufferId>>,
    for_position: Option<text::Anchor>,
    cx: &mut App,
) -> Task<Vec<(LocationLink, SpawnInTerminal)>> {
    let lsp_task_sources = task_sources
        .iter()
        .filter_map(|(name, buffer_ids)| {
            let buffers = buffer_ids
                .iter()
                .filter(|&&buffer_id| match for_position {
                    Some(for_position) => for_position.buffer_id == buffer_id,
                    None => true,
                })
                .filter_map(|&buffer_id| project.read(cx).buffer_for_id(buffer_id, cx))
                .collect::<Vec<_>>();

            let server_id = buffers.iter().find_map(|buffer| {
                project.read_with(cx, |project, cx| project.language_server_id_for_name(buffer.read(cx), name, cx))
            });
            server_id.zip(Some(buffers))
        })
        .collect::<Vec<_>>();

    cx.spawn(async move |cx| {
        let mut lsp_tasks = Vec::new();
        for (server_id, buffers) in lsp_task_sources {
            for buffer in buffers {
                let runnables_task = project.update(cx, |project, cx| {
                    let buffer_id = buffer.read(cx).buffer_id();
                    project.request_lsp(
                        buffer,
                        LanguageServerToQuery::Other(server_id),
                        GetLspRunnables { buffer_id, position: for_position, server_id },
                        cx,
                    )
                });
                if let Some(new_runnables) = runnables_task.await.log_err() {
                    lsp_tasks.extend(new_runnables.runnables);
                }
            }
        }
        lsp_tasks
    })
}
