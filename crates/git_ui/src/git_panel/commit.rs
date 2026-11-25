use std::sync::Arc;

use anyhow::Context as _;
use editor::Editor;
use editor::EditorMode;
use editor::MultiBufferOffset;
use editor::RewrapOptions;
use editor::SizingBehavior;
use editor::actions::SelectAll;
use git::Amend;
use git::Commit;
use git::Signoff;
use git::SkipHooks;
use git::ToggleCommitEditor;
use git::ToggleFillCommitEditor;
use git::repository::CommitDetails;
use git::repository::CommitOptions;
use git::repository::GitCommitTemplate;
use git::repository::ResetMode;
use gpui::AppContext as _;
use gpui::Entity;
use gpui::Focusable as _;
use gpui::PromptLevel;
use gpui::Task;
use itertools::Itertools as _;
use language::Buffer;
use ui::prelude::App;
use ui::prelude::Context;
use ui::prelude::SharedString;
use ui::prelude::Window;
use util::ResultExt as _;

use super::GitPanel;
use super::MAX_PANEL_EDITOR_LINES;
use super::commit_message_editor;
use super::prompt;

impl GitPanel {
    pub fn commit_message_buffer(&self, cx: &App) -> Entity<Buffer> {
        self.commit_editor.read(cx).buffer().read(cx).as_singleton().expect("commit editor has a singleton buffer")
    }

