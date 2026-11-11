use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use futures::FutureExt;
use futures::StreamExt as _;
use futures::channel::mpsc;
use futures::future::Shared;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::Task;
use language::Buffer;
use rustc_hash::FxHashMap;
use task::Shell;
use util::shell_env;
use worktree::Worktree;

use crate::worktree_store::WorktreeStore;

pub struct ProjectEnvironment {
    #[allow(clippy::type_complexity)]
    local_environments: FxHashMap<(Shell, Arc<Path>), Shared<Task<Option<FxHashMap<String, String>>>>>,
    environment_error_messages: VecDeque<String>,
    environment_error_messages_tx: mpsc::UnboundedSender<String>,
    _tasks: Vec<Task<()>>,
}

pub enum ProjectEnvironmentEvent {
    ErrorsUpdated,
}

impl EventEmitter<ProjectEnvironmentEvent> for ProjectEnvironment {}

impl ProjectEnvironment {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded();
        let task = cx.spawn(async move |this, cx| {
            while let Some(message) = rx.next().await {
                this.update(cx, |this, cx| {
                    this.environment_error_messages.push_back(message);
                    cx.emit(ProjectEnvironmentEvent::ErrorsUpdated);
                })
                .ok();
            }
        });
        Self {
            local_environments: FxHashMap::default(),
            environment_error_messages: VecDeque::default(),
            environment_error_messages_tx: tx,
            _tasks: vec![task],
        }
    }

    pub fn buffer_environment(
        &mut self,
        buffer: &Entity<Buffer>,
        worktree_store: &Entity<WorktreeStore>,
        cx: &mut Context<Self>,
    ) -> Shared<Task<Option<FxHashMap<String, String>>>> {
        let Some(worktree) = buffer
            .read(cx)
            .file()
            .map(|f| f.worktree_id(cx))
            .and_then(|worktree_id| worktree_store.read(cx).worktree_for_id(worktree_id, cx))
        else {
            return Task::ready(None).shared();
        };
        self.worktree_environment(worktree, cx)
    }

    pub fn worktree_environment(
        &mut self,
        worktree: Entity<Worktree>,
        cx: &mut App,
    ) -> Shared<Task<Option<FxHashMap<String, String>>>> {
        let worktree = worktree.read(cx);
        let mut abs_path = worktree.abs_path();
        if worktree.is_single_file() {
            let Some(parent) = abs_path.parent() else {
                return Task::ready(None).shared();
            };
            abs_path = parent.into();
        }

        self.local_directory_environment(&Shell::System, abs_path, cx)
    }

    /// Returns the project environment by spawning a shell in `abs_path`, to get environment
    /// variables as if the user has `cd`'d there.
    pub fn local_directory_environment(
        &mut self,
        shell: &Shell,
        abs_path: Arc<Path>,
        cx: &mut App,
    ) -> Shared<Task<Option<FxHashMap<String, String>>>> {
        self.local_environments
            .entry((shell.clone(), abs_path.clone()))
            .or_insert_with(|| {
                let shell = shell.clone();
                let tx = self.environment_error_messages_tx.clone();
                cx.spawn(async move |cx| {
                    let shell_env = match cx
                        .background_spawn(load_directory_shell_environment(shell, abs_path.clone(), tx))
                        .await
                    {
                        Ok(shell_env) => Some(shell_env),
                        Err(e) => {
                            log::error!("Failed to load shell environment for directory {abs_path:?}: {e:#}");
                            None
                        }
                    };

                    if let Some(shell_env) = shell_env.as_ref() {
                        let path = shell_env.get("PATH").map(|path| path.as_str()).unwrap_or_default();
                        log::debug!(
                            "using project environment variables shell launched in {:?}. PATH={:?}",
                            abs_path,
                            path
                        );
                    }

                    shell_env
                })
                .shared()
            })
            .clone()
    }

    pub fn peek_environment_error(&self) -> Option<&String> {
        self.environment_error_messages.front()
    }

    pub fn pop_environment_error(&mut self) -> Option<String> {
        self.environment_error_messages.pop_front()
    }
}

async fn load_directory_shell_environment(
    shell: Shell,
    abs_path: Arc<Path>,
    tx: mpsc::UnboundedSender<String>,
) -> anyhow::Result<FxHashMap<String, String>> {
    let (shell, args) = shell.program_and_args();
    shell_env::capture(shell.clone(), args, abs_path).await.with_context(|| {
        tx.unbounded_send("Failed to load environment variables".into()).ok();
        format!("capturing shell environment with {shell:?}")
    })
}
