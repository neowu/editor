use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use anyhow::Result;
use futures::channel::oneshot;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Context;
use gpui::Entity;
use gpui::PathPromptOptions;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::WeakEntity;
use gpui::WindowHandle;
use gpui_util::ResultExt as _;
use project::DirectoryLister;
use project::Project;
use project::ProjectEntryId;
use project::ProjectPath;
use project::ResolvedPath;
use project::Worktree;
use ui::Window;
use url::Url;
use util::paths::SanitizedPath;
use util::rel_path::RelPath;

use super::AddFolderToProject;
use super::CloseIntent;
use super::OpenMode;
use super::OpenVisible;
use super::Pane;
use super::Workspace;
use super::workspace_windows;
use crate::item::ItemBufferKind;
use crate::item::ItemHandle;
use crate::item::ProjectItem;
use crate::item_registry::ProjectItemRegistry;
use crate::item_registry::WorkspaceItemBuilder;
use crate::pane_group::Member;
use crate::pane_group::SplitDirection;

impl Workspace {
    pub fn prompt_for_open_path(
        &mut self,
        path_prompt_options: PathPromptOptions,
        lister: DirectoryLister,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>> {
        {
            let (tx, rx) = oneshot::channel();
            let abs_path = cx.prompt_for_paths(path_prompt_options);

            cx.spawn_in(window, async move |workspace, cx| {
                let Ok(result) = abs_path.await else {
                    return Ok(());
                };

                match result {
                    Ok(result) => {
                        tx.send(result).ok();
                    }
                    Err(err) => {
                        let rx = workspace.update_in(cx, |workspace, window, cx| {
                            workspace.show_error(err.to_string(), cx);
                            let prompt =
                                workspace.on_prompt_for_open_path.take().expect("open path prompt handler is set");
                            let rx = prompt(workspace, lister, window, cx);
                            workspace.on_prompt_for_open_path = Some(prompt);
                            rx
                        })?;
                        if let Ok(path) = rx.await {
                            tx.send(path).ok();
                        }
                    }
                }
                anyhow::Ok(())
            })
            .detach();

            rx
        }
    }

    pub fn prompt_for_new_path(
        &mut self,
        lister: DirectoryLister,
        suggested_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Option<Vec<PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        cx.spawn_in(window, async move |workspace, cx| {
            let abs_path = workspace.update(cx, |workspace, cx| {
                let relative_to = workspace
                    .most_recent_active_path(cx)
                    .and_then(|p| p.parent().map(|x| x.to_path_buf()))
                    .or_else(|| {
                        let project = workspace.project.read(cx);
                        project.visible_worktrees(cx).next().map(|worktree| worktree.read(cx).abs_path().to_path_buf())
                    })
                    .or_else(env::home_dir)
                    .unwrap_or_else(|| PathBuf::from(""));
                cx.prompt_for_new_path(&relative_to, suggested_name.as_deref())
            })?;
            let abs_path = match abs_path.await? {
                Ok(path) => path,
                Err(err) => {
                    let rx = workspace.update_in(cx, |workspace, window, cx| {
                        workspace.show_error(err.to_string(), cx);

                        let prompt = workspace.on_prompt_for_new_path.take().expect("new path prompt handler is set");
                        let rx = prompt(workspace, lister, suggested_name, window, cx);
                        workspace.on_prompt_for_new_path = Some(prompt);
                        rx
                    })?;
                    if let Ok(path) = rx.await {
                        tx.send(path).ok();
                    }
                    return anyhow::Ok(());
                }
            };

            tx.send(abs_path.map(|path| vec![path])).ok();
            anyhow::Ok(())
        })
        .detach();

        rx
    }

    pub fn open_workspace_for_paths(
        &mut self,
        // replace_current_window: bool,
        mut open_mode: OpenMode,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        let requesting_window = window.window_handle().downcast::<Workspace>();
        let has_worktree = self.project.read(cx).worktrees(cx).next().is_some();
        let has_dirty_items = self.items(cx).any(|item| item.is_dirty(cx));

        let workspace_is_empty = !has_worktree && !has_dirty_items;
        if workspace_is_empty {
            open_mode = OpenMode::Activate;
        }

        cx.spawn(async move |_, cx| {
            let OpenResult { workspace, .. } = cx
                .update(|cx| {
                    open_paths(
                        &paths,
                        OpenOptions {
                            requesting_window,
                            open_mode,
                            workspace_matching: if open_mode == OpenMode::NewWindow {
                                WorkspaceMatching::None
                            } else {
                                WorkspaceMatching::default()
                            },
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .await?;
            Ok(workspace)
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn open_paths(
        &mut self,
        mut abs_paths: Vec<PathBuf>,
        options: OpenOptions,
        pane: Option<WeakEntity<Pane>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Vec<Option<anyhow::Result<Box<dyn ItemHandle>>>>> {
        let fs = fs::Fs::global(cx);

        let caller_ordered_abs_paths = abs_paths.clone();

        // Sort the paths to ensure we add worktrees for parents before their children.
        abs_paths.sort_unstable();
        cx.spawn_in(window, async move |this, cx| {
            let mut tasks = Vec::with_capacity(abs_paths.len());

            for abs_path in &abs_paths {
                let visible = match options.visible.as_ref().unwrap_or(&OpenVisible::None) {
                    OpenVisible::All => Some(true),
                    OpenVisible::None => Some(false),
                    OpenVisible::OnlyFiles => match fs.metadata(abs_path).await.log_err() {
                        Some(Some(metadata)) => Some(!metadata.is_dir),
                        Some(None) => Some(true),
                        None => None,
                    },
                    OpenVisible::OnlyDirectories => match fs.metadata(abs_path).await.log_err() {
                        Some(Some(metadata)) => Some(metadata.is_dir),
                        Some(None) => Some(false),
                        None => None,
                    },
                };
                let project_path = match visible {
                    Some(visible) => match this
                        .update(cx, |this, cx| {
                            Workspace::project_path_for_path(this.project.clone(), abs_path, visible, cx)
                        })
                        .log_err()
                    {
                        Some(project_path) => project_path.await.log_err(),
                        None => None,
                    },
                    None => None,
                };

                let this = this.clone();
                let abs_path: Arc<Path> = SanitizedPath::new(&abs_path).as_path().into();
                let fs = Arc::clone(&fs);
                let pane = pane.clone();
                let task = cx.spawn(async move |cx| {
                    let (worktree, project_path) = project_path?;
                    let entry_is_directory = worktree.read_with(cx, |worktree, _| {
                        let entry = if project_path.path.as_unix_str().is_empty() {
                            worktree.root_entry()
                        } else {
                            worktree.entry_for_path(&project_path.path)
                        };
                        entry.map(project::Entry::is_dir)
                    });
                    let is_directory = match entry_is_directory {
                        Some(is_directory) => is_directory,
                        None => fs.is_dir(&abs_path).await,
                    };

                    if is_directory {
                        // Opening a directory should not race to update the active entry.
                        // We'll select/reveal a deterministic final entry after all paths finish opening.
                        None
                    } else {
                        Some(
                            this.update_in(cx, |this, window, cx| {
                                this.open_path(project_path, pane, options.focus.unwrap_or(true), window, cx)
                            })
                            .ok()?
                            .await,
                        )
                    }
                });
                tasks.push(task);
            }

            let results = futures::future::join_all(tasks).await;

            // Determine the winner using the fake/abstract FS metadata, not `Path::is_dir`.
            let mut winner: Option<(PathBuf, bool)> = None;
            for abs_path in caller_ordered_abs_paths.into_iter().rev() {
                if let Some(Some(metadata)) = fs.metadata(&abs_path).await.log_err() {
                    if !metadata.is_dir {
                        winner = Some((abs_path, false));
                        break;
                    }
                    if winner.is_none() {
                        winner = Some((abs_path, true));
                    }
                } else if winner.is_none() {
                    winner = Some((abs_path, false));
                }
            }

            // Compute the winner entry id on the foreground thread and emit once, after all
            // paths finish opening. This avoids races between concurrently-opening paths
            // (directories in particular) and makes the resulting project panel selection
            // deterministic.
            if let Some((winner_abs_path, winner_is_dir)) = winner {
                'emit_winner: {
                    let winner_abs_path: Arc<Path> = SanitizedPath::new(&winner_abs_path).as_path().into();

                    let visible = match options.visible.as_ref().unwrap_or(&OpenVisible::None) {
                        OpenVisible::All => true,
                        OpenVisible::None => false,
                        OpenVisible::OnlyFiles => !winner_is_dir,
                        OpenVisible::OnlyDirectories => winner_is_dir,
                    };

                    let Some(worktree_task) = this
                        .update(cx, |workspace, cx| {
                            workspace.project.update(cx, |project, cx| {
                                project.find_or_create_worktree(winner_abs_path.as_ref(), visible, cx)
                            })
                        })
                        .ok()
                    else {
                        break 'emit_winner;
                    };

                    let Ok((worktree, _)) = worktree_task.await else {
                        break 'emit_winner;
                    };

                    let Ok(Some(entry_id)) = this.update(cx, |_, cx| {
                        let worktree = worktree.read(cx);
                        let worktree_abs_path = worktree.abs_path();
                        let entry = if winner_abs_path.as_ref() == worktree_abs_path.as_ref() {
                            worktree.root_entry()
                        } else {
                            winner_abs_path.strip_prefix(worktree_abs_path.as_ref()).ok().and_then(|relative_path| {
                                let relative_path = RelPath::new(relative_path).log_err()?;
                                worktree.entry_for_path(&relative_path)
                            })
                        }?;
                        Some(entry.id)
                    }) else {
                        break 'emit_winner;
                    };

                    this.update(cx, |workspace, cx| {
                        workspace.project.update(cx, |_, cx| {
                            cx.emit(project::Event::ActiveEntryChanged(Some(entry_id)));
                        });
                    })
                    .ok();
                }
            }

            results
        })
    }

    pub fn open_resolved_path(
        &mut self,
        path: ResolvedPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        match path {
            ResolvedPath::ProjectPath { project_path, .. } => self.open_path(project_path, None, true, window, cx),
            ResolvedPath::AbsPath { path, .. } => self.open_abs_path(
                PathBuf::from(path),
                OpenOptions { visible: Some(OpenVisible::None), ..Default::default() },
                window,
                cx,
            ),
        }
    }

    pub fn add_folder_to_project(&mut self, _: &AddFolderToProject, window: &mut Window, cx: &mut Context<Self>) {
        let _project = self.project.read(cx);

        let paths = self.prompt_for_open_path(
            PathPromptOptions { files: false, directories: true, multiple: true, prompt: None },
            DirectoryLister(self.project.clone()),
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(paths) = paths.await.log_err().flatten() {
                let results = this
                    .update_in(cx, |this, window, cx| {
                        this.open_paths(
                            paths,
                            OpenOptions { visible: Some(OpenVisible::All), ..Default::default() },
                            None,
                            window,
                            cx,
                        )
                    })?
                    .await;
                for result in results.into_iter().flatten() {
                    result.log_err();
                }
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub fn project_path_for_path(
        project: Entity<Project>,
        abs_path: &Path,
        visible: bool,
        cx: &mut App,
    ) -> Task<Result<(Entity<Worktree>, ProjectPath)>> {
        let entry = project.update(cx, |project, cx| project.find_or_create_worktree(abs_path, visible, cx));
        cx.spawn(async move |cx| {
            let (worktree, path) = entry.await?;
            let worktree_id = worktree.read_with(cx, |t, _| t.id());
            Ok((worktree, ProjectPath { worktree_id, path }))
        })
    }

    pub fn add_item_to_center(
        &mut self,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(center_pane) = self.last_active_center_pane.clone() {
            if let Some(center_pane) = center_pane.upgrade() {
                center_pane.update(cx, |pane, cx| pane.add_item(item, true, true, None, window, cx));
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn add_item_to_active_pane(
        &mut self,
        item: Box<dyn ItemHandle>,
        destination_index: Option<usize>,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.add_item(self.active_pane.clone(), item, destination_index, false, focus_item, window, cx);
    }

    pub fn add_item(
        &mut self,
        pane: Entity<Pane>,
        item: Box<dyn ItemHandle>,
        destination_index: Option<usize>,
        activate_pane: bool,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        pane.update(cx, |pane, cx| pane.add_item(item, activate_pane, focus_item, destination_index, window, cx));
    }

    pub fn split_item(
        &mut self,
        split_direction: SplitDirection,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_pane = self.split_pane(self.active_pane.clone(), split_direction, window, cx);
        self.add_item(new_pane, item, None, true, true, window, cx);
    }

    pub fn open_abs_path(
        &mut self,
        abs_path: PathBuf,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        cx.spawn_in(window, async move |workspace, cx| {
            let open_paths_task_result = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_paths(vec![abs_path.clone()], options, None, window, cx)
                })
                .with_context(|| format!("open abs path {abs_path:?} task spawn"))?
                .await;
            anyhow::ensure!(
                open_paths_task_result.len() == 1,
                "open abs path {abs_path:?} task returned incorrect number of results"
            );
            match open_paths_task_result.into_iter().next().expect("ensured single task result") {
                Some(open_result) => open_result.with_context(|| format!("open abs path {abs_path:?} task join")),
                None => anyhow::bail!("open abs path {abs_path:?} task returned None"),
            }
        })
    }

    pub fn split_abs_path(
        &mut self,
        abs_path: PathBuf,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let project_path_task = Workspace::project_path_for_path(self.project.clone(), &abs_path, visible, cx);
        cx.spawn_in(window, async move |this, cx| {
            let (_, path) = project_path_task.await?;
            this.update_in(cx, |this, window, cx| this.split_path(path, window, cx))?.await
        })
    }

    /// A file lives in one pane: if it is open anywhere, that tab is revealed.
    /// Otherwise it opens in `pane`, or the last active center pane for `None`.
    pub fn open_path(
        &mut self,
        path: impl Into<ProjectPath>,
        pane: Option<WeakEntity<Pane>>,
        focus_item: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        self.open_path_preview(path, pane, focus_item, false, true, window, cx)
    }

    pub fn open_path_preview(
        &mut self,
        path: impl Into<ProjectPath>,
        pane: Option<WeakEntity<Pane>>,
        focus_item: bool,
        allow_preview: bool,
        activate: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let requested_pane = pane.unwrap_or_else(|| {
            self.last_active_center_pane
                .clone()
                .unwrap_or_else(|| self.panes.first().expect("There must be an active pane").downgrade())
        });

        let workspace = self.weak_self.clone();
        let project_path = path.into();
        let task = self.load_path(project_path.clone(), window, cx);
        window.spawn(cx, async move |cx| {
            let (project_entry_id, build_item) = task.await?;
            let pane = workspace
                .read_with(cx, |workspace, cx| {
                    workspace.pane_containing_project_item(&requested_pane, project_entry_id, &project_path, cx)
                })
                .ok()
                .flatten()
                .map(|pane| pane.downgrade())
                .unwrap_or(requested_pane);

            pane.update_in(cx, |pane, window, cx| {
                pane.open_item(
                    project_entry_id,
                    project_path,
                    focus_item,
                    allow_preview,
                    activate,
                    None,
                    window,
                    cx,
                    build_item,
                )
            })
        })
    }

    fn pane_containing_project_item(
        &self,
        requested_pane: &WeakEntity<Pane>,
        project_entry_id: Option<ProjectEntryId>,
        project_path: &ProjectPath,
        cx: &App,
    ) -> Option<Entity<Pane>> {
        let pane_contains_project_item = |pane: &Entity<Pane>| {
            pane.read(cx).items().any(|item| {
                if item.buffer_kind(cx) != ItemBufferKind::Singleton {
                    return false;
                }

                if let Some(project_entry_id) = project_entry_id {
                    item.project_entry_ids(cx).as_slice() == [project_entry_id]
                } else {
                    item.project_path(cx).as_ref() == Some(project_path)
                }
            })
        };

        let requested_pane = requested_pane.upgrade();
        if let Some(requested_pane) = requested_pane.as_ref()
            && pane_contains_project_item(requested_pane)
        {
            return Some(requested_pane.clone());
        }

        self.panes.iter().find_map(|pane| {
            if requested_pane.as_ref() == Some(pane) || !pane_contains_project_item(pane) {
                None
            } else {
                Some(pane.clone())
            }
        })
    }

    /// Opens a URL or file path, intelligently routing to the appropriate handler:
    ///
    /// - `http://` and `https://` URLs are opened in the system's default browser
    /// - `file://` URIs are opened as files in the editor
    /// - Paths without a URL scheme are treated as file paths:
    ///   - Absolute paths are opened directly
    ///   - Relative paths are first resolved against `base_path` (if provided),
    ///     then against visible project worktrees
    /// - Other URI schemes (e.g., `mailto:`, `vscode:`) are passed to the system handler
    ///
    /// # Arguments
    /// * `url_or_path` - The URL or file path to open
    /// * `base_path` - Optional base directory for resolving relative paths (e.g., the
    ///   directory containing a markdown file). If not provided, relative paths are
    ///   resolved against project worktrees.
    ///
    /// This method provides a unified way to handle links that may be either URLs
    /// or file paths, such as those found in markdown documents, terminal output,
    /// or LSP responses.
    pub fn open_url_or_file(
        &mut self,
        url_or_path: &str,
        base_path: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut open_abs_path = |this: &mut Self, path, cx: &mut _| {
            let url_or_path = url_or_path.to_owned();
            let task = this.open_abs_path(
                path,
                OpenOptions { visible: Some(OpenVisible::None), ..Default::default() },
                window,
                cx,
            );
            (**cx)
                .spawn(async move |cx| {
                    if task.await.is_err() {
                        cx.update(|cx| cx.open_url(&url_or_path));
                    }
                })
                .detach();
        };

        if let Ok(url) = Url::parse(url_or_path) {
            match url.scheme() {
                "http" | "https" => cx.open_url(url_or_path),
                "file" => open_abs_path(self, PathBuf::from(url.path()), cx),
                _ => cx.open_url(url_or_path),
            }
            return;
        }

        // Not a valid URL - treat as a file path
        let project = self.project();

        // If it's an absolute path, open it directly
        if Path::new(url_or_path).has_root() {
            open_abs_path(self, PathBuf::from(url_or_path), cx);
            return;
        }

        let path = Path::new(url_or_path);
        // Try to resolve relative path against base_path first
        if let Some(base) = base_path {
            let resolved_path = base.join(path);
            if resolved_path.exists() {
                open_abs_path(self, resolved_path, cx);
                return;
            }
        }

        // Try to resolve against project worktrees
        let project_path = project.read(cx).find_project_path(path, cx);
        if let Some(project_path) = project_path {
            let url_or_path = url_or_path.to_owned();
            let task = self.open_path(project_path, None, true, window, cx);
            (**cx)
                .spawn(async move |cx| {
                    if task.await.is_err() {
                        cx.update(|cx| cx.open_url(&url_or_path));
                    }
                })
                .detach();
            return;
        }

        // Couldn't resolve as a file path - try opening as URL anyway
        // (the OS might be able to handle it)
        cx.open_url(url_or_path);
    }

    pub fn split_path(
        &mut self,
        path: impl Into<ProjectPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        self.split_path_preview(path, false, None, window, cx)
    }

    pub fn split_path_preview(
        &mut self,
        path: impl Into<ProjectPath>,
        allow_preview: bool,
        split_direction: Option<SplitDirection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let pane = self
            .last_active_center_pane
            .clone()
            .unwrap_or_else(|| self.panes.first().expect("There must be an active pane").downgrade());

        if let Member::Pane(center_pane) = &self.center.root
            && center_pane.read(cx).items_len() == 0
        {
            return self.open_path(path, Some(pane), true, window, cx);
        }

        let project_path = path.into();
        let task = self.load_path(project_path.clone(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let (project_entry_id, build_item) = task.await?;
            this.update_in(cx, move |this, window, cx| -> Option<_> {
                // Already open: reveal it where it is rather than splitting off a copy.
                if let Some(open_pane) = this.pane_containing_project_item(&pane, project_entry_id, &project_path, cx) {
                    return Some(open_pane.update(cx, |open_pane, cx| {
                        open_pane.open_item(
                            project_entry_id,
                            project_path,
                            true,
                            allow_preview,
                            true,
                            None,
                            window,
                            cx,
                            build_item,
                        )
                    }));
                }
                let pane = pane.upgrade()?;
                let new_pane = this.split_pane(pane, split_direction.unwrap_or(SplitDirection::Right), window, cx);
                new_pane.update(cx, |new_pane, cx| {
                    Some(new_pane.open_item(
                        project_entry_id,
                        project_path,
                        true,
                        allow_preview,
                        true,
                        None,
                        window,
                        cx,
                        build_item,
                    ))
                })
            })
            .map(|option| option.context("pane was dropped"))?
        })
    }

    pub(super) fn load_path(
        &self,
        path: ProjectPath,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<(Option<ProjectEntryId>, WorkspaceItemBuilder)>> {
        let registry = cx.default_global::<ProjectItemRegistry>().clone();
        registry.open_path(self.project(), &path, window, cx)
    }

    pub fn find_project_item<T>(
        &self,
        pane: &Entity<Pane>,
        project_item: &Entity<T::Item>,
        cx: &App,
    ) -> Option<Entity<T>>
    where
        T: ProjectItem,
    {
        use project::ProjectItem as _;
        let project_item = project_item.read(cx);
        let entry_id = project_item.entry_id(cx);
        let project_path = project_item.project_path(cx);

        let mut item = entry_id.and_then(|entry_id| pane.read(cx).item_for_entry(entry_id, cx));
        if item.is_none()
            && let Some(project_path) = project_path
        {
            item = pane.read(cx).item_for_path(project_path, cx);
        }

        let item = item?;
        item.downcast::<T>()
    }

    /// A file lives in one pane: if `project_item` is open anywhere, that tab is
    /// revealed. Otherwise it opens in `pane`, or the active pane for `None`.
    pub fn open_project_item<T>(
        &mut self,
        pane: Option<Entity<Pane>>,
        project_item: Entity<T::Item>,
        activate_pane: bool,
        focus_item: bool,
        keep_old_preview: bool,
        allow_new_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<T>
    where
        T: ProjectItem,
    {
        let requested_pane = pane.unwrap_or_else(|| self.active_pane.clone());
        let existing_item = self
            .find_project_item(&requested_pane, &project_item, cx)
            .map(|item| (requested_pane.clone(), item))
            .or_else(|| {
                self.panes.iter().find_map(|pane| {
                    if pane == &requested_pane {
                        None
                    } else {
                        self.find_project_item(pane, &project_item, cx).map(|item| (pane.clone(), item))
                    }
                })
            });
        let pane = existing_item.as_ref().map(|(pane, _)| pane.clone()).unwrap_or(requested_pane);
        let old_item_id = pane.read(cx).active_item().map(|item| item.item_id());

        if let Some((_, item)) = existing_item {
            if !keep_old_preview
                && let Some(old_id) = old_item_id
                && old_id != item.item_id()
            {
                // switching to a different item, so unpreview old active item
                pane.update(cx, |pane, _| {
                    pane.unpreview_item_if_preview(old_id);
                });
            }

            self.activate_item(&item, activate_pane, focus_item, window, cx);
            if !allow_new_preview {
                pane.update(cx, |pane, _| {
                    pane.unpreview_item_if_preview(item.item_id());
                });
            }
            return item;
        }

        let item = pane.update(cx, |pane, cx| {
            cx.new(|cx| T::for_project_item(self.project().clone(), Some(pane), project_item, window, cx))
        });
        let mut destination_index = None;
        pane.update(cx, |pane, cx| {
            if !keep_old_preview && let Some(old_id) = old_item_id {
                pane.unpreview_item_if_preview(old_id);
            }
            if allow_new_preview {
                destination_index = pane.replace_preview_item_id(item.item_id(), window, cx);
            }
        });

        self.add_item(pane, Box::new(item.clone()), destination_index, activate_pane, focus_item, window, cx);
        item
    }

    /// Replaces this window's workspace with one for `paths`, once this one
    /// consents to closing. Returns this workspace if it doesn't.
    pub fn open_project(
        &mut self,
        paths: Vec<PathBuf>,
        open_mode: OpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Workspace>>> {
        cx.spawn_in(window, async move |this, cx| {
            let should_continue = this
                .update_in(cx, |this, window, cx| this.prepare_to_close(CloseIntent::ReplaceWindow, window, cx))?
                .await?;
            if should_continue {
                this.update_in(cx, |this, window, cx| this.open_workspace_for_paths(open_mode, paths, window, cx))?
                    .await
            } else {
                this.upgrade().context("workspace was released")
            }
        })
    }
}

pub(super) fn prompt_and_open_paths(options: PathPromptOptions, create_new_window: bool, cx: &mut App) {
    if let Some(workspace_window) = workspace_windows(cx).into_iter().next() {
        workspace_window
            .update(cx, |workspace, window, cx| {
                prompt_for_open_path_and_open(workspace, options, create_new_window, window, cx);
            })
            .ok();
    } else {
        let task = Workspace::new_local(Vec::new(), None, None, OpenMode::Activate, cx);
        cx.spawn(async move |cx| {
            let OpenResult { window, .. } = task.await?;
            window.update(cx, |workspace, window, cx| {
                window.activate_window();
                prompt_for_open_path_and_open(workspace, options, create_new_window, window, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

pub fn prompt_for_open_path_and_open(
    workspace: &mut Workspace,
    options: PathPromptOptions,
    create_new_window: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let paths = workspace.prompt_for_open_path(options, DirectoryLister(workspace.project().clone()), window, cx);
    let workspace_window = window.window_handle().downcast::<Workspace>();
    cx.spawn_in(window, async move |this, cx| {
        let Some(paths) = paths.await.log_err().flatten() else {
            return;
        };
        if !create_new_window && let Some(handle) = workspace_window {
            if let Some(task) = handle
                .update(cx, |workspace, window, cx| workspace.open_project(paths, OpenMode::Activate, window, cx))
                .log_err()
            {
                task.await.log_err();
            }
            return;
        }
        if let Some(task) = this
            .update_in(cx, |this, window, cx| this.open_workspace_for_paths(OpenMode::NewWindow, paths, window, cx))
            .log_err()
        {
            task.await.log_err();
        }
    })
    .detach();
}

pub async fn find_existing_workspace(
    abs_paths: &[PathBuf],
    open_options: &OpenOptions,
    cx: &mut AsyncApp,
) -> (Option<(WindowHandle<Workspace>, Entity<Workspace>)>, OpenVisible) {
    let mut existing: Option<(WindowHandle<Workspace>, Entity<Workspace>)> = None;
    let open_visible = OpenVisible::All;
    let mut best_match = None;

    if open_options.workspace_matching != WorkspaceMatching::None {
        cx.update(|cx| {
            for window in workspace_windows(cx) {
                let Ok(workspace) = window.entity(cx) else {
                    continue;
                };
                let project = workspace.read(cx).project.read(cx);
                let m = match open_options.workspace_matching {
                    WorkspaceMatching::None => None,
                    WorkspaceMatching::MatchExact => project.visibility_for_paths(abs_paths, cx),
                    WorkspaceMatching::MatchSubpaths => project.visibility_for_subpaths(abs_paths, cx),
                };
                if m > best_match {
                    existing = Some((window, workspace.clone()));
                    best_match = m;
                }
            }
        });
    }
    (existing, open_visible)
}

/// Controls whether to reuse an existing workspace whose worktrees contain the
/// given paths, and how broadly to match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceMatching {
    /// Always open a new workspace. No matching against existing worktrees.
    None,
    /// Match paths against existing worktree roots and files within them.
    #[default]
    MatchExact,
    /// Match files and directories inside existing worktrees, excluding the
    /// worktree roots themselves.
    MatchSubpaths,
}

#[derive(Clone, Default)]
pub struct OpenOptions {
    pub visible: Option<OpenVisible>,
    pub focus: Option<bool>,
    pub workspace_matching: WorkspaceMatching,
    pub requesting_window: Option<WindowHandle<Workspace>>,
    pub open_mode: OpenMode,
}

impl OpenOptions {
    fn should_reuse_existing_window(&self) -> bool {
        !matches!(self.workspace_matching, WorkspaceMatching::None | WorkspaceMatching::MatchSubpaths)
            && self.open_mode != OpenMode::NewWindow
    }
}

/// The result of opening a workspace via [`open_paths`], [`Workspace::new_local`],
/// or [`Workspace::open_workspace_for_paths`].
pub struct OpenResult {
    pub window: WindowHandle<Workspace>,
    pub workspace: Entity<Workspace>,
    pub opened_items: Vec<Option<anyhow::Result<Box<dyn ItemHandle>>>>,
}

#[allow(clippy::type_complexity)]
pub fn open_paths(abs_paths: &[PathBuf], open_options: OpenOptions, cx: &mut App) -> Task<anyhow::Result<OpenResult>> {
    let abs_paths = abs_paths.to_vec();
    let fs = fs::Fs::global(cx);

    cx.spawn(async move |cx| {
        let (mut existing, mut open_visible) = find_existing_workspace(&abs_paths, &open_options, cx).await;

        // Fallback: if no workspace contains the paths and all paths are files,
        // prefer an existing local workspace window (active window first).
        if open_options.should_reuse_existing_window() && existing.is_none() {
            let all_paths = abs_paths.iter().map(|path| fs.metadata(path));
            let all_metadatas =
                futures::future::join_all(all_paths).await.into_iter().filter_map(|result| result.ok().flatten());

            if all_metadatas.into_iter().all(|file| !file.is_dir) {
                cx.update(|cx| {
                    let windows = workspace_windows(cx);
                    let window = cx
                        .active_window()
                        .and_then(|window| window.downcast::<Workspace>())
                        .filter(|window| windows.contains(window))
                        .or_else(|| windows.into_iter().next());
                    if let Some(window) = window
                        && let Ok(workspace) = window.entity(cx)
                    {
                        existing = Some((window, workspace));
                        open_visible = OpenVisible::None;
                    }
                });
            }
        }

        if let Some((existing, target_workspace)) = existing {
            let open_task = existing
                .update(cx, |workspace, window, cx| {
                    window.activate_window();
                    workspace.open_paths(
                        abs_paths,
                        OpenOptions { visible: Some(open_visible), ..Default::default() },
                        None,
                        window,
                        cx,
                    )
                })?
                .await;

            _ = existing.update(cx, |workspace, _, cx| {
                for item in open_task.iter().flatten() {
                    if let Err(e) = item {
                        workspace.show_error(format!("Error: {e}"), cx);
                    }
                }
            });

            Ok(OpenResult { window: existing, workspace: target_workspace, opened_items: open_task })
        } else {
            let result = cx
                .update(move |cx| {
                    Workspace::new_local(abs_paths, open_options.requesting_window, None, open_options.open_mode, cx)
                })
                .await;

            if let Ok(ref result) = result {
                result
                    .window
                    .update(cx, |_, window, _cx| {
                        window.activate_window();
                    })
                    .log_err();
            }

            result
        }
    })
}

pub fn open_new(
    open_options: OpenOptions,
    cx: &mut App,
    init: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static + Send,
) -> Task<anyhow::Result<()>> {
    let addition = open_options.open_mode;
    let task = Workspace::new_local(Vec::new(), open_options.requesting_window, Some(Box::new(init)), addition, cx);
    cx.spawn(async move |cx| {
        let OpenResult { window, .. } = task.await?;
        window
            .update(cx, |_, window, _cx| {
                window.activate_window();
            })
            .ok();
        Ok(())
    })
}
