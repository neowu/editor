use gpui::App;
use gpui::Entity;
use gpui::Task;
use lsp::LanguageServerName;
use project::LanguageServerToQuery;
use project::LocationLink;
use project::Project;
use project::lsp_store::lsp_ext_command::GetLspRunnables;
use rustc_hash::FxHashMap;
use task::SpawnInTerminal;
use text::BufferId;
use util::ResultExt as _;

/// Runnables from the language servers in `task_sources`, for their buffers (or only `for_position`'s).
pub(crate) fn lsp_tasks(
    project: Entity<Project>,
    task_sources: &FxHashMap<LanguageServerName, Vec<BufferId>>,
    for_position: Option<text::Anchor>,
    cx: &App,
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