    pub(super) fn on_commit(&mut self, _: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_editor.focus_handle(cx).contains_focused(window, cx) {
            let options = self.commit_options();
            self.commit_changes(options, window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Enters the amend state on first invocation, loading the last commit
    /// message for editing. On second invocation, performs the amend commit.
    pub(super) fn on_amend(&mut self, _: &Amend, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commit_editor.focus_handle(cx).contains_focused(window, cx) {
            cx.propagate();
        } else if self.head_commit(cx).is_some() {
            if self.amend_pending {
                let options = self.commit_options();
                self.commit_changes(options, window, cx);
            } else {
                self.toggle_amend_pending(cx);
            }
        }
    }

    pub fn head_commit(&self, cx: &App) -> Option<CommitDetails> {
        self.active_repository.as_ref().and_then(|repo| repo.read(cx).head_commit.as_ref()).cloned()
    }

    pub fn load_last_commit_message(&mut self, cx: &mut Context<Self>) {
        let Some(head_commit) = self.head_commit(cx) else {
            return;
        };

        let recent_sha = head_commit.sha.to_string();
        let detail_task = self.load_commit_details(recent_sha, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(message) = detail_task.await.map(|detail| detail.message) {
                this.update(cx, |this, cx| {
                    this.commit_message_buffer(cx).update(cx, |buffer, cx| {
                        let start = buffer.anchor_before(0);
                        let end = buffer.anchor_after(buffer.len());
                        buffer.edit([(start..end, message)], None, cx);
                    });
                })
                .log_err();
            }
        })
        .detach();
    }

    fn custom_or_suggested_commit_message(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let git_commit_language = self.commit_editor.read(cx).language_at(MultiBufferOffset(0), cx);
        let message = self.commit_editor.read(cx).text(cx);
        if message.is_empty() {
            return self.suggest_commit_message(cx).filter(|message| !message.trim().is_empty());
        } else if message.trim().is_empty() {
            return None;
        }
        let buffer = cx.new(|cx| {
            let mut buffer = Buffer::local(message, cx);
            buffer.set_language(git_commit_language, cx);
            buffer
        });
        let editor = cx.new(|cx| Editor::for_buffer(buffer, None, window, cx));
        let wrapped_message = editor.update(cx, |editor, cx| {
            editor.select_all(&SelectAll, window, cx);
            editor.rewrap(
                RewrapOptions {
                    override_language_settings: false,
                    preserve_existing_whitespace: true,
                    line_length: None,
                },
                cx,
            );
            editor.text(cx)
        });
        if wrapped_message.trim().is_empty() {
            return None;
        }
        Some(wrapped_message)
    }

    fn has_commit_message(&self, cx: &Context<Self>) -> bool {
        let text = self.commit_editor.read(cx).text(cx);
        if !text.trim().is_empty() {
            true
        } else if text.is_empty() {
            self.suggest_commit_message(cx).is_some_and(|text| !text.trim().is_empty())
        } else {
            false
        }
    }

    pub(crate) const fn commit_options(&self) -> CommitOptions {
        CommitOptions {
            amend: self.amend_pending,
            signoff: self.signoff_enabled,
            allow_empty: false,
            no_verify: self.skip_hooks_enabled,
        }
    }

    pub(crate) fn commit_changes(&mut self, options: CommitOptions, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_repository) = self.active_repository.clone() else {
            return;
        };
        if self.rebase_in_progress(cx) {
            return;
        }
        let error_spawn = |message, window: &mut Window, cx: &mut App| {
            let prompt = window.prompt(PromptLevel::Warning, message, None, &["OK"]);
            cx.spawn(async move |_| {
                prompt.await.ok();
            })
            .detach();
        };

        if self.has_unstaged_conflicts() {
            error_spawn("There are still conflicts. You must stage these before committing", window, cx);
            return;
        }

        let commit_message = self.custom_or_suggested_commit_message(window, cx);

        let Some(message) = commit_message else {
            self.commit_editor.read(cx).focus_handle(cx).focus(window, cx);
            return;
        };

        let task = if self.has_staged_changes() {
            // Repository serializes all git operations, so we can just send a commit immediately
            let commit_task = active_repository.update(cx, |repo, cx| repo.commit(message.into(), None, options, cx));
            cx.background_spawn(async move { commit_task.await? })
        } else {
            let changed_files = self
                .change_entries_by_path()
                .filter(|status_entry| !status_entry.status.is_created())
                .map(|status_entry| status_entry.repo_path.clone())
                .collect::<Vec<_>>();

            if changed_files.is_empty() && !options.amend {
                error_spawn("No changes to commit", window, cx);
                return;
            }

            let stage_task = active_repository.update(cx, |repo, cx| repo.stage_entries(changed_files, cx));
            cx.spawn(async move |_, cx| {
                stage_task.await?;
                let commit_task =
                    active_repository.update(cx, |repo, cx| repo.commit(message.into(), None, options, cx));
                commit_task.await?
            })
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.pending_commit.take();

                match result {
                    Ok(()) => {
                        this.set_skip_hooks_enabled(false, cx);
                        if options.amend {
                            this.set_amend_pending(false, cx);
                        } else {
                            this.commit_editor.update(cx, |editor, cx| editor.clear(window, cx));
                            this.original_commit_message = None;
                        }
                    }
                    Err(e) => this.show_error_toast("commit", e, cx),
                }
            })
            .ok();
        });

        self.pending_commit = Some(task);
    }

    pub(crate) fn uncommit(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repository.clone() else {
            return;
        };
        if self.rebase_in_progress(cx) {
            return;
        }

        let confirmation = self.check_for_pushed_commits(window, cx);
        let prior_head = self.load_commit_details("HEAD".to_owned(), cx);

        let task = cx.spawn_in(window, async move |this, cx| {
            let result = async {
                if matches!(confirmation.await, Ok(true)) {
                    let prior_head = prior_head.await?;

                    repo.update(cx, |repo, cx| repo.reset("HEAD^".to_owned(), ResetMode::Soft, cx)).await??;

                    Ok(Some(prior_head))
                } else {
                    Ok(None)
                }
            }
            .await;

            this.update_in(cx, |this, window, cx| {
                this.pending_commit.take();
                match result {
                    Ok(None) => {}
                    Ok(Some(prior_commit)) => {
                        this.commit_editor.update(cx, |editor, cx| editor.set_text(prior_commit.message, window, cx));
                    }
                    Err(e) => this.show_error_toast("reset", e, cx),
                }
            })
            .ok();
        });

        self.pending_commit = Some(task);
    }

    fn check_for_pushed_commits(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl Future<Output = anyhow::Result<bool>> + use<> {
        let repo = self.active_repository.clone();
        let mut cx = window.to_async(cx);

        async move {
            let repo = repo.context("No active repository")?;

            let pushed_to: Vec<SharedString> =
                repo.update(&mut cx, |repo, _| repo.check_for_pushed_commits()).await??;

            if pushed_to.is_empty() {
                Ok(true)
            } else {
                #[derive(strum::EnumIter, strum::VariantNames)]
                #[strum(serialize_all = "title_case")]
                enum CancelUncommit {
                    Uncommit,
                    Cancel,
                }
                let detail = format!("This commit was already pushed to {}.", pushed_to.into_iter().join(", "));
                let result = cx.update(|window, cx| prompt("Are you sure?", Some(&detail), window, cx))?.await?;

                match result {
                    CancelUncommit::Cancel => Ok(false),
                    CancelUncommit::Uncommit => Ok(true),
                }
            }
        }
    }

    /// Suggests a commit message based on the changed files and their statuses.
    pub fn suggest_commit_message(&self, cx: &App) -> Option<String> {
        if let Some(merge_message) =
            self.active_repository.as_ref().and_then(|repo| repo.read(cx).merge.message.as_ref())
        {
            return Some(merge_message.to_string());
        }

        let git_status_entry = if let Some(staged_entry) = &self.single_staged_entry {
            Some(staged_entry)
        } else if self.total_staged_count() == 0
            && let Some(single_tracked_entry) = &self.single_tracked_entry
        {
            Some(single_tracked_entry)
        } else {
            None
        }?;

        let action_text = if git_status_entry.status.is_deleted() {
            Some("Delete")
        } else if git_status_entry.status.is_created() {
            Some("Create")
        } else if git_status_entry.status.is_modified() {
            Some("Update")
        } else {
            None
        }?;

        let file_name = git_status_entry.repo_path.file_name().unwrap_or_default().to_owned();

        Some(format!("{action_text} {file_name}"))
    }

    pub(super) fn reopen_commit_buffer(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(active_repo) = self.active_repository.as_ref() else {
            self.reopen_commit_buffer_task = Task::ready(());
            return;
        };
        let load_buffer = active_repo.update(cx, |active_repo, cx| {
            let project = self.project.read(cx);
            active_repo.open_commit_buffer(Some(Arc::clone(project.languages())), project.buffer_store().clone(), cx)
        });
        let load_template = self.load_commit_template(cx);

        self.reopen_commit_buffer_task = cx.spawn_in(window, async move |git_panel, cx| {
            let result = async {
                // Set up the buffer before awaiting the commit template, which
                // must not block the commit editor from attaching to the buffer.
                let buffer = load_buffer.await?;
                git_panel.update_in(cx, |git_panel, window, cx| {
                    if git_panel.commit_editor.read(cx).buffer().read(cx).as_singleton().as_ref() != Some(&buffer) {
                        git_panel.commit_editor = cx.new(|cx| {
                            commit_message_editor(
                                buffer.clone(),
                                git_panel.suggest_commit_message(cx).map(SharedString::from),
                                window,
                                cx,
                            )
                        });
                    }
                })?;

                // Only apply the template if it's non-empty and the buffer has no
                // content, so we never override a commit message that was already
                // in progress.
                let commit_template = load_template.await?;
                git_panel.update(cx, |git_panel, cx| {
                    git_panel.commit_template = commit_template;

                    if let Some(commit_template) = git_panel.commit_template.as_ref()
                        && !commit_template.template.is_empty()
                        && buffer.read(cx).text().trim().is_empty()
                    {
                        buffer.update(cx, |buffer, cx| {
                            buffer.set_text(commit_template.template.clone(), cx);
                        });
                    }
                })?;

                anyhow::Ok(())
            }
            .await;

            result.log_err();
        });
    }

    pub fn configure_commit_button(&self, cx: &mut Context<Self>) -> (bool, &'static str) {
        if self.rebase_in_progress(cx) {
            (false, "Rebase in progress: continue or abort it first")
        } else if self.has_unstaged_conflicts() {
            (false, "You must resolve conflicts before committing")
        } else if !self.has_staged_changes() && !self.has_tracked_changes() && !self.amend_pending {
            (false, "No changes to commit")
        } else if self.pending_commit.is_some() {
            (false, "Commit in progress")
        } else if !self.has_commit_message(cx) {
            (false, "No commit message")
        } else {
            (true, self.commit_button_title())
        }
    }

    pub const fn commit_button_title(&self) -> &'static str {
        if self.amend_pending {
            if self.has_staged_changes() {
                "Amend"
            } else if self.has_tracked_changes() {
                "Amend Tracked"
            } else {
                "Amend"
            }
        } else if self.has_staged_changes() {
            "Commit"
        } else {
            "Commit Tracked"
        }
    }

    pub(super) fn toggle_fill_commit_editor(
        &mut self,
        _: &ToggleFillCommitEditor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_editor_expanded = !self.commit_editor_expanded;
        self.commit_editor.update(cx, |editor, _cx| {
            if self.commit_editor_expanded {
                editor.set_mode(EditorMode::Full {
                    scale_ui_elements_with_buffer_font_size: false,
                    show_active_line_background: false,
                    sizing_behavior: SizingBehavior::ExcludeOverscrollMargin,
                });
            } else {
                editor.set_mode(EditorMode::AutoHeight {
                    min_lines: MAX_PANEL_EDITOR_LINES,
                    max_lines: Some(MAX_PANEL_EDITOR_LINES),
                });
            }
        });

        cx.notify();
    }

    pub(super) fn toggle_commit_editor(&mut self, _: &ToggleCommitEditor, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_editor_collapsed = !self.commit_editor_collapsed;
        // Filling the panel and being hidden are mutually exclusive states, so
        // collapsing has to undo the fill rather than leave it dangling.
        if self.commit_editor_collapsed && self.commit_editor_expanded {
            self.toggle_fill_commit_editor(&ToggleFillCommitEditor, window, cx);
        }
        cx.notify();
    }

    pub fn load_commit_details(&self, sha: String, cx: &mut Context<Self>) -> Task<anyhow::Result<CommitDetails>> {
        let Some(repo) = self.active_repository.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active repo")));
        };
        repo.update(cx, |repo, cx| repo.show_commit(sha, cx))
    }

    pub fn load_commit_template(&self, cx: &mut Context<Self>) -> Task<anyhow::Result<Option<GitCommitTemplate>>> {
        let Some(repo) = self.active_repository.clone() else {
            return Task::ready(Err(anyhow::anyhow!("no active repo")));
        };
        repo.update(cx, |repo, cx| {
            let rx = repo.load_commit_template_text();
            cx.spawn(async move |_, _| rx.await?)
        })
    }

    pub const fn amend_pending(&self) -> bool {
        self.amend_pending
    }

    /// Sets the pending amend state, ensuring that the original commit message
    /// is either saved, when `value` is `true` and there's no pending amend, or
    /// restored, when `value` is `false` and there's a pending amend.
    pub fn set_amend_pending(&mut self, value: bool, cx: &mut Context<Self>) {
        if value && !self.amend_pending {
            let current_message = self.commit_message_buffer(cx).read(cx).text();
            self.original_commit_message = if current_message.trim().is_empty() { None } else { Some(current_message) };
        } else if !value && self.amend_pending {
            let message = self.original_commit_message.take().unwrap_or_default();
            self.commit_message_buffer(cx).update(cx, |buffer, cx| {
                let start = buffer.anchor_before(0);
                let end = buffer.anchor_after(buffer.len());
                buffer.edit([(start..end, message)], None, cx);
            });
        }

        self.amend_pending = value;
        cx.notify();
    }

    pub(super) fn set_skip_hooks_enabled(&mut self, value: bool, cx: &mut Context<Self>) {
        if self.skip_hooks_enabled != value {
            self.skip_hooks_enabled = value;
            cx.notify();
        }
    }

    pub fn toggle_skip_hooks(&mut self, _: &SkipHooks, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_skip_hooks_enabled(!self.skip_hooks_enabled, cx);
    }

    pub fn set_signoff_enabled(&mut self, value: bool, cx: &mut Context<Self>) {
        self.signoff_enabled = value;
        cx.notify();
    }

    pub fn toggle_signoff_enabled(&mut self, _: &Signoff, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_signoff_enabled(!self.signoff_enabled, cx);
    }

    pub(crate) fn toggle_amend_pending(&mut self, cx: &mut Context<Self>) {
        self.set_amend_pending(!self.amend_pending, cx);
        if self.amend_pending {
            self.load_last_commit_message(cx);
        }
    }
}
