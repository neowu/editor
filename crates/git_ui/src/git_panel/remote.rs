use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use git::repository::FetchOptions;
use git::repository::PushOptions;
use git::repository::RebaseAction;
use git::repository::Remote;
use git::repository::RemoteCommandOutput;
use git::repository::Upstream;
use git::repository::UpstreamTracking;
use git_ui_core::notifications::open_output;
use gpui::PromptLevel;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::WeakEntity;
use itertools::Itertools as _;
use notifications::status_toast::StatusToast;
use ui::SharedString;
use ui::prelude::App;
use ui::prelude::Color;
use ui::prelude::Context;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::Window;
use util::ResultExt as _;
use util::paths;
use workspace::Workspace;

use super::FALLBACK_BRANCH_NAME;
use super::GitPanel;
use super::RemoteOperationKind;
use crate::picker_prompt;
use crate::remote_output;
use crate::remote_output::RemoteAction;
use crate::remote_output::SuccessMessage;

#[allow(clippy::type_complexity)]
fn clone_and_open(
    repo_url: SharedString,
    workspace: WeakEntity<Workspace>,
    window: &Window,
    cx: &App,
    on_success: Arc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + Send + Sync + 'static>,
) {
    let destination_prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Select as Repository Destination".into()),
    });

    window
        .spawn(cx, async move |cx| {
            let mut paths = destination_prompt.await.ok()?.ok()??;
            let mut destination_dir = paths.pop()?;

            let repo_name = repo_url
                .split('/')
                .next_back()
                .map_or("repository", |name| name.strip_suffix(".git").unwrap_or(name))
                .to_owned();

            let clone_task = workspace
                .update(cx, |_, cx| {
                    let fs = fs::Fs::global(cx);
                    let destination_dir = destination_dir.clone();
                    let repo_url = repo_url.clone();
                    cx.spawn(async move |_workspace, _cx| fs.git_clone(destination_dir.as_path(), &repo_url).await)
                })
                .ok()?;

            if let Err(error) = clone_task.await {
                workspace
                    .update(cx, |workspace, cx| {
                        let toast = StatusToast::new(error.to_string(), cx, |this, _| {
                            this.icon(Icon::new(IconName::XCircle).size(IconSize::Small).color(Color::Error))
                                .dismiss_button(true)
                        });
                        workspace.toggle_status_toast(toast, cx);
                    })
                    .log_err();
                return None;
            }

            let has_worktrees = workspace
                .read_with(cx, |workspace, cx| workspace.project().read(cx).worktrees(cx).next().is_some())
                .ok()?;

            let prompt_answer = if has_worktrees {
                cx.update(|window, _| {
                    window.prompt(
                        PromptLevel::Info,
                        &format!("Git Clone: {repo_name}"),
                        None,
                        &["Add repo to project", "Open repo in new project"],
                    )
                })
                .ok()?
                .await
                .ok()?
            } else {
                // Don't ask if project is empty
                0
            };

            destination_dir.push(&repo_name);

            match prompt_answer {
                0 => {
                    workspace
                        .update_in(cx, |workspace, window, cx| {
                            let create_task = workspace
                                .project()
                                .update(cx, |project, cx| project.create_worktree(destination_dir.as_path(), true, cx));

                            let workspace_weak = cx.weak_entity();
                            let on_success = Arc::clone(&on_success);
                            cx.spawn_in(window, async move |_window, cx| {
                                if create_task.await.log_err().is_some() {
                                    workspace_weak
                                        .update_in(cx, |workspace, window, cx| {
                                            (on_success)(workspace, window, cx);
                                        })
                                        .ok();
                                }
                            })
                            .detach();
                        })
                        .ok()?;
                }
                1 => {
                    workspace
                        .update(cx, move |_, cx| {
                            let destination_path = destination_dir.clone();
                            let on_success = Arc::clone(&on_success);

                            workspace::open_new(workspace::OpenOptions::default(), cx, move |workspace, window, cx| {
                                cx.activate(true);

                                let create_task = workspace.project().update(cx, |project, cx| {
                                    project.create_worktree(destination_path.as_path(), true, cx)
                                });

                                let workspace_weak = cx.weak_entity();
                                cx.spawn_in(window, async move |_window, cx| {
                                    if create_task.await.log_err().is_some() {
                                        workspace_weak
                                            .update_in(cx, |workspace, window, cx| {
                                                (on_success)(workspace, window, cx);
                                            })
                                            .ok();
                                    }
                                })
                                .detach();
                            })
                            .detach();
                        })
                        .ok();
                }
                _ => {}
            }

            Some(())
        })
        .detach();
}

