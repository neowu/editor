use std::ffi::OsStr;

use ed_actions::git::SelectRepo;
use project::git_store::Repository;
mod application_menu;
mod worktree_branch;

use std::path::Path;

use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Focusable;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Styled;
use gpui::Subscription;
use gpui::WeakEntity;
use gpui::Window;
use gpui::actions;
use project::Project;
use project::git_store::GitStoreEvent;
use project::linked_worktree_short_name;
use project::repo_identity_path;
use ui::PopoverMenu;
use ui::PopoverMenuHandle;
use ui::Tooltip;
use ui::prelude::Color;
use ui::prelude::FluentBuilder;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::SharedString;
use ui::prelude::h_flex;
use workspace::Workspace;

use crate::application_menu::ApplicationMenu;
use crate::worktree_branch::header_text_button;

const MAX_PROJECT_NAME_LENGTH: usize = 40;

fn linked_worktree_name_anchor<'a>(
    main_worktree_path: Option<&'a Path>,
    repository_identity_path: Option<&'a Path>,
    is_linked_worktree: bool,
) -> Option<&'a Path> {
    main_worktree_path.or_else(|| is_linked_worktree.then_some(repository_identity_path).flatten())
}

actions!(
    title_bar,
    [
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let item = cx.new(|cx| TitleBar::new(workspace, window, cx));
        // The project menu is the only folder and repository switcher.
        let title_bar = item.downgrade();
        workspace.register_action(move |_, _: &ToggleProjectMenu, window, cx| {
            toggle_project_menu(title_bar.clone(), window, cx);
        });
        let title_bar = item.downgrade();
        workspace.register_action(move |_, _: &SelectRepo, window, cx| {
            toggle_project_menu(title_bar.clone(), window, cx);
        });
        workspace.set_titlebar_item(item.into(), window, cx);
    })
    .detach();
}

/// Toggles the header's project menu. Deferred because building the menu
/// reads the workspace, which is being updated while its actions run.
fn toggle_project_menu(title_bar: WeakEntity<TitleBar>, window: &Window, cx: &mut App) {
    window.defer(cx, move |window, cx| {
        title_bar.update(cx, |title_bar, cx| title_bar.project_menu_handle.toggle(window, cx)).ok();
    });
}

/// The project's name, worktree and branch, shown in the
/// window header above the sidebar or at the start of the top-left tab bar.
pub struct TitleBar {
    project: Entity<Project>,

    workspace: WeakEntity<Workspace>,
    application_menu: Option<Entity<ApplicationMenu>>,
    project_menu_handle: PopoverMenuHandle<recent_projects::RecentProjects>,
    _subscriptions: Vec<Subscription>,
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_project_items = !self.application_menu.as_ref().is_some_and(|menu| menu.read(cx).all_menus_shown());
        let active = show_project_items.then(|| ActiveRepository::of(&self.project, cx));

        h_flex().h_full().children(self.application_menu.clone()).when_some(active, |title_bar, active| {
            let worktree_and_branch = active
                .repository
                .and_then(|repository| self.render_worktree_and_branch(repository, active.linked_worktree_name, cx));
            title_bar.child(self.render_project_name(active.project_name, window, cx)).children(worktree_and_branch)
        })
    }
}

/// What the header names: the active folder, and the active repository when it
/// contains the folder or is nested in it.
struct ActiveRepository {
    project_name: Option<SharedString>,
    repository: Option<Entity<Repository>>,
    linked_worktree_name: Option<SharedString>,
}

impl ActiveRepository {
    fn of(project: &Entity<Project>, cx: &App) -> ActiveRepository {
        let mut active = ActiveRepository { project_name: None, repository: None, linked_worktree_name: None };
        let git_store = project.read(cx).git_store().read(cx);
        let Some(worktree) = git_store.active_worktree(cx) else {
            return active;
        };
        let worktree_abs_path = worktree.read(cx).abs_path();
        active.repository = git_store.active_repository().filter(|repo| {
            let repo_path = &repo.read(cx).work_directory_abs_path;
            worktree_abs_path.starts_with(repo_path.as_ref()) || repo_path.starts_with(&worktree_abs_path)
        });
        active.project_name =
            worktree.read(cx).root_name().file_name().map(|name| SharedString::from(name.to_string()));
        let Some(active_repo) = &active.repository else {
            return active;
        };
        let nested_label = active.project_name.as_ref().and_then(|folder_name| {
            nested_repository_label(folder_name, &worktree_abs_path, &active_repo.read(cx).work_directory_abs_path)
        });
        if let Some(label) = nested_label {
            active.project_name = Some(label.into());
            return active;
        }
        let repo = active_repo.read(cx);
        let identity = repo_identity_path(&repo.common_dir_abs_path);
        active.linked_worktree_name =
            linked_worktree_name_anchor(repo.main_worktree_abs_path(), Some(identity), repo.is_linked_worktree())
                .and_then(|name_anchor_path| {
                    linked_worktree_short_name(name_anchor_path, repo.work_directory_abs_path.as_ref())
                })
                .or_else(|| repo.is_linked_worktree().then_some(active.project_name.clone()).flatten());

        let display_name =
            if identity.extension() == Some(OsStr::new("git")) { identity.file_stem() } else { identity.file_name() };

        if let Some(repo_name) = display_name.and_then(|n| n.to_str()) {
            let name = if visible_worktrees_in_repository(project, repo, cx) == 1
                && let Ok(relative) = worktree_abs_path.strip_prefix(&*repo.work_directory_abs_path)
                && !relative.as_os_str().is_empty()
            {
                format!("{}/{}", repo_name, relative.display())
            } else {
                repo_name.to_string()
            };
            active.project_name = Some(SharedString::from(name));
        }
        active
    }
}

