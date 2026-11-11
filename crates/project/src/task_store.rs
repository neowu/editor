use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use fs::Fs;
use gpui::App;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Task;
use language::ContextLocation;
use language::ContextProvider as _;
use language::Location;
use rustc_hash::FxHashMap;
use task::TaskContext;
use task::TaskVariables;

use crate::BasicContextProvider;
use crate::ProjectEnvironment;
use crate::git_store::GitStore;
use crate::worktree_store::WorktreeStore;

pub struct TaskStore {
    environment: Entity<ProjectEnvironment>,
    worktree_store: Entity<WorktreeStore>,
    git_store: Entity<GitStore>,
}

impl EventEmitter<crate::Event> for TaskStore {}

impl TaskStore {
    pub const fn new(
        worktree_store: Entity<WorktreeStore>,
        environment: Entity<ProjectEnvironment>,
        git_store: Entity<GitStore>,
    ) -> Self {
        Self { environment, git_store, worktree_store }
    }

    pub fn task_context_for_location(
        &self,
        captured_variables: TaskVariables,
        location: Location,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<TaskContext>>> {
        local_task_context_for_location(
            self.worktree_store.clone(),
            self.git_store.clone(),
            self.environment.clone(),
            captured_variables,
            location,
            cx,
        )
    }
}

fn local_task_context_for_location(
    worktree_store: Entity<WorktreeStore>,
    git_store: Entity<GitStore>,
    environment: Entity<ProjectEnvironment>,
    captured_variables: TaskVariables,
    location: Location,
    cx: &App,
) -> Task<anyhow::Result<Option<TaskContext>>> {
    let worktree_id = location.buffer.read(cx).file().map(|f| f.worktree_id(cx));
    let worktree_abs_path = worktree_id
        .and_then(|worktree_id| worktree_store.read(cx).worktree_for_id(worktree_id, cx))
        .and_then(|worktree| worktree.read(cx).root_dir());
    let fs = worktree_store.read(cx).fs();

    cx.spawn(async move |cx| {
        let project_env = environment
            .update(cx, |environment, cx| environment.buffer_environment(&location.buffer, &worktree_store, cx))
            .await;

        let mut task_variables = cx
            .update(|cx| {
                combine_task_variables(
                    captured_variables,
                    fs,
                    worktree_store.clone(),
                    location,
                    project_env.clone(),
                    BasicContextProvider::new(worktree_store, git_store),
                    cx,
                )
            })
            .await?;

        // Remove all custom entries starting with _, as they're not intended for use by the end user.
        task_variables.sweep();

        Ok(Some(TaskContext {
            project_env: project_env.unwrap_or_default(),
            cwd: worktree_abs_path.map(|p| p.to_path_buf()),
            task_variables,
        }))
    })
}

fn worktree_root(worktree_store: &Entity<WorktreeStore>, location: &Location, cx: &App) -> Option<PathBuf> {
    location
        .buffer
        .read(cx)
        .file()
        .map(|f| f.worktree_id(cx))
        .and_then(|worktree_id| worktree_store.read(cx).worktree_for_id(worktree_id, cx))
        .and_then(|worktree| {
            let worktree = worktree.read(cx);
            if !worktree.is_visible() {
                return None;
            }
            let root_entry = worktree.root_entry()?;
            if !root_entry.is_dir() {
                return None;
            }
            Some(worktree.absolutize(&root_entry.path))
        })
}

fn combine_task_variables(
    mut captured_variables: TaskVariables,
    fs: Arc<dyn Fs>,
    worktree_store: Entity<WorktreeStore>,
    location: Location,
    project_env: Option<FxHashMap<String, String>>,
    baseline: BasicContextProvider,
    cx: &App,
) -> Task<anyhow::Result<TaskVariables>> {
    let language_context_provider =
        location.buffer.read(cx).language().and_then(|language| language.context_provider());
    cx.spawn(async move |cx| {
        let baseline = cx
            .update(|cx| {
                let worktree_root = worktree_root(&worktree_store, &location, cx);
                baseline.build_context(
                    &captured_variables,
                    ContextLocation { fs: fs.clone(), worktree_root, file_location: &location },
                    project_env.clone(),
                    cx,
                )
            })
            .await
            .context("building basic default context")?;
        captured_variables.extend(baseline);
        if let Some(provider) = language_context_provider {
            captured_variables.extend(
                cx.update(|cx| {
                    let worktree_root = worktree_root(&worktree_store, &location, cx);
                    provider.build_context(
                        &captured_variables,
                        ContextLocation { fs, worktree_root, file_location: &location },
                        project_env,
                        cx,
                    )
                })
                .await?,
            );
        }
        Ok(captured_variables)
    })
}