impl GitPanel {
    fn get_fetch_options(&self, window: &Window, cx: &Context<Self>) -> Task<Option<FetchOptions>> {
        let repo = self.active_repository.clone();
        let workspace = self.workspace.clone();

        cx.spawn_in(window, async move |_, cx| {
            let repo = repo?;
            let remotes = repo.update(cx, |repo, _| repo.get_remotes(None, false)).await.ok()?.log_err()?;

            let mut remotes: Vec<_> = remotes.into_iter().map(FetchOptions::Remote).collect();
            if remotes.len() > 1 {
                remotes.push(FetchOptions::All);
            }
            let selection = cx
                .update(|window, cx| {
                    picker_prompt::prompt(
                        "Pick which remote to fetch",
                        remotes.iter().map(|r| r.name()).collect(),
                        workspace,
                        window,
                        cx,
                    )
                })
                .ok()?
                .await?;
            remotes.get(selection).cloned()
        })
    }

    pub(crate) fn fetch(&mut self, is_fetch_all: bool, window: &Window, cx: &mut Context<Self>) {
        if !self.can_push_and_pull(cx) {
            return;
        }

        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        if !self.start_remote_operation(RemoteOperationKind::Fetch, cx) {
            return;
        }

        let this = cx.weak_entity();

        let fetch_options =
            if is_fetch_all { Task::ready(Some(FetchOptions::All)) } else { self.get_fetch_options(window, cx) };

        window
            .spawn(cx, async move |cx| {
                let _clear_pending_remote_operation = cx.on_drop(&this, |this, cx| {
                    this.clear_remote_operation(cx);
                });

                let Some(fetch_options) = fetch_options.await else {
                    return Ok(());
                };
                let fetch = repo.update(cx, |repo, cx| repo.fetch(fetch_options.clone(), cx));

                let remote_message = fetch.await?;
                this.update(cx, |this, cx| {
                    let action = match fetch_options {
                        FetchOptions::All | FetchOptions::Unshallow => RemoteAction::Fetch(None),
                        FetchOptions::Remote(remote) => RemoteAction::Fetch(Some(remote)),
                    };
                    match remote_message {
                        Ok(remote_message) => this.show_remote_output(action, remote_message, cx),
                        Err(e) => {
                            log::error!("Error while fetching {e:?}");
                            this.show_error_toast(action.name(), e, cx);
                        }
                    }

                    anyhow::Ok(())
                })
                .ok();
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
    }

    pub(crate) fn git_clone(&self, repo: String, window: &Window, cx: &Context<Self>) {
        let workspace = self.workspace.clone();

        clone_and_open(repo.into(), workspace, window, cx, Arc::new(|_workspace: &mut Workspace, _window, _cx| {}));
    }

    pub(crate) fn git_init(&self, window: &Window, cx: &Context<Self>) {
        let worktrees = self
            .project
            .read(cx)
            .visible_worktrees(cx)
            .filter(|worktree| !worktree.read(cx).is_single_file())
            .collect::<Vec<_>>();

        let worktree = if let [worktree] = worktrees.as_slice() {
            Task::ready(Some(worktree.clone()))
        } else if worktrees.is_empty() {
            let result = window.prompt(
                PromptLevel::Warning,
                "Unable to initialize a git repository",
                Some("Open a directory first"),
                &["OK"],
            );
            cx.background_executor()
                .spawn(async move {
                    result.await.ok();
                })
                .detach();
            return;
        } else {
            let worktree_directories = worktrees
                .iter()
                .map(|worktree| worktree.read(cx).abs_path())
                .map(|worktree_abs_path| {
                    if let Ok(path) = worktree_abs_path.strip_prefix(paths::home_dir()) {
                        Path::new("~").join(path).to_string_lossy().to_string().into()
                    } else {
                        worktree_abs_path.to_string_lossy().into_owned().into()
                    }
                })
                .collect_vec();
            let prompt = picker_prompt::prompt(
                "Where would you like to initialize this git repository?",
                worktree_directories,
                self.workspace.clone(),
                window,
                cx,
            );

            cx.spawn(async move |_, _| prompt.await.map(|ix| worktrees[ix].clone()))
        };

        cx.spawn_in(window, async move |this, cx| {
            let Some(worktree) = worktree.await else {
                return;
            };

            let Ok(result) = this.update(cx, |this, cx| {
                this.project.read(cx).git_init(worktree.read(cx).abs_path(), FALLBACK_BRANCH_NAME.into(), cx)
            }) else {
                return;
            };

            let result = result.await;

            this.update_in(cx, |this, _, cx| match result {
                Ok(()) => {}
                Err(e) => this.show_error_toast("init", e, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn pull(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.can_push_and_pull(cx) {
            return;
        }
        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        let Some(branch) = repo.read(cx).branch.clone() else {
            return;
        };
        if !self.start_remote_operation(RemoteOperationKind::Pull, cx) {
            return;
        }

        let remote = self.get_remote(false, false, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let _clear_pending_remote_operation = cx.on_drop(&this, |this, cx| {
                this.clear_remote_operation(cx);
            });

            let remote = match remote.await {
                Ok(Some(remote)) => remote,
                Ok(None) => {
                    return Ok(());
                }
                Err(e) => {
                    log::error!("Failed to get current remote: {e}");
                    this.update(cx, |this, cx| this.show_error_toast("pull", e, cx)).ok();
                    return Ok(());
                }
            };

            // Autostash rewrites files on disk, which would leave dirty buffers
            // in conflict with them.
            let saved = this.update_in(cx, |this, window, cx| this.save_dirty_files(window, cx))??;
            if !saved.await? {
                return Ok(());
            }

            let branch_name = branch.upstream.is_none().then(|| branch.name().to_owned().into());

            let pull = repo.update(cx, |repo, cx| repo.pull(branch_name, remote.name.clone(), cx));

            let remote_message = pull.await?;

            let action = RemoteAction::Pull(remote);
            this.update(cx, |this, cx| match remote_message {
                Ok(remote_message) => this.show_remote_output(action, remote_message, cx),
                Err(e) => {
                    log::error!("Error while pulling {e:?}");
                    this.show_error_toast(action.name(), e, cx);
                }
            })
            .ok();

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub(crate) fn push(&mut self, force_push: bool, select_remote: bool, window: &Window, cx: &mut Context<Self>) {
        if !self.can_push_and_pull(cx) {
            return;
        }
        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        let Some(branch) = repo.read(cx).branch.clone() else {
            return;
        };
        if !self.start_remote_operation(RemoteOperationKind::Push, cx) {
            return;
        }

        let options = if force_push {
            Some(PushOptions::Force)
        } else {
            match branch.upstream {
                Some(Upstream { tracking: UpstreamTracking::Gone, .. }) | None => Some(PushOptions::SetUpstream),
                _ => None,
            }
        };
        let remote = self.get_remote(select_remote, true, window, cx);

        cx.spawn_in(window, async move |this, cx| {
            let _clear_pending_remote_operation = cx.on_drop(&this, |this, cx| {
                this.clear_remote_operation(cx);
            });

            let remote = match remote.await {
                Ok(Some(remote)) => remote,
                Ok(None) => {
                    this.update(cx, |this, cx| {
                        this.show_error_toast(
                            "push",
                            anyhow::anyhow!(
                                "No remote available to push to. Add a remote to be able to publish changes."
                            ),
                            cx,
                        );
                    })
                    .ok();
                    return Ok(());
                }
                Err(e) => {
                    log::error!("Failed to get current remote: {e}");
                    this.update(cx, |this, cx| this.show_error_toast("push", e, cx)).ok();
                    return Ok(());
                }
            };

            let push = repo.update(cx, |repo, cx| {
                repo.push(
                    branch.name().to_owned().into(),
                    branch
                        .upstream
                        .as_ref()
                        .filter(|u| matches!(u.tracking, UpstreamTracking::Tracked(_)))
                        .and_then(|u| u.branch_name())
                        .unwrap_or_else(|| branch.name())
                        .to_owned()
                        .into(),
                    remote.name.clone(),
                    options,
                    cx,
                )
            });

            let remote_output = push.await?;

            let action = RemoteAction::Push(branch.name().to_owned().into(), remote);
            this.update(cx, |this, cx| match remote_output {
                Ok(remote_message) => this.show_remote_output(action, remote_message, cx),
                Err(e) => {
                    log::error!("Error while pushing {e:?}");
                    this.show_error_toast(action.name(), e, cx);
                }
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// Updates git's configuration, adding the directory of the current
    /// worktree to the `safe.directory` config, ensuring that, even if the user
    /// that's running the application is not the owner of `.git/`, it can still
    /// read the repository's contents.
    pub(super) fn add_safe_directory(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_repository) = &self.active_repository else {
            return;
        };

        let path = active_repository.update(cx, |repository, _cx| repository.snapshot().work_directory_abs_path);

        if let Some(path_str) = path.to_str() {
            let path_arg = String::from(path_str);
            let args = vec![String::from("--global"), String::from("--add"), String::from("safe.directory"), path_arg];

            self.project.read(cx).git_config(path, args, cx).detach_and_log_err(cx);
        }
    }

    pub(super) fn can_push_and_pull(&self, cx: &App) -> bool {
        !self.rebase_in_progress(cx)
    }

    /// A rebase is stopped on conflicts: only resolving, continuing or
    /// aborting applies until it ends.
    pub(super) fn rebase_in_progress(&self, cx: &App) -> bool {
        self.active_repository.as_ref().is_some_and(|repo| repo.read(cx).merge.rebase_in_progress)
    }

    fn save_dirty_files(&self, window: &Window, cx: &mut Context<Self>) -> anyhow::Result<Task<anyhow::Result<bool>>> {
        self.workspace.update(cx, |workspace, cx| workspace.save_dirty_files(window, cx))
    }

    pub(crate) fn rebase(&mut self, action: RebaseAction, window: &Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        if !self.rebase_in_progress(cx) || (action == RebaseAction::Continue && self.has_unstaged_conflicts()) {
            return;
        }
        if !self.start_remote_operation(RemoteOperationKind::Rebase, cx) {
            return;
        }

        cx.spawn_in(window, async move |this, cx| {
            let _clear_pending_remote_operation = cx.on_drop(&this, |this, cx| {
                this.clear_remote_operation(cx);
            });

            // Both ways out rewrite files on disk.
            let saved = this.update_in(cx, |this, window, cx| this.save_dirty_files(window, cx))??;
            if !saved.await? {
                return Ok(());
            }

            let output = repo.update(cx, |repo, _| repo.rebase(action)).await?;
            this.update(cx, |this, cx| match output {
                Ok(output) => this.show_remote_output(RemoteAction::Rebase(action), output, cx),
                Err(e) => this.show_error_toast(action.name(), e, cx),
            })
            .ok();

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn start_remote_operation(&mut self, kind: RemoteOperationKind, cx: &mut Context<Self>) -> bool {
        if self.pending_remote_operation.is_some() {
            return false;
        }

        self.pending_remote_operation = Some(kind);
        cx.notify();
        true
    }

    fn clear_remote_operation(&mut self, cx: &mut Context<Self>) {
        self.pending_remote_operation.take();
        cx.notify();
    }

    fn get_remote(
        &self,
        always_select: bool,
        is_push: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl Future<Output = anyhow::Result<Option<Remote>>> + use<> {
        let repo = self.active_repository.clone();
        let workspace = self.workspace.clone();
        let mut cx = window.to_async(cx);

        async move {
            let repo = repo.context("No active repository")?;
            let current_remotes: Vec<Remote> = repo
                .update(&mut cx, |repo, _| {
                    let current_branch = if always_select {
                        None
                    } else {
                        let current_branch = repo.branch.as_ref().context("No active branch")?;
                        Some(current_branch.name().to_owned())
                    };
                    anyhow::Ok(repo.get_remotes(current_branch, is_push))
                })?
                .await??;

            let current_remotes: Vec<_> = current_remotes.into_iter().map(|remotes| remotes.name).collect();
            let selection = cx
                .update(|window, cx| {
                    picker_prompt::prompt(
                        "Pick which remote to push to",
                        current_remotes.clone(),
                        workspace,
                        window,
                        cx,
                    )
                })?
                .await;

            Ok(selection.map(|selection| Remote { name: current_remotes[selection].clone() }))
        }
    }

    fn show_remote_output(&self, action: RemoteAction, info: RemoteCommandOutput, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        workspace.update(cx, |workspace, cx| {
            let SuccessMessage { message, style } = remote_output::format_output(&action, info);
            let workspace_weak = cx.weak_entity();
            let operation = action.name();

            let status_toast = StatusToast::new(message, cx, move |this, _cx| {
                use remote_output::SuccessStyle::Toast;
                use remote_output::SuccessStyle::ToastWithLog;
                let this = this.icon(Icon::new(IconName::GitBranch).size(IconSize::Small).color(Color::Muted));
                match style {
                    Toast => this,
                    ToastWithLog { output } => this.action("View Log", move |window, cx| {
                        let output = output.clone();
                        let output = format!("stdout:\n{}\nstderr:\n{}", output.stdout, output.stderr);
                        workspace_weak
                            .update(cx, move |workspace, cx| open_output(operation, workspace, &output, window, cx))
                            .ok();
                    }),
                }
                .dismiss_button(true)
            });
            workspace.toggle_status_toast(status_toast, cx);
        });
    }
}