/// The header name while a repository nested inside the folder is active:
/// `folder/path/to/repo`. `None` when the repository contains the folder.
fn nested_repository_label(folder_name: &str, worktree_path: &Path, repo_path: &Path) -> Option<String> {
    let relative = repo_path.strip_prefix(worktree_path).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(format!("{folder_name}/{}", relative.display()))
}

fn visible_worktrees_in_repository(project: &Entity<Project>, repository: &Repository, cx: &App) -> usize {
    let repo_path = &repository.work_directory_abs_path;
    project
        .read(cx)
        .visible_worktrees(cx)
        .filter(|worktree| {
            let worktree_path = worktree.read(cx).abs_path();
            worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref())
        })
        .count()
}

impl TitleBar {
    pub fn new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();

        let application_menu = if option_env!("ED_USE_CROSS_PLATFORM_MENU").is_some() {
            Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
        } else {
            None
        };

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&git_store, move |_, _, event, cx| match event {
            GitStoreEvent::ActiveRepositoryChanged(_)
            | GitStoreEvent::ActiveWorktreeChanged
            | GitStoreEvent::RepositoryUpdated(_, _, true) => {
                cx.notify();
            }
            _ => {}
        }));
        subscriptions.push(cx.observe(&project, |_, _, cx| cx.notify()));
        if let Some(workspace_entity) = workspace.weak_handle().upgrade() {
            subscriptions.push(cx.subscribe(&workspace_entity, |_, _, event: &workspace::Event, cx| {
                if matches!(event, workspace::Event::WorktreeCreationChanged) {
                    cx.notify();
                }
            }));
        }

        Self {
            application_menu,
            workspace: workspace.weak_handle(),
            project,
            project_menu_handle: PopoverMenuHandle::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Whether the project menu has more than one folder or repository to
    /// switch to.
    fn has_location_choices(&self, cx: &App) -> bool {
        let project = self.project.read(cx);
        let git_store = project.git_store().read(cx);
        let mut worktrees = project.visible_worktrees(cx);
        let first = worktrees.next();
        worktrees.next().is_some()
            || first.is_some_and(|worktree| !git_store.nested_repositories(worktree.read(cx).id(), cx).is_empty())
    }

    fn render_project_name(&self, name: Option<SharedString>, _: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let is_project_selected = name.is_some();
        let has_choices = self.has_location_choices(cx);

        let display_name = if let Some(ref name) = name {
            util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH)
        } else {
            "Open Recent Project".to_string()
        };

        let focus_handle =
            workspace.upgrade().map(|w| w.read(cx).focus_handle(cx)).unwrap_or_else(|| cx.focus_handle());

        PopoverMenu::new("recent-projects-menu")
            .with_handle(self.project_menu_handle.clone())
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                header_text_button("project_name_trigger", display_name)
                    .when(has_choices, |this| {
                        this.end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    })
                    .when(!is_project_selected, |s| s.color(Color::Muted)),
                move |_window, cx| {
                    let title = if has_choices { "Switch Folder" } else { "Recent Projects" };
                    Tooltip::for_action(title, &ed_actions::OpenRecent::default(), cx)
                },
            )
            .anchor(gpui::Anchor::TopLeft)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::nested_repository_label;

    #[test]
    fn test_nested_repository_label() {
        let folder = Path::new("/code/notebook");
        assert_eq!(
            nested_repository_label("notebook", folder, Path::new("/code/notebook/vendor/katex")).as_deref(),
            Some("notebook/vendor/katex")
        );
        assert_eq!(nested_repository_label("notebook", folder, folder), None);
        assert_eq!(nested_repository_label("notebook", folder, Path::new("/code")), None);
    }
}
