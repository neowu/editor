use actions::menu;
use actions::search as search_actions;
use actions::workspace::CopyPath;
use actions::workspace::CopyRelativePath;
use editor::actions::MoveToBeginningOfLine;
use editor::actions::MoveToEndOfLine;
use ui::utils as ui_utils;
use util::paths;
use util::paths::SortMode;
use util::paths::SortOrder;

mod clipboard;
mod drag;
mod entry_edit;
mod render;
mod undo;
mod utils;
mod visible_entries;

use std::any::TypeId;
use std::cell::OnceCell;
use std::cmp;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use actions::menu::Confirm;
use actions::menu::SelectFirst;
use actions::menu::SelectLast;
use actions::menu::SelectNext;
use actions::menu::SelectPrevious;
use actions::project_panel::Toggle;
use actions::project_panel::ToggleFocus;
use actions::workspace::OpenWithSystem;
use anyhow::Context as _;
use anyhow::Result;
use editor::Editor;
use editor::EditorEvent;
use file_icons::FileIcons;
use fuzzy::CharBag;
use git::status::GitSummary;
use git_ui_core::file_diff_view::FileDiffView;
use gpui::Action;
use gpui::Anchor;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncWindowContext;
use gpui::Bounds;
use gpui::ClipboardItem;
use gpui::Context;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::InteractiveElement as _;
use gpui::KeyContext;
use gpui::Modifiers;
use gpui::Pixels;
use gpui::Point;
use gpui::PromptLevel;
use gpui::ScrollStrategy;
use gpui::Subscription;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::UniformListScrollHandle;
use gpui::WeakEntity;
use gpui::Window;
use gpui::actions;
use gpui::point;
use gpui::px;
use gpui::size;
use language::DiagnosticSeverity;
use markdown_preview::markdown_preview_view::MarkdownPreviewView;
use notifications::status_toast::StatusToast;
use project::Entry;
use project::EntryKind;
use project::GitEntry;
use project::GitEntryRef;
use project::Project;
use project::ProjectEntryId;
use project::ProjectPath;
use project::Worktree;
use project::WorktreeId;
use project::git_store::GitStoreEvent;
use project::git_store::RepositoryEvent;
use project::project_settings::GoToDiagnosticSeverityFilter;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use svg_preview::svg_preview_view::SvgPreviewView;
use ui::ContextMenu;
use ui::IconDecorationKind;
use ui::ScrollableHandle as _;
use ui::prelude::Color;
use ui::prelude::FluentBuilder as _;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::SharedString;
use util::ResultExt as _;
use util::markdown::MarkdownInlineCode;
use util::rel_path::RelPath;
use workspace::OpenInTerminal;
use workspace::SelectedEntry;
use workspace::SplitDirection;
use workspace::Workspace;
use workspace::dock::DockPosition;
use workspace::dock::Panel;
use workspace::dock::PanelEvent;
use workspace::dock::SIDEBAR_MIN_WIDTH;
use workspace::notifications::DetachAndPromptErr as _;
use workspace::notifications::NotifyTaskExt as _;

use crate::undo::Change;
use crate::undo::UndoManager;
use crate::undo::already_exists_error;

const PROJECT_PANEL_KEY: &str = "ProjectPanel";
const DEFAULT_WIDTH: Pixels = px(360.);
const INDENT_SIZE: Pixels = px(20.);
const TITLE_TOOLTIP_DELAY: Duration = Duration::from_millis(1500);
const NEW_ENTRY_ID: ProjectEntryId = ProjectEntryId::MAX;
/// The "External Libraries" group row is a one-entry section with ids that no
/// worktree uses, so row indexing, selection and scrolling treat it like any other row.
const LIBRARIES_WORKTREE_ID: WorktreeId = WorktreeId::from_usize(usize::MAX);
const LIBRARIES_ENTRY_ID: ProjectEntryId = ProjectEntryId::from_usize(usize::MAX - 1);

struct VisibleEntriesForWorktree {
    worktree_id: WorktreeId,
    /// Library worktrees are nested one level under the "External Libraries" row.
    depth_offset: usize,
    entries: Vec<GitEntry>,
    index: OnceCell<HashSet<Arc<RelPath>>>,
}

struct State {
    last_worktree_root_id: Option<ProjectEntryId>,
    /// Maps from leaf project entry ID to the currently selected ancestor.
    /// Relevant only for auto-fold dirs, where a single project panel entry may actually consist of several
    /// project entries (and all non-leaf nodes are guaranteed to be directories).
    ancestors: FxHashMap<ProjectEntryId, FoldedAncestors>,
    visible_entries: Vec<VisibleEntriesForWorktree>,
    max_width_item_index: Option<usize>,
    edit_state: Option<EditState>,
    temporarily_unfolded_pending_state: Option<TemporaryUnfoldedPendingState>,
    unfolded_dir_ids: HashSet<ProjectEntryId>,
    expanded_dir_ids: FxHashMap<WorktreeId, Vec<ProjectEntryId>>,
}

impl State {
    fn is_unfolded(&self, entry_id: ProjectEntryId) -> bool {
        self.unfolded_dir_ids.contains(&entry_id)
            || self.edit_state.as_ref().is_some_and(|edit_state| edit_state.temporarily_unfolded == Some(entry_id))
    }

    fn derive(old: &Self) -> Self {
        Self {
            last_worktree_root_id: None,
            ancestors: FxHashMap::default(),
            visible_entries: Vec::default(),
            max_width_item_index: None,
            edit_state: old.edit_state.clone(),
            temporarily_unfolded_pending_state: None,
            unfolded_dir_ids: old.unfolded_dir_ids.clone(),
            expanded_dir_ids: old.expanded_dir_ids.clone(),
        }
    }
}

pub struct ProjectPanel {
    project: Entity<Project>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    // An update loop that keeps incrementing/decrementing scroll offset while there is a dragged entry that's
    // hovered over the start/end of a list.
    hover_scroll_task: Option<Task<()>>,
    rendered_entries_len: usize,
    folded_directory_drag_target: Option<FoldedDirectoryDragTarget>,
    drag_target_entry: Option<DragTarget>,
    /// Last visible row of the highlighted drop target, refreshed each list render.
    drag_highlight_last_entry_id: Option<ProjectEntryId>,
    marked_entries: Vec<SelectedEntry>,
    selection: Option<SelectedEntry>,
    context_menu: Option<DeployedContextMenu>,
    filename_editor: Entity<Editor>,
    clipboard: Option<ClipboardEntry>,
    _dragged_entry_destination: Option<Arc<Path>>,
    workspace: WeakEntity<Workspace>,
    diagnostics: FxHashMap<(WorktreeId, Arc<RelPath>), DiagnosticSeverity>,
    diagnostic_counts: FxHashMap<(WorktreeId, Arc<RelPath>), DiagnosticCount>,
    diagnostic_summary_update: Task<()>,
    libraries_expanded: bool,
    // We keep track of the mouse down state on entries so we don't flash the UI
    // in case a user clicks to open a file.
    mouse_down: bool,
    hover_expand_task: Option<Task<()>>,
    previous_drag_position: Option<Point<Pixels>>,
    sticky_items_count: usize,
    /// Session-only filters toggled by `ToggleHideGitIgnore` / `ToggleHideHidden`.
    hide_gitignore: bool,
    hide_hidden: bool,

    update_visible_entries_task: UpdateVisibleEntriesTask,
    undo_manager: UndoManager,
    state: State,
}

struct UpdateVisibleEntriesTask {
    _visible_entries_task: Task<()>,
    focus_filename_editor: bool,
    autoscroll: bool,
    /// Kept across superseding updates, so a reveal isn't lost when another
    /// update (e.g. a library becoming listed) replaces its task.
    selected_entry: Option<(WorktreeId, ProjectEntryId)>,
}

#[derive(Debug)]
struct TemporaryUnfoldedPendingState {
    previously_focused_leaf_entry: SelectedEntry,
    temporarily_unfolded_active_entry_id: ProjectEntryId,
}

impl Default for UpdateVisibleEntriesTask {
    fn default() -> Self {
        UpdateVisibleEntriesTask {
            _visible_entries_task: Task::ready(()),
            focus_filename_editor: Default::default(),
            autoscroll: Default::default(),
            selected_entry: None,
        }
    }
}

/// Where a context menu should appear when it is deployed.
#[derive(Copy, Clone, Debug, PartialEq)]
enum ContextMenuPlacement {
    /// Slightly offset from the given mouse position, e.g. for a right click.
    AtMouse(Point<Pixels>),
    /// Next to the row of the entry the menu is deployed for, e.g. when opened from the keyboard.
    AtEntry,
}

const CONTEXT_MENU_KEYBOARD_OFFSET: Point<Pixels> = point(px(2.), px(2.));

struct DeployedContextMenu {
    menu: Entity<ContextMenu>,
    position: Point<Pixels>,
    /// Which corner of the menu is placed at `position`, determining the direction the menu opens
    /// in. When `None`, the menu is placed like any other anchored element.
    anchor: Option<Anchor>,
    _subscription: Subscription,
}

enum DragTarget {
    /// Dragging on an entry.
    Entry {
        /// The entry currently under the mouse cursor during a drag operation.
        entry_id: ProjectEntryId,
        /// Highlight this entry along with all of its children.
        highlight_entry_id: ProjectEntryId,
    },
    /// Dragging on background.
    Background,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct FoldedDirectoryDragTarget {
    entry_id: ProjectEntryId,
    index: usize,
    /// Whether we are dragging over the delimiter rather than the component itself.
    is_delimiter_target: bool,
}

#[derive(Clone, Debug)]
enum ValidationState {
    None,
    Warning(String),
    Error(String),
}

#[derive(Clone, Debug)]
struct EditState {
    worktree_id: WorktreeId,
    entry_id: ProjectEntryId,
    leaf_entry_id: Option<ProjectEntryId>,
    is_dir: bool,
    depth: usize,
    processing_filename: Option<Arc<RelPath>>,
    previously_focused: Option<SelectedEntry>,
    validation_state: ValidationState,
    temporarily_unfolded: Option<ProjectEntryId>,
}

impl EditState {
    const fn is_new_entry(&self) -> bool {
        self.leaf_entry_id.is_none()
    }
}

#[derive(Clone, Debug)]
enum ClipboardEntry {
    Copied(BTreeSet<SelectedEntry>),
    Cut(BTreeSet<SelectedEntry>),
}

#[derive(Clone, Copy)]
enum EntryTransferOperation {
    Copy,
    Move,
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
struct DiagnosticCount {
    error_count: usize,
    warning_count: usize,
}

impl DiagnosticCount {
    fn capped_error_count(&self) -> String {
        Self::capped_count(self.error_count)
    }

    fn capped_warning_count(&self) -> String {
        Self::capped_count(self.warning_count)
    }

    fn capped_count(count: usize) -> String {
        if count > 99 { "99+".to_owned() } else { count.to_string() }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct EntryDetails {
    filename: String,
    icon: Option<SharedString>,
    path: Arc<RelPath>,
    depth: usize,
    kind: EntryKind,
    is_ignored: bool,
    is_expanded: bool,
    is_selected: bool,
    is_marked: bool,
    is_editing: bool,
    is_processing: bool,
    is_cut: bool,
    sticky: Option<StickyDetails>,
    filename_text_color: Color,
    diagnostic_severity: Option<DiagnosticSeverity>,
    diagnostic_mark: Option<DiagnosticMark>,
    diagnostic_count: Option<DiagnosticCount>,
    git_status: GitSummary,
    worktree_id: WorktreeId,
    canonical_path: Option<Arc<Path>>,
}

/// The glyph a row's diagnostic mark decorates, and the shape of that mark.
///
/// A row has one indicator slot, so the mark decorates whichever glyph already fills
/// it rather than claiming a slot of its own. `Standalone` covers rows that draw no
/// glyph at all.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum DiagnosticMark {
    OnIcon(IconDecorationKind),
    Standalone(IconName),
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct StickyDetails {
    sticky_index: usize,
}

/// Moves the selected files or directories to the system trash. Undoable.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, Action)]
#[action(namespace = project_panel)]
#[serde(deny_unknown_fields)]
struct Delete {
    #[serde(default)]
    pub skip_prompt: bool,
}

/// Selects the next entry with diagnostics.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, Action)]
#[action(namespace = project_panel)]
#[serde(deny_unknown_fields)]
struct SelectNextDiagnostic {
    #[serde(default)]
    pub severity: GoToDiagnosticSeverityFilter,
}

/// Selects the previous entry with diagnostics.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, Action)]
#[action(namespace = project_panel)]
#[serde(deny_unknown_fields)]
struct SelectPrevDiagnostic {
    #[serde(default)]
    pub severity: GoToDiagnosticSeverityFilter,
}

actions!(
    project_panel,
    [
        /// Expands the selected entry in the project tree.
        ExpandSelectedEntry,
        /// Collapses the selected entry in the project tree.
        CollapseSelectedEntry,
        /// Collapses the selected entry and its children in the project tree.
        CollapseSelectedEntryAndChildren,
        /// Expands the selected entry and its children in the project tree.
        ExpandSelectedEntryAndChildren,
        /// Collapses all entries in the project tree.
        CollapseAllEntries,
        /// Expands all entries in the project tree.
        ExpandAllEntries,
        /// Creates a new directory.
        NewDirectory,
        /// Creates a new file.
        NewFile,
        /// Copies the selected file or directory.
        Copy,
        /// Duplicates the selected file or directory.
        Duplicate,
        /// Reveals the selected item in the system file manager.
        RevealInFileManager,
        /// Removes the selected folder from the project.
        RemoveFromProject,
        /// Cuts the selected file or directory.
        Cut,
        /// Pastes the previously cut or copied item.
        Paste,
        /// Renames the selected file or directory.
        Rename,
        /// Opens the selected file in the editor.
        Open,
        /// Opens the selected file in a permanent tab.
        OpenPermanent,
        /// Opens the selected file in a vertical split.
        OpenSplitVertical,
        /// Opens the selected file in a horizontal split.
        OpenSplitHorizontal,
        /// Toggles visibility of git-ignored files.
        ToggleHideGitIgnore,
        /// Toggles visibility of hidden files.
        ToggleHideHidden,
        /// Starts a new search in the selected directory.
        NewSearchInDirectory,
        /// Unfolds the selected directory.
        UnfoldDirectory,
        /// Folds the selected directory.
        FoldDirectory,
        /// Scroll half a page upwards
        ScrollUp,
        /// Scroll half a page downwards
        ScrollDown,
        /// Scroll until the cursor displays at the center
        ScrollCursorCenter,
        /// Scroll until the cursor displays at the top
        ScrollCursorTop,
        /// Scroll until the cursor displays at the bottom
        ScrollCursorBottom,
        /// Selects the parent directory.
        SelectParent,
        /// Selects the next entry with git changes.
        SelectNextGitEntry,
        /// Selects the previous entry with git changes.
        SelectPrevGitEntry,
        /// Selects the next directory.
        SelectNextDirectory,
        /// Selects the previous directory.
        SelectPrevDirectory,
        /// Opens a diff view to compare two marked files.
        CompareMarkedFiles,
        /// Undoes the last file operation.
        Undo,
        /// Redoes the last undone file operation.
        Redo,
        /// Opens a markdown preview for the selected file.
        OpenMarkdownPreview,
        /// Opens the context menu for the selected entry.
        OpenContextMenu,
        /// Opens an SVG preview for the selected file.
        OpenSvgPreview,
    ]
);

#[derive(Clone, Debug, Default)]
struct FoldedAncestors {
    current_ancestor_depth: usize,
    ancestors: Vec<ProjectEntryId>,
}

impl FoldedAncestors {
    const fn max_ancestor_depth(&self) -> usize {
        self.ancestors.len()
    }

    /// Note: This returns None for last item in ancestors list.
    fn active_ancestor(&self) -> Option<ProjectEntryId> {
        if self.current_ancestor_depth == 0 {
            return None;
        }
        self.ancestors.get(self.current_ancestor_depth).copied()
    }

    const fn active_index(&self) -> usize {
        self.max_ancestor_depth().saturating_sub(1).saturating_sub(self.current_ancestor_depth)
    }

    const fn set_active_index(&mut self, index: usize) -> bool {
        let new_depth = self.max_ancestor_depth().saturating_sub(1).saturating_sub(index);
        if self.current_ancestor_depth == new_depth {
            false
        } else {
            self.current_ancestor_depth = new_depth;
            true
        }
    }

    fn active_component(&self, file_name: &str) -> Option<String> {
        Path::new(file_name)
            .components()
            .nth(self.active_index())
            .map(|comp| comp.as_os_str().to_string_lossy().into_owned())
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ProjectPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &Toggle, window, cx| {
            if !workspace.toggle_panel_focus::<ProjectPanel>(window, cx) {
                workspace.close_panel::<ProjectPanel>(window, cx);
            }
        });

        workspace.register_action(|workspace, _: &ToggleHideGitIgnore, window, cx| {
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.hide_gitignore = !panel.hide_gitignore;
                    panel.update_visible_entries(None, false, false, window, cx);
                    cx.notify();
                });
            }
        });

        workspace.register_action(|workspace, _: &ToggleHideHidden, window, cx| {
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.hide_hidden = !panel.hide_hidden;
                    panel.update_visible_entries(None, false, false, window, cx);
                    cx.notify();
                });
            }
        });

        workspace.register_action(|workspace, action: &CollapseAllEntries, window, cx| {
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.collapse_all_entries(action, window, cx);
                });
            }
        });

        workspace.register_action(|workspace, action: &ExpandAllEntries, window, cx| {
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.expand_all_entries(action, window, cx);
                });
            }
        });

        workspace.register_action(|workspace, action: &Rename, window, cx| {
            workspace.open_panel::<ProjectPanel>(window, cx);
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    if let Some(first_marked) = panel.marked_entries.first() {
                        let first_marked = *first_marked;
                        panel.marked_entries.clear();
                        panel.selection = Some(first_marked);
                    }
                    panel.rename(action, window, cx);
                });
            }
        });

        workspace.register_action(|workspace, action: &Duplicate, window, cx| {
            workspace.open_panel::<ProjectPanel>(window, cx);
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.duplicate(action, window, cx);
                });
            }
        });

        workspace.register_action(|workspace, action: &Delete, window, cx| {
            if let Some(panel) = workspace.panel::<ProjectPanel>(cx) {
                panel.update(cx, |panel, cx| panel.delete(action, window, cx));
            }
        });

        // Forwards `git::FileHistory` to the file history opener installed by
        // `git_ui` when the project panel is the focused source of selection.
        // Lives here (and not in `git_ui`) so that `git_ui` does not need to
        // depend on `project_panel`, which would create a dependency cycle.
        workspace.register_action_renderer(|div, workspace, window, cx| {
            let Some(panel) = workspace.panel::<ProjectPanel>(cx) else {
                return div;
            };
            if !panel.read(cx).focus_handle(cx).contains_focused(window, cx) {
                return div;
            }
            if panel.read(cx).selected_entry_project_path(cx).is_none() {
                return div;
            }
            let workspace = workspace.weak_handle();
            div.capture_action(move |_: &git::FileHistory, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        let Some(panel) = workspace.panel::<ProjectPanel>(cx) else {
                            return;
                        };
                        let Some(project_path) = panel.read(cx).selected_entry_project_path(cx) else {
                            return;
                        };
                        git_ui_core::open_file_history(workspace, &project_path, window, cx);
                    })
                    .log_err();
                cx.stop_propagation();
            })
        });
    })
    .detach();
}

#[derive(Debug)]
pub enum Event {
    OpenedEntry { entry_id: ProjectEntryId, focus_opened_item: bool, allow_preview: bool },
    SplitEntry { entry_id: ProjectEntryId, allow_preview: bool, split_direction: Option<SplitDirection> },
}

struct DraggedProjectEntryView {
    selection: SelectedEntry,
    icon: Option<SharedString>,
    filename: String,
    click_offset: Point<Pixels>,
    selections: Arc<[SelectedEntry]>,
}

/// The last entry of `root_id`'s subtree, given entries in depth-first order.
fn last_entry_in_subtree<'a>(
    entries: impl IntoIterator<Item = (ProjectEntryId, &'a RelPath)>,
    root_id: ProjectEntryId,
) -> Option<ProjectEntryId> {
    let mut entries = entries.into_iter().skip_while(|(id, _)| *id != root_id);
    let (_, root_path) = entries.next()?;
    Some(entries.take_while(|(_, path)| path.starts_with(root_path)).last().map_or(root_id, |(id, _)| id))
}

enum TrashEntryOutcome {
    /// Entry was trashed, returning the `Change` to record on the undo stack.
    Trashed(Change),
    /// The entry or its worktree was already gone.
    Missing,
    /// Trashing the entry failed at the OS level.
    Failed,
}

struct RemovalPrompt {
    message: String,
    confirmation_label: &'static str,
}

impl ProjectPanel {
    fn new(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Entity<Self> {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let project_panel =
            cx.new(|cx: &mut Context<Self>| {
                let focus_handle = cx.focus_handle();
                cx.subscribe_in(&git_store, window, |this, _, event, window, cx| match event {
                    GitStoreEvent::RepositoryUpdated(_, RepositoryEvent::StatusesChanged, _)
                    | GitStoreEvent::RepositoryAdded
                    | GitStoreEvent::RepositoryRemoved(_) => {
                        this.update_visible_entries(None, false, false, window, cx);
                        cx.notify();
                    }
                    _ => {}
                })
                .detach();

                cx.subscribe_in(&project, window, |this, project, event, window, cx| match event {
                    project::Event::ActiveEntryChanged(Some(entry_id)) => {
                        this.reveal_entry(project.clone(), *entry_id, true, window, cx).ok();
                    }
                    project::Event::ActiveEntryChanged(None) => {
                        let is_active_item_file_diff_view = this
                            .workspace
                            .upgrade()
                            .and_then(|ws| ws.read(cx).active_item(cx))
                            .is_some_and(|item| item.act_as_type(TypeId::of::<FileDiffView>(), cx).is_some());
                        if !is_active_item_file_diff_view {
                            this.marked_entries.clear();
                        }
                    }
                    project::Event::RevealInProjectPanel(entry_id) => {
                        if this.reveal_entry(project.clone(), *entry_id, false, window, cx).log_err() == Some(()) {
                            cx.emit(PanelEvent::Activate);
                        }
                    }
                    project::Event::ActivateProjectPanel => {
                        cx.emit(PanelEvent::Activate);
                    }
                    project::Event::DiskBasedDiagnosticsFinished { .. } | project::Event::DiagnosticsUpdated { .. } => {
                        this.diagnostic_summary_update = cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(30)).await;
                            this.update(cx, |this, cx| {
                                if this.update_diagnostics(cx) {
                                    cx.notify();
                                }
                            })
                            .log_err();
                        });
                    }
                    project::Event::WorktreeRemoved(id) => {
                        this.state.expanded_dir_ids.remove(id);
                        this.update_visible_entries(None, false, false, window, cx);
                        cx.notify();
                    }
                    project::Event::WorktreeUpdatedEntries(_, _)
                    | project::Event::WorktreeAdded(_)
                    | project::Event::WorktreeOrderChanged => {
                        this.update_visible_entries(None, false, false, window, cx);
                        cx.notify();
                    }
                    project::Event::ExpandedAllForEntry(worktree_id, entry_id) => {
                        this.synchronously_expand_all_directories(*worktree_id, *entry_id, window, cx);
                    }
                    _ => {}
                })
                .detach();

                let filename_editor = cx.new(|cx| Editor::single_line(window, cx));

                cx.subscribe_in(&filename_editor, window, |project_panel, _, editor_event, window, cx| {
                    match editor_event {
                        EditorEvent::BufferEdited => {
                            project_panel.populate_validation_error(cx);
                            project_panel.autoscroll(cx);
                        }
                        EditorEvent::SelectionsChanged { .. } => {
                            project_panel.autoscroll(cx);
                        }
                        EditorEvent::Blurred
                            if window.is_window_active()
                                && project_panel
                                    .state
                                    .edit_state
                                    .as_ref()
                                    .is_some_and(|state| state.processing_filename.is_none()) =>
                        {
                            match project_panel.confirm_edit(false, window, cx) {
                                Some(task) => {
                                    task.detach_and_notify_err(project_panel.workspace.clone(), window, cx);
                                }
                                None => {
                                    project_panel.discard_edit_state(window, cx);
                                }
                            }
                        }
                        _ => {}
                    }
                })
                .detach();

                cx.observe_global::<FileIcons>(|_, cx| {
                    cx.notify();
                })
                .detach();

                let scroll_handle = UniformListScrollHandle::new();
                let weak_project_panel = cx.weak_entity();
                let mut this = Self {
                    project: project.clone(),
                    hover_scroll_task: None,
                    focus_handle,
                    rendered_entries_len: 0,
                    folded_directory_drag_target: None,
                    drag_target_entry: None,
                    drag_highlight_last_entry_id: None,
                    marked_entries: Vec::default(),
                    selection: None,
                    context_menu: None,
                    filename_editor,
                    clipboard: None,
                    _dragged_entry_destination: None,
                    workspace: workspace.weak_handle(),
                    diagnostics: FxHashMap::default(),
                    diagnostic_counts: FxHashMap::default(),
                    diagnostic_summary_update: Task::ready(()),
                    libraries_expanded: true,
                    scroll_handle,
                    mouse_down: false,
                    hover_expand_task: None,
                    previous_drag_position: None,
                    sticky_items_count: 0,
                    hide_gitignore: false,
                    hide_hidden: false,

                    state: State {
                        max_width_item_index: None,
                        edit_state: None,
                        temporarily_unfolded_pending_state: None,
                        last_worktree_root_id: Option::default(),
                        visible_entries: Vec::default(),
                        ancestors: FxHashMap::default(),
                        expanded_dir_ids: FxHashMap::default(),
                        unfolded_dir_ids: HashSet::default(),
                    },
                    update_visible_entries_task: UpdateVisibleEntriesTask::default(),
                    undo_manager: UndoManager::new(workspace.weak_handle(), weak_project_panel, cx),
                };
                this.update_visible_entries(None, false, false, window, cx);

                this
            });

        cx.subscribe_in(&project_panel, window, {
            let project_panel = project_panel.downgrade();
            move |workspace, _, event, window, cx| match *event {
                Event::OpenedEntry { entry_id, focus_opened_item, allow_preview } => {
                    if let Some(worktree) = project.read(cx).worktree_for_entry(entry_id, cx)
                        && let Some(entry) = worktree.read(cx).entry_for_id(entry_id)
                    {
                        let file_path = Arc::clone(&entry.path);
                        let worktree_id = worktree.read(cx).id();
                        let entry_id = entry.id;

                        workspace
                            .open_path_preview(
                                ProjectPath { worktree_id, path: file_path },
                                None,
                                focus_opened_item,
                                allow_preview,
                                true,
                                window,
                                cx,
                            )
                            .detach_and_prompt_err("Failed to open file", window, cx, move |e| {
                                // Returning Some prevents the error popup from saying "Try Again",
                                // which is a red herring for a file that is too large.
                                e.to_string().contains("File is too large to load").then(|| e.to_string())
                            });

                        if let Some(project_panel) = project_panel.upgrade() {
                            // Always select and mark the entry, regardless of whether it is opened or not.
                            project_panel.update(cx, |project_panel, _| {
                                let entry = SelectedEntry { worktree_id, entry_id };
                                project_panel.marked_entries.clear();
                                project_panel.marked_entries.push(entry);
                                project_panel.selection = Some(entry);
                            });
                            if !focus_opened_item {
                                let focus_handle = project_panel.read(cx).focus_handle.clone();
                                window.focus(&focus_handle, cx);
                            }
                        }
                    }
                }
                Event::SplitEntry { entry_id, allow_preview, split_direction } => {
                    if let Some(worktree) = project.read(cx).worktree_for_entry(entry_id, cx)
                        && let Some(entry) = worktree.read(cx).entry_for_id(entry_id)
                    {
                        workspace
                            .split_path_preview(
                                ProjectPath { worktree_id: worktree.read(cx).id(), path: Arc::clone(&entry.path) },
                                allow_preview,
                                split_direction,
                                window,
                                cx,
                            )
                            .detach_and_log_err(cx);
                    }
                }
            }
        })
        .detach();

        project_panel
    }

    pub fn load(workspace: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext) -> Result<Entity<Self>> {
        workspace.update_in(cx, ProjectPanel::new)
    }

    /// Returns whether anything changed.
    fn update_diagnostics(&mut self, cx: &Context<Self>) -> bool {
        let mut diagnostics: FxHashMap<(WorktreeId, Arc<RelPath>), DiagnosticSeverity> = FxHashMap::default();
        let mut diagnostic_counts: FxHashMap<(WorktreeId, Arc<RelPath>), DiagnosticCount> = FxHashMap::default();
        for (project_path, _, summary) in self.project.read(cx).diagnostic_summaries(false, cx) {
            let count =
                diagnostic_counts.entry((project_path.worktree_id, Arc::clone(&project_path.path))).or_default();
            count.error_count += summary.error_count;
            count.warning_count += summary.warning_count;
            if let Some(severity) = strongest_diagnostic_severity(summary.error_count, summary.warning_count) {
                for path in project_path.path.ancestors() {
                    Self::update_strongest_diagnostic_severity(&mut diagnostics, &project_path, path.into(), severity);
                }
            }
        }
        let changed = self.diagnostics != diagnostics || self.diagnostic_counts != diagnostic_counts;
        self.diagnostics = diagnostics;
        self.diagnostic_counts = diagnostic_counts;
        changed
    }

    fn update_strongest_diagnostic_severity(
        diagnostics: &mut FxHashMap<(WorktreeId, Arc<RelPath>), DiagnosticSeverity>,
        project_path: &ProjectPath,
        path_buffer: Arc<RelPath>,
        diagnostic_severity: DiagnosticSeverity,
    ) {
        diagnostics
            .entry((project_path.worktree_id, path_buffer))
            .and_modify(|strongest_diagnostic_severity| {
                *strongest_diagnostic_severity = cmp::min(*strongest_diagnostic_severity, diagnostic_severity);
            })
            .or_insert(diagnostic_severity);
    }

    /// Whether keyboard focus is within the panel, including a deployed context menu.
    ///
    /// `contains_focused` answers from the last rendered frame's element tree. On the frame a
    /// context menu is deployed, its focus node is not part of that tree yet, so the menu has
    /// to be checked directly against the window's current focus.
    ///
    /// TODO: We really really should fix context menu focus to not have to deal with
    /// this everywhere as this is not a proper fix here but a mere workaround. Yet
    /// a fix would require some more time and thought to not break other users of this.
    fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
            || self
                .context_menu
                .as_ref()
                .is_some_and(|context_menu| context_menu.menu.focus_handle(cx).is_focused(window))
    }

    fn deploy_context_menu(
        &mut self,
        placement: ContextMenuPlacement,
        entry_id: ProjectEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.read(cx);

        let Some(worktree_id) = project.worktree_id_for_entry(entry_id, cx) else {
            return;
        };

        self.selection = Some(SelectedEntry { worktree_id, entry_id });

        if let Some((worktree, entry)) = self.selected_sub_entry(cx) {
            let worktree = worktree.read(cx);
            let is_root = Some(entry) == worktree.root_entry();
            let is_dir = entry.is_dir();
            let is_foldable = self.is_foldable(entry, worktree);
            let is_unfoldable = self.is_unfoldable(entry, worktree);
            let is_library = worktree.library_name().is_some();
            let file_name = Path::new(entry_file_name(entry, worktree.root_name()));
            let is_previewable = !is_dir && MarkdownPreviewView::is_markdown_path(file_name, project.languages());
            let is_svg = !is_dir && SvgPreviewView::is_svg_path(file_name);

            let should_show_compare = !is_dir && self.file_abs_paths_to_diff(cx).is_some();

            let (has_git_repo, has_history) = {
                let project_path = project::ProjectPath { worktree_id, path: Arc::clone(&entry.path) };
                let git_store = project.git_store().read(cx);
                let has_git_repo = git_store.repository_and_path_for_project_path(&project_path, cx).is_some();
                let has_history = has_git_repo
                    && !git_store.project_path_git_status(&project_path, cx).is_some_and(|x| x.is_created());
                (has_git_repo, has_history)
            };

            let has_pasteable_content = self.has_pasteable_content(cx);
            let context_menu = ContextMenu::build(window, cx, |menu, _, _| {
                menu.context(self.focus_handle.clone()).map(|menu| {
                    if is_library {
                        menu.action(ui_utils::reveal_in_file_manager_label(), Box::new(RevealInFileManager))
                            .action("Open in Terminal", Box::new(OpenInTerminal))
                            .when(is_previewable, |menu| menu.action("Open Preview", Box::new(OpenMarkdownPreview)))
                            .when(is_svg, |menu| menu.action("Open SVG Preview", Box::new(OpenSvgPreview)))
                            .when(is_unfoldable, |menu| menu.action("Unfold Directory", Box::new(UnfoldDirectory)))
                            .when(is_foldable, |menu| menu.action("Fold Directory", Box::new(FoldDirectory)))
                            .separator()
                            .action("Copy", Box::new(Copy))
                            .action("Copy Path", Box::new(CopyPath))
                            .action("Copy Relative Path", Box::new(CopyRelativePath))
                            .when(is_dir && !is_root, |menu| {
                                menu.separator()
                                    .action("Expand All", Box::new(ExpandSelectedEntryAndChildren))
                                    .action("Collapse All", Box::new(CollapseSelectedEntryAndChildren))
                            })
                            .when(is_root, |menu| {
                                menu.separator().action("Remove Library", Box::new(RemoveFromProject))
                            })
                    } else {
                        menu.action("New File", Box::new(NewFile))
                            .action("New Folder", Box::new(NewDirectory))
                            .separator()
                            .action(ui_utils::reveal_in_file_manager_label(), Box::new(RevealInFileManager))
                            .action("Open in Default App", Box::new(OpenWithSystem))
                            .action("Open in Terminal", Box::new(OpenInTerminal))
                            .when(is_previewable, |menu| menu.action("Open Preview", Box::new(OpenMarkdownPreview)))
                            .when(is_svg, |menu| menu.action("Open SVG Preview", Box::new(OpenSvgPreview)))
                            .when(is_dir, |menu| {
                                menu.separator().action("Find in Folder…", Box::new(NewSearchInDirectory))
                            })
                            .when(is_unfoldable, |menu| menu.action("Unfold Directory", Box::new(UnfoldDirectory)))
                            .when(is_foldable, |menu| menu.action("Fold Directory", Box::new(FoldDirectory)))
                            .when(should_show_compare, |menu| {
                                menu.separator().action("Compare Marked Files", Box::new(CompareMarkedFiles))
                            })
                            .separator()
                            .action("Cut", Box::new(Cut))
                            .action("Copy", Box::new(Copy))
                            .action("Duplicate", Box::new(Duplicate))
                            .action_disabled_when(!has_pasteable_content, "Paste", Box::new(Paste))
                            .map(|menu| {
                                let can_undo = self.undo_manager.can_undo();
                                let can_redo = self.undo_manager.can_redo();

                                menu.action_disabled_when(!can_undo, "Undo", Box::new(Undo)).action_disabled_when(
                                    !can_redo,
                                    "Redo",
                                    Box::new(Redo),
                                )
                            })
                            .separator()
                            .action("Copy Path", Box::new(CopyPath))
                            .action("Copy Relative Path", Box::new(CopyRelativePath))
                            .when(has_git_repo, |menu| {
                                menu.separator()
                                    .when(!is_dir && self.has_git_changes(entry_id), |menu| {
                                        menu.action("Restore File", Box::new(git::RestoreFile { skip_prompt: false }))
                                    })
                                    .action("Add to .gitignore", Box::new(git::AddToGitignore))
                                    .action("Add to .git/info/exclude", Box::new(git::AddToGitInfoExclude))
                                    .when(has_history, |menu| menu.action("View History", Box::new(git::FileHistory)))
                            })
                            .separator()
                            .action("Rename", Box::new(Rename))
                            .when(!is_root, |menu| menu.action("Delete", Box::new(Delete { skip_prompt: false })))
                            .when(is_root, |menu| {
                                menu.separator()
                                    .action("Add Folders to Project…", Box::new(workspace::AddFolderToProject))
                                    .action("Remove from Project", Box::new(RemoveFromProject))
                            })
                            .when(is_dir && !is_root, |menu| {
                                menu.separator()
                                    .action("Expand All", Box::new(ExpandSelectedEntryAndChildren))
                                    .action("Collapse All", Box::new(CollapseSelectedEntryAndChildren))
                            })
                            .when(is_dir && is_root, |menu| {
                                menu.separator()
                                    .action("Expand All", Box::new(ExpandAllEntries))
                                    .action("Collapse All", Box::new(CollapseAllEntries))
                            })
                    }
                })
            });

            let (position, anchor) = match placement {
                ContextMenuPlacement::AtMouse(mouse_position) => (mouse_position, None),
                ContextMenuPlacement::AtEntry => match self.scroll_selection_into_view() {
                    Some(entry_bounds) => {
                        let (position, anchor) = context_menu_placement(entry_bounds, window.viewport_size());

                        let shifted_position = position
                            + if anchor.is_bottom() {
                                -CONTEXT_MENU_KEYBOARD_OFFSET
                            } else {
                                CONTEXT_MENU_KEYBOARD_OFFSET
                            };

                        (shifted_position, Some(anchor))
                    }
                    None => (window.mouse_position(), None),
                },
            };

            window.focus(&context_menu.focus_handle(cx), cx);
            let subscription = cx.subscribe(&context_menu, |this, _, _: &DismissEvent, cx| {
                this.context_menu.take();
                cx.notify();
            });
            self.context_menu =
                Some(DeployedContextMenu { menu: context_menu, position, anchor, _subscription: subscription });
        }

        cx.notify();
    }

    fn open_context_menu(&mut self, _: &OpenContextMenu, window: &mut Window, cx: &mut Context<Self>) {
        let entry_id = if let Some(selection) = &self.selection {
            selection.entry_id
        } else if let Some(entry_id) = self.state.last_worktree_root_id {
            entry_id
        } else {
            return;
        };

        self.deploy_context_menu(ContextMenuPlacement::AtEntry, entry_id, window, cx);
    }

    /// The height of a single row in the entries list, as measured by the list during its last layout.
    fn entry_row_height(&self) -> Option<Pixels> {
        let item_count: usize = self.state.visible_entries.iter().map(|worktree| worktree.entries.len()).sum();
        if item_count == 0 {
            return None;
        }
        let last_item_size = self.scroll_handle.0.borrow().last_item_size?;
        let row_height = last_item_size.contents.height / item_count as f32;
        (row_height > Pixels::ZERO).then_some(row_height)
    }

    /// Scrolls the entries list the minimal amount needed for the selected entry to be fully
    /// visible, and returns the bounds of the entry's row in window coordinates.
    fn scroll_selection_into_view(&self) -> Option<Bounds<Pixels>> {
        let selection = self.selection?;
        let (_, _, index) = self.index_for_selection(selection)?;
        let row_height = self.entry_row_height()?;

        let viewport = self.scroll_handle.viewport();
        let current_offset = self.scroll_handle.offset();
        let row_top = row_height * index;
        let row_bottom = row_top + row_height;
        // Sticky entries are drawn over the top rows of the list, so a row underneath them is not
        // actually visible.
        let visible_top = row_height * self.sticky_items_count;

        let mut offset = current_offset;
        if row_top + offset.y < visible_top {
            offset.y = visible_top - row_top;
        } else if row_bottom + offset.y > viewport.size.height {
            offset.y = viewport.size.height - row_bottom;
        }
        offset.y = offset.y.clamp(-self.scroll_handle.max_offset().y, Pixels::ZERO);
        if offset != current_offset {
            self.scroll_handle.set_offset(offset);
        }

        Some(Bounds::new(
            point(viewport.left(), viewport.top() + offset.y + row_top),
            size(viewport.size.width, row_height),
        ))
    }

    fn has_git_changes(&self, entry_id: ProjectEntryId) -> bool {
        for visible in &self.state.visible_entries {
            if let Some(git_entry) = visible.entries.iter().find(|e| e.id == entry_id) {
                let total_modified = git_entry.git_summary.index.modified + git_entry.git_summary.worktree.modified;
                let total_deleted = git_entry.git_summary.index.deleted + git_entry.git_summary.worktree.deleted;
                return total_modified > 0 || total_deleted > 0;
            }
        }
        false
    }

    fn is_unfoldable(&self, entry: &Entry, worktree: &Worktree) -> bool {
        if !entry.is_dir() || self.state.unfolded_dir_ids.contains(&entry.id) {
            return false;
        }

        if let Some(parent_path) = entry.path.parent() {
            let snapshot = worktree.snapshot();
            let mut child_entries = snapshot.child_entries(parent_path);
            if let Some(child) = child_entries.next()
                && child_entries.next().is_none()
            {
                return child.kind.is_dir();
            }
        }
        false
    }

    fn is_foldable(&self, entry: &Entry, worktree: &Worktree) -> bool {
        if entry.is_dir() {
            let snapshot = worktree.snapshot();

            let mut child_entries = snapshot.child_entries(&entry.path);
            if let Some(child) = child_entries.next()
                && child_entries.next().is_none()
            {
                return child.kind.is_dir();
            }
        }
        false
    }

    fn expand_selected_entry(&mut self, _: &ExpandSelectedEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_libraries_header_selected() {
            if self.libraries_expanded {
                self.select_next(&SelectNext, window, cx);
            } else {
                self.set_libraries_expanded(true, window, cx);
            }
            return;
        }
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            if let Some(folded_ancestors) = self.state.ancestors.get_mut(&entry.id)
                && folded_ancestors.current_ancestor_depth > 0
            {
                folded_ancestors.current_ancestor_depth -= 1;
                cx.notify();
                return;
            }
            if entry.is_dir() {
                let worktree_id = worktree.id();
                let entry_id = entry.id;
                let Some(expanded_dir_ids) = self.state.expanded_dir_ids.get_mut(&worktree_id) else {
                    return;
                };

                match expanded_dir_ids.binary_search(&entry_id) {
                    Ok(_) => self.select_next(&SelectNext, window, cx),
                    Err(ix) => {
                        self.project.update(cx, |project, cx| {
                            project.expand_entry(worktree_id, entry_id, cx);
                        });

                        expanded_dir_ids.insert(ix, entry_id);
                        self.update_visible_entries(None, false, false, window, cx);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn collapse_selected_entry(&mut self, _: &CollapseSelectedEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_libraries_header_selected() {
            self.set_libraries_expanded(false, window, cx);
            return;
        }
        let Some((worktree, entry)) = self.selected_entry_handle(cx) else {
            return;
        };
        self.collapse_entry(entry.clone(), worktree, window, cx);
    }

    fn collapse_entry(&mut self, entry: Entry, worktree: Entity<Worktree>, window: &Window, cx: &mut Context<Self>) {
        let worktree = worktree.read(cx);
        if let Some(folded_ancestors) = self.state.ancestors.get_mut(&entry.id)
            && folded_ancestors.current_ancestor_depth + 1 < folded_ancestors.max_ancestor_depth()
        {
            folded_ancestors.current_ancestor_depth += 1;
            cx.notify();
            return;
        }
        let worktree_id = worktree.id();
        let Some(expanded_dir_ids) = self.state.expanded_dir_ids.get_mut(&worktree_id) else {
            return;
        };

        let mut entry = &entry;
        loop {
            let entry_id = entry.id;
            match expanded_dir_ids.binary_search(&entry_id) {
                Ok(ix) if entry.is_dir() => {
                    expanded_dir_ids.remove(ix);
                    self.selection = Some(SelectedEntry { worktree_id, entry_id });
                    self.update_visible_entries(Some((worktree_id, entry_id)), false, true, window, cx);
                    cx.notify();
                    break;
                }
                Ok(_) | Err(_) => {
                    if let Some(parent_entry) = entry.path.parent().and_then(|p| worktree.entry_for_path(p)) {
                        entry = parent_entry;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    fn collapse_selected_entry_and_children(
        &mut self,
        _: &CollapseSelectedEntryAndChildren,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            let worktree_id = worktree.id();
            let entry_id = entry.id;

            self.collapse_all_for_entry(worktree_id, entry_id, cx);

            self.update_visible_entries(Some((worktree_id, entry_id)), false, false, window, cx);
            cx.notify();
        }
    }

    fn collapse_worktree_expanded_dirs(&mut self, worktree_id: WorktreeId, root_id: ProjectEntryId, cx: &App) {
        let single_worktree = self.project.read(cx).visible_worktrees(cx).count() == 1;
        if let Some(expanded_dir_ids) = self.state.expanded_dir_ids.get_mut(&worktree_id) {
            if single_worktree {
                expanded_dir_ids.retain(|id| id == &root_id);
            } else {
                expanded_dir_ids.clear();
            }
        }
    }

    fn all_worktree_roots(&self, cx: &App) -> Vec<(WorktreeId, ProjectEntryId)> {
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .filter_map(|worktree| {
                let worktree = worktree.read(cx);
                Some((worktree.id(), worktree.root_entry()?.id))
            })
            .collect()
    }

    fn expand_worktree_roots(
        &mut self,
        roots: Vec<(WorktreeId, ProjectEntryId)>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        for (worktree_id, root_id) in roots {
            self.expand_all_for_entry(worktree_id, root_id, cx);
            self.synchronously_expand_all_directories_internal(worktree_id, root_id, cx);
        }

        self.update_visible_entries(None, false, false, window, cx);
        cx.notify();
    }

    fn collapse_worktree_roots(
        &mut self,
        roots: Vec<(WorktreeId, ProjectEntryId)>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        for (worktree_id, root_id) in roots {
            self.collapse_worktree_expanded_dirs(worktree_id, root_id, cx);
        }

        self.update_visible_entries(None, false, false, window, cx);
        cx.notify();
    }

    fn collapse_all_entries(&mut self, _: &CollapseAllEntries, window: &mut Window, cx: &mut Context<Self>) {
        let roots = self.all_worktree_roots(cx);
        self.collapse_worktree_roots(roots, window, cx);
    }

    fn expand_all_entries(&mut self, _: &ExpandAllEntries, window: &mut Window, cx: &mut Context<Self>) {
        let roots = self.all_worktree_roots(cx);
        self.expand_worktree_roots(roots, window, cx);
    }

    fn expand_all_for_entry_and_refresh(
        &mut self,
        worktree_id: WorktreeId,
        entry_id: ProjectEntryId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.expand_all_for_entry(worktree_id, entry_id, cx);
        self.synchronously_expand_all_directories(worktree_id, entry_id, window, cx);
    }

    fn expand_selected_entry_and_children(
        &mut self,
        _: &ExpandSelectedEntryAndChildren,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            let worktree_id = worktree.id();
            let entry_id = entry.id;
            self.expand_all_for_entry_and_refresh(worktree_id, entry_id, window, cx);
        }
    }

    fn synchronously_expand_all_directories(
        &mut self,
        worktree_id: WorktreeId,
        entry_id: ProjectEntryId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.synchronously_expand_all_directories_internal(worktree_id, entry_id, cx);
        self.update_visible_entries(None, false, false, window, cx);
        cx.notify();
    }

    fn synchronously_expand_all_directories_internal(
        &mut self,
        worktree_id: WorktreeId,
        entry_id: ProjectEntryId,
        cx: &Context<Self>,
    ) {
        let project = self.project.read(cx);
        let Some((worktree, expanded_dir_ids)) =
            project.worktree_for_id(worktree_id, cx).zip(self.state.expanded_dir_ids.get_mut(&worktree_id))
        else {
            return;
        };

        let worktree = worktree.read(cx);
        let Some(entry) = worktree.entry_for_id(entry_id) else {
            return;
        };
        let include_ignored_dirs = !entry.is_ignored;

        if entry.is_dir()
            && let Err(ix) = expanded_dir_ids.binary_search(&entry_id)
        {
            expanded_dir_ids.insert(ix, entry_id);
        }

        let mut dirs_to_expand = vec![entry_id];
        while let Some(current_id) = dirs_to_expand.pop() {
            let Some(current_entry) = worktree.entry_for_id(current_id) else {
                continue;
            };
            for child in worktree.child_entries(&current_entry.path) {
                if !child.is_dir() || (include_ignored_dirs && child.is_ignored) {
                    continue;
                }

                dirs_to_expand.push(child.id);

                if let Err(ix) = expanded_dir_ids.binary_search(&child.id) {
                    expanded_dir_ids.insert(ix, child.id);
                }
                self.state.unfolded_dir_ids.insert(child.id);
            }
        }
    }

    fn is_libraries_header_selected(&self) -> bool {
        self.selection.is_some_and(|selection| selection.entry_id == LIBRARIES_ENTRY_ID)
    }

    fn set_libraries_expanded(&mut self, expanded: bool, window: &Window, cx: &mut Context<Self>) {
        self.libraries_expanded = expanded;
        self.update_visible_entries(Some((LIBRARIES_WORKTREE_ID, LIBRARIES_ENTRY_ID)), false, false, window, cx);
        cx.notify();
    }

    fn libraries_header_details(&self) -> EntryDetails {
        let entry = libraries_header_entry();
        let selection = SelectedEntry { worktree_id: LIBRARIES_WORKTREE_ID, entry_id: LIBRARIES_ENTRY_ID };
        EntryDetails {
            filename: "External Libraries".to_owned(),
            icon: Some(IconName::Library.path().into()),
            path: entry.entry.path,
            depth: 0,
            kind: EntryKind::Dir,
            is_ignored: false,
            is_expanded: self.libraries_expanded,
            is_selected: self.selection == Some(selection),
            is_marked: false,
            is_editing: false,
            is_processing: false,
            is_cut: false,
            sticky: None,
            filename_text_color: Color::Default,
            diagnostic_severity: None,
            diagnostic_mark: None,
            diagnostic_count: None,
            git_status: GitSummary::default(),
            worktree_id: LIBRARIES_WORKTREE_ID,
            canonical_path: None,
        }
    }

    fn toggle_expanded(&mut self, entry_id: ProjectEntryId, window: &mut Window, cx: &mut Context<Self>) {
        if entry_id == LIBRARIES_ENTRY_ID {
            self.set_libraries_expanded(!self.libraries_expanded, window, cx);
            window.focus(&self.focus_handle, cx);
            return;
        }
        if let Some(worktree_id) = self.project.read(cx).worktree_id_for_entry(entry_id, cx)
            && let Some(expanded_dir_ids) = self.state.expanded_dir_ids.get_mut(&worktree_id)
        {
            self.project.update(cx, |project, cx| match expanded_dir_ids.binary_search(&entry_id) {
                Ok(ix) => {
                    expanded_dir_ids.remove(ix);
                }
                Err(ix) => {
                    project.expand_entry(worktree_id, entry_id, cx);
                    expanded_dir_ids.insert(ix, entry_id);
                }
            });
            self.update_visible_entries(Some((worktree_id, entry_id)), false, false, window, cx);
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    fn toggle_expand_all(&mut self, entry_id: ProjectEntryId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(worktree_id) = self.project.read(cx).worktree_id_for_entry(entry_id, cx)
            && let Some(expanded_dir_ids) = self.state.expanded_dir_ids.get_mut(&worktree_id)
        {
            match expanded_dir_ids.binary_search(&entry_id) {
                Ok(_ix) => {
                    self.collapse_all_for_entry(worktree_id, entry_id, cx);
                }
                Err(_ix) => {
                    self.expand_all_for_entry(worktree_id, entry_id, cx);
                }
            }
            self.update_visible_entries(Some((worktree_id, entry_id)), false, false, window, cx);
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    fn expand_all_for_entry(&mut self, worktree_id: WorktreeId, entry_id: ProjectEntryId, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            if let Some((worktree, expanded_dir_ids)) =
                project.worktree_for_id(worktree_id, cx).zip(self.state.expanded_dir_ids.get_mut(&worktree_id))
            {
                if let Some(task) = project.expand_all_for_entry(worktree_id, entry_id, cx) {
                    task.detach();
                }

                let worktree = worktree.read(cx);

                if let Some(mut entry) = worktree.entry_for_id(entry_id) {
                    loop {
                        if entry.is_dir()
                            && let Err(ix) = expanded_dir_ids.binary_search(&entry.id)
                        {
                            expanded_dir_ids.insert(ix, entry.id);
                        }

                        if let Some(parent_entry) = entry.path.parent().and_then(|p| worktree.entry_for_path(p)) {
                            entry = parent_entry;
                        } else {
                            break;
                        }
                    }
                }
            }
        });
    }

    fn collapse_all_for_entry(&mut self, worktree_id: WorktreeId, entry_id: ProjectEntryId, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            if let Some((worktree, expanded_dir_ids)) =
                project.worktree_for_id(worktree_id, cx).zip(self.state.expanded_dir_ids.get_mut(&worktree_id))
            {
                let worktree = worktree.read(cx);
                let mut dirs_to_collapse = vec![entry_id];
                while let Some(current_id) = dirs_to_collapse.pop() {
                    let Some(current_entry) = worktree.entry_for_id(current_id) else {
                        continue;
                    };
                    if let Ok(ix) = expanded_dir_ids.binary_search(&current_id) {
                        expanded_dir_ids.remove(ix);
                    }
                    self.state.unfolded_dir_ids.remove(&current_id);
                    for child in worktree.child_entries(&current_entry.path) {
                        if child.is_dir() {
                            dirs_to_collapse.push(child.id);
                        }
                    }
                }
            }
        });
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit_state) = &self.state.edit_state
            && edit_state.processing_filename.is_none()
        {
            self.filename_editor.update(cx, |editor, cx| {
                editor.move_to_beginning_of_line(
                    &MoveToBeginningOfLine { stop_at_soft_wraps: false, stop_at_indent: false },
                    window,
                    cx,
                );
            });
            return;
        }
        if let Some(previous_selection) = self.selection {
            let (mut worktree_ix, mut entry_ix, _) = self.index_for_selection(previous_selection).unwrap_or_default();
            if entry_ix > 0 {
                entry_ix -= 1;
            } else if worktree_ix > 0 {
                worktree_ix -= 1;
                entry_ix = self.state.visible_entries[worktree_ix].entries.len() - 1;
            } else {
                return;
            }

            let VisibleEntriesForWorktree { worktree_id, entries, .. } = &self.state.visible_entries[worktree_ix];
            let selection = SelectedEntry { worktree_id: *worktree_id, entry_id: entries[entry_ix].id };
            self.selection = Some(selection);
            if window.modifiers().shift {
                self.extend_marked_entries(previous_selection, selection);
            }
            self.autoscroll(cx);
            cx.notify();
        } else {
            self.select_first(&SelectFirst {}, window, cx);
        }
    }

    fn extend_marked_entries(&mut self, previous_selection: SelectedEntry, selection: SelectedEntry) {
        // Mark the entry the selection started from too, so that extending a selection
        // with the keyboard includes it, the same as a shift-click range does.
        if !self.marked_entries.contains(&previous_selection) {
            self.marked_entries.push(previous_selection);
        }
        if !self.marked_entries.contains(&selection) {
            self.marked_entries.push(selection);
        }
        self.select_outermost_folded_ancestor(selection.entry_id);
    }

    // A vertical selection gesture says nothing about which segment of a folded
    // directory is meant, so select the whole folded directory.
    fn select_outermost_folded_ancestor(&mut self, entry_id: ProjectEntryId) {
        if let Some(folds) = self.state.ancestors.get_mut(&entry_id) {
            folds.set_active_index(0);
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.confirm_edit(true, window, cx) {
            task.detach_and_notify_err(self.workspace.clone(), window, cx);
        }
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.open_internal(true, false, None, window, cx);
    }

    fn open_permanent(&mut self, _: &OpenPermanent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_internal(false, true, None, window, cx);
    }

    fn open_split_vertical(&mut self, _: &OpenSplitVertical, window: &mut Window, cx: &mut Context<Self>) {
        self.open_internal(false, true, Some(SplitDirection::Right), window, cx);
    }

    fn open_split_horizontal(&mut self, _: &OpenSplitHorizontal, window: &mut Window, cx: &mut Context<Self>) {
        self.open_internal(false, true, Some(SplitDirection::Down), window, cx);
    }

    fn open_markdown_preview(&mut self, _: &OpenMarkdownPreview, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project_path, file_name)) = self.selected_file(cx) else {
            return;
        };
        if !MarkdownPreviewView::is_markdown_path(Path::new(file_name), self.project.read(cx).languages()) {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| {
                MarkdownPreviewView::open_for_project_path(project_path, workspace, window, cx);
            })
            .ok();
    }

    fn open_svg_preview(&mut self, _: &OpenSvgPreview, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project_path, file_name)) = self.selected_file(cx) else {
            return;
        };
        if !SvgPreviewView::is_svg_path(Path::new(file_name)) {
            return;
        }
        self.workspace
            .update(cx, |workspace, cx| SvgPreviewView::open_for_project_path(project_path, workspace, window, cx))
            .ok();
    }

    fn open_internal(
        &mut self,
        allow_preview: bool,
        focus_opened_item: bool,
        split_direction: Option<SplitDirection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_libraries_header_selected() {
            self.set_libraries_expanded(!self.libraries_expanded, window, cx);
            return;
        }
        if let Some((_, entry)) = self.selected_entry(cx) {
            if entry.is_file() {
                if split_direction.is_some() {
                    self.split_entry(entry.id, allow_preview, split_direction, cx);
                } else {
                    self.open_entry(entry.id, focus_opened_item, allow_preview, cx);
                }
                cx.notify();
            } else {
                self.toggle_expanded(entry.id, window, cx);
            }
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if cx.stop_active_drag(window) {
            self.clear_drag_state(cx);
            return;
        }
        self.marked_entries.clear();
        cx.notify();
        self.discard_edit_state(window, cx);
        window.focus(&self.focus_handle, cx);
    }

    fn open_entry(
        &self,
        entry_id: ProjectEntryId,
        focus_opened_item: bool,
        allow_preview: bool,

        cx: &mut Context<Self>,
    ) {
        cx.emit(Event::OpenedEntry { entry_id, focus_opened_item, allow_preview });
    }

    fn split_entry(
        &self,
        entry_id: ProjectEntryId,
        allow_preview: bool,
        split_direction: Option<SplitDirection>,

        cx: &mut Context<Self>,
    ) {
        cx.emit(Event::SplitEntry { entry_id, allow_preview, split_direction });
    }

    fn unflatten_entry_id(&self, leaf_entry_id: ProjectEntryId) -> ProjectEntryId {
        if let Some(ancestors) = self.state.ancestors.get(&leaf_entry_id) {
            ancestors.ancestors.get(ancestors.current_ancestor_depth).copied().unwrap_or(leaf_entry_id)
        } else {
            leaf_entry_id
        }
    }

    pub fn undo(&mut self, _: &Undo, _window: &mut Window, _cx: &mut Context<Self>) {
        self.undo_manager.undo().log_err();
    }

    pub fn redo(&mut self, _: &Redo, _window: &mut Window, _cx: &mut Context<Self>) {
        self.undo_manager.redo().log_err();
    }

    fn restore_file(&mut self, action: &git::RestoreFile, window: &mut Window, cx: &mut Context<Self>) {
        (|| {
            let selection = self.selection?;
            let project = self.project.read(cx);

            let (_worktree, entry) = self.selected_sub_entry(cx)?;
            if entry.is_dir() {
                return None;
            }

            let project_path = project.path_for_entry(selection.entry_id, cx)?;

            let git_store = project.git_store();
            let (repository, repo_path) = git_store.read(cx).repository_and_path_for_project_path(&project_path, cx)?;

            let snapshot = repository.read(cx).snapshot();
            let status = snapshot.status_for_path(&repo_path)?;
            if !status.status.is_modified() && !status.status.is_deleted() {
                return None;
            }

            let file_name = entry.path.file_name()?.to_owned();

            let answer = if action.skip_prompt {
                None
            } else {
                let prompt = format!("Discard changes to {}?", MarkdownInlineCode(&file_name));
                Some(window.prompt(PromptLevel::Info, &prompt, None, &["Restore", "Cancel"], cx))
            };

            cx.spawn_in(window, async move |panel, cx| {
                if let Some(answer) = answer
                    && answer.await != Ok(0)
                {
                    return anyhow::Ok(());
                }

                let task = panel.update(cx, |_panel, cx| {
                    repository.update(cx, |repo, cx| repo.checkout_files("HEAD", vec![repo_path], cx))
                })?;

                if let Err(e) = task.await {
                    panel
                        .update(cx, |panel, cx| {
                            let message = format!("Failed to restore {file_name}: {e}");
                            let toast = StatusToast::new(message, cx, |this, _| {
                                this.icon(Icon::new(IconName::XCircle).size(IconSize::Small).color(Color::Error))
                                    .dismiss_button(true)
                            });
                            panel
                                .workspace
                                .update(cx, |workspace, cx| {
                                    workspace.toggle_status_toast(toast, cx);
                                })
                                .ok();
                        })
                        .ok();
                }

                panel
                    .update(cx, |panel, cx| {
                        panel.project.update(cx, |project, cx| {
                            if let Some(buffer_id) =
                                project.buffer_store().read(cx).buffer_id_for_project_path(&project_path)
                                && let Some(buffer) = project.buffer_for_id(*buffer_id, cx)
                            {
                                buffer.update(cx, |buffer, cx| {
                                    drop(buffer.reload(cx));
                                });
                            }
                        });
                    })
                    .ok();

                anyhow::Ok(())
            })
            .detach_and_log_err(cx);

            Some(())
        })();
    }

    fn add_to_gitignore(&mut self, _: &git::AddToGitignore, _window: &mut Window, cx: &mut Context<Self>) {
        (|| {
            let selection = self.selection?;
            let (_, entry) = self.selected_sub_entry(cx)?;
            let is_dir = entry.is_dir();
            let project = self.project.read(cx);

            let project_path = project.path_for_entry(selection.entry_id, cx)?;

            let git_store = project.git_store();
            let (repository, repo_path) = git_store.read(cx).repository_and_path_for_project_path(&project_path, cx)?;

            let workspace = self.workspace.clone();
            let receiver = repository.update(cx, |repo, _| repo.add_path_to_gitignore(&repo_path, is_dir));

            cx.spawn(async move |_, cx| {
                if let Err(e) = receiver.await?
                    && let Some(workspace) = workspace.upgrade()
                {
                    cx.update(|cx| {
                        let message = format!("Failed to add to .gitignore: {e}");
                        let toast = StatusToast::new(message, cx, |this, _| {
                            this.icon(Icon::new(IconName::XCircle).color(Color::Error)).dismiss_button(true)
                        });
                        workspace.update(cx, |workspace, cx| {
                            workspace.toggle_status_toast(toast, cx);
                        });
                    });
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);

            Some(())
        })();
    }

    fn add_to_git_info_exclude(&mut self, _: &git::AddToGitInfoExclude, _window: &mut Window, cx: &mut Context<Self>) {
        (|| {
            let selection = self.selection?;
            let (_, entry) = self.selected_sub_entry(cx)?;
            let is_dir = entry.is_dir();
            let project = self.project.read(cx);

            let project_path = project.path_for_entry(selection.entry_id, cx)?;

            let git_store = project.git_store();
            let (repository, repo_path) = git_store.read(cx).repository_and_path_for_project_path(&project_path, cx)?;

            let workspace = self.workspace.clone();
            let receiver = repository.update(cx, |repo, _| repo.add_path_to_git_info_exclude(&repo_path, is_dir));

            cx.spawn(async move |_, cx| {
                if let Err(e) = receiver.await?
                    && let Some(workspace) = workspace.upgrade()
                {
                    cx.update(|cx| {
                        let message = format!("Failed to add to .git/info/exclude: {e}");
                        let toast = StatusToast::new(message, cx, |this, _| {
                            this.icon(Icon::new(IconName::XCircle).color(Color::Error)).dismiss_button(true)
                        });
                        workspace.update(cx, |workspace, cx| {
                            workspace.toggle_status_toast(toast, cx);
                        });
                    });
                }
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);

            Some(())
        })();
    }

    fn unfold_directory(&mut self, _: &UnfoldDirectory, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            self.state.unfolded_dir_ids.insert(entry.id);

            let snapshot = worktree.snapshot();
            let mut parent_path = entry.path.parent();
            while let Some(path) = parent_path {
                if let Some(parent_entry) = worktree.entry_for_path(path) {
                    let mut children_iter = snapshot.child_entries(path);

                    if children_iter.by_ref().take(2).count() > 1 {
                        break;
                    }

                    self.state.unfolded_dir_ids.insert(parent_entry.id);
                    parent_path = path.parent();
                } else {
                    break;
                }
            }

            self.update_visible_entries(None, false, true, window, cx);
            cx.notify();
        }
    }

    fn fold_directory(&mut self, _: &FoldDirectory, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            self.state.unfolded_dir_ids.remove(&entry.id);

            let snapshot = worktree.snapshot();
            let mut path = &*entry.path;
            loop {
                let mut child_entries_iter = snapshot.child_entries(path);
                if let Some(child) = child_entries_iter.next() {
                    if child_entries_iter.next().is_none() && child.is_dir() {
                        self.state.unfolded_dir_ids.remove(&child.id);
                        path = &*child.path;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            self.update_visible_entries(None, false, true, window, cx);
            cx.notify();
        }
    }

    fn scroll_up(&mut self, _: &ScrollUp, window: &mut Window, cx: &mut Context<Self>) {
        for _ in 0..self.rendered_entries_len / 2 {
            window.dispatch_action(SelectPrevious.boxed_clone(), cx);
        }
    }

    fn scroll_down(&mut self, _: &ScrollDown, window: &mut Window, cx: &mut Context<Self>) {
        for _ in 0..self.rendered_entries_len / 2 {
            window.dispatch_action(SelectNext.boxed_clone(), cx);
        }
    }

    fn scroll_cursor_center(&mut self, _: &ScrollCursorCenter, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, _, index)) = self.selection.and_then(|s| self.index_for_selection(s)) {
            self.scroll_handle.scroll_to_item_strict(index, ScrollStrategy::Center);
            cx.notify();
        }
    }

    fn scroll_cursor_top(&mut self, _: &ScrollCursorTop, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, _, index)) = self.selection.and_then(|s| self.index_for_selection(s)) {
            self.scroll_handle.scroll_to_item_strict(index, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn scroll_cursor_bottom(&mut self, _: &ScrollCursorBottom, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, _, index)) = self.selection.and_then(|s| self.index_for_selection(s)) {
            self.scroll_handle.scroll_to_item_strict(index, ScrollStrategy::Bottom);
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit_state) = &self.state.edit_state
            && edit_state.processing_filename.is_none()
        {
            self.filename_editor.update(cx, |editor, cx| {
                editor.move_to_end_of_line(&MoveToEndOfLine { stop_at_soft_wraps: false }, window, cx);
            });
            return;
        }
        if let Some(previous_selection) = self.selection {
            let (mut worktree_ix, mut entry_ix, _) = self.index_for_selection(previous_selection).unwrap_or_default();
            if let Some(worktree_entries) = self.state.visible_entries.get(worktree_ix).map(|v| &v.entries) {
                if entry_ix + 1 < worktree_entries.len() {
                    entry_ix += 1;
                } else {
                    worktree_ix += 1;
                    entry_ix = 0;
                }
            }

            if let Some(VisibleEntriesForWorktree { worktree_id, entries, .. }) =
                self.state.visible_entries.get(worktree_ix)
                && let Some(entry) = entries.get(entry_ix)
            {
                let selection = SelectedEntry { worktree_id: *worktree_id, entry_id: entry.id };
                self.selection = Some(selection);
                if window.modifiers().shift {
                    self.extend_marked_entries(previous_selection, selection);
                }

                self.autoscroll(cx);
                cx.notify();
            }
        } else {
            self.select_first(&SelectFirst {}, window, cx);
        }
    }

    fn select_prev_diagnostic(&mut self, action: &SelectPrevDiagnostic, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_entry(
            self.selection.as_ref(),
            true,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_file()
                    && self
                        .diagnostics
                        .get(&(worktree_id, Arc::clone(&entry.path)))
                        .is_some_and(|severity| action.severity.matches(*severity))
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.expand_entry(selection.worktree_id, selection.entry_id, cx);
            self.update_visible_entries(Some((selection.worktree_id, selection.entry_id)), false, true, window, cx);
            cx.notify();
        }
    }

    fn select_next_diagnostic(&mut self, action: &SelectNextDiagnostic, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_entry(
            self.selection.as_ref(),
            false,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_file()
                    && self
                        .diagnostics
                        .get(&(worktree_id, Arc::clone(&entry.path)))
                        .is_some_and(|severity| action.severity.matches(*severity))
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.expand_entry(selection.worktree_id, selection.entry_id, cx);
            self.update_visible_entries(Some((selection.worktree_id, selection.entry_id)), false, true, window, cx);
            cx.notify();
        }
    }

    fn select_prev_git_entry(&mut self, _: &SelectPrevGitEntry, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_entry(
            self.selection.as_ref(),
            true,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_file()
                    && entry.git_summary.index.modified + entry.git_summary.worktree.modified > 0
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.expand_entry(selection.worktree_id, selection.entry_id, cx);
            self.update_visible_entries(Some((selection.worktree_id, selection.entry_id)), false, true, window, cx);
            cx.notify();
        }
    }

    fn select_prev_directory(&mut self, _: &SelectPrevDirectory, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_visible_entry(
            self.selection.as_ref(),
            true,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_dir()
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.autoscroll(cx);
            cx.notify();
        }
    }

    fn select_next_directory(&mut self, _: &SelectNextDirectory, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_visible_entry(
            self.selection.as_ref(),
            false,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_dir()
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.autoscroll(cx);
            cx.notify();
        }
    }

    fn select_next_git_entry(&mut self, _: &SelectNextGitEntry, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.find_entry(
            self.selection.as_ref(),
            false,
            &|entry: GitEntryRef, worktree_id: WorktreeId| {
                self.selection.is_none_or(|selection| {
                    if selection.worktree_id == worktree_id { selection.entry_id != entry.id } else { true }
                }) && entry.is_file()
                    && entry.git_summary.index.modified + entry.git_summary.worktree.modified > 0
            },
            cx,
        );

        if let Some(selection) = selection {
            self.selection = Some(selection);
            self.expand_entry(selection.worktree_id, selection.entry_id, cx);
            self.update_visible_entries(Some((selection.worktree_id, selection.entry_id)), false, true, window, cx);
            cx.notify();
        }
    }

    fn select_parent(&mut self, _: &SelectParent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_sub_entry(cx) {
            if entry.path.is_empty() && worktree.read(cx).library_name().is_some() {
                self.selection =
                    Some(SelectedEntry { worktree_id: LIBRARIES_WORKTREE_ID, entry_id: LIBRARIES_ENTRY_ID });
                self.autoscroll(cx);
                cx.notify();
            } else if let Some(parent) = entry.path.parent() {
                let worktree = worktree.read(cx);
                if let Some(parent_entry) = worktree.entry_for_path(parent) {
                    self.selection = Some(SelectedEntry { worktree_id: worktree.id(), entry_id: parent_entry.id });
                    self.autoscroll(cx);
                    cx.notify();
                }
            }
        } else {
            self.select_first(&SelectFirst {}, window, cx);
        }
    }

    fn select_first(&mut self, _: &SelectFirst, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(VisibleEntriesForWorktree { worktree_id, entries, .. }) = self.state.visible_entries.first()
            && let Some(entry) = entries.first()
        {
            let selection = SelectedEntry { worktree_id: *worktree_id, entry_id: entry.id };
            self.selection = Some(selection);
            if window.modifiers().shift {
                self.marked_entries.push(selection);
            }
            self.autoscroll(cx);
            cx.notify();
        }
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(VisibleEntriesForWorktree { worktree_id, entries, .. }) = self.state.visible_entries.last() {
            let worktree = self.project.read(cx).worktree_for_id(*worktree_id, cx);
            if let (Some(worktree), Some(entry)) = (worktree, entries.last()) {
                let worktree = worktree.read(cx);
                if let Some(entry) = worktree.entry_for_id(entry.id) {
                    let selection = SelectedEntry { worktree_id: *worktree_id, entry_id: entry.id };
                    self.selection = Some(selection);
                    self.autoscroll(cx);
                    cx.notify();
                }
            }
        }
    }

    fn autoscroll(&self, cx: &mut Context<Self>) {
        if let Some((_, _, index)) = self.selection.and_then(|s| self.index_for_selection(s)) {
            self.scroll_handle.scroll_to_item_with_offset(index, ScrollStrategy::Center, self.sticky_items_count);
            cx.notify();
        }
    }

    fn copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let abs_file_paths = {
            let project = self.project.read(cx);
            self.effective_entries()
                .into_iter()
                .filter_map(|entry| {
                    let entry_path = project.path_for_entry(entry.entry_id, cx)?.path;
                    Some(
                        project
                            .worktree_for_id(entry.worktree_id, cx)?
                            .read(cx)
                            .absolutize(&entry_path)
                            .to_string_lossy()
                            .to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        if !abs_file_paths.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(abs_file_paths.join("\n")));
        }
    }

    fn copy_relative_path(&mut self, _: &CopyRelativePath, _: &mut Window, cx: &mut Context<Self>) {
        let file_paths = {
            let project = self.project.read(cx);
            self.effective_entries()
                .into_iter()
                .filter_map(|entry| Some(project.path_for_entry(entry.entry_id, cx)?.path.display().to_owned()))
                .collect::<Vec<_>>()
        };
        if !file_paths.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(file_paths.join("\n")));
        }
    }

    fn selected_file<'a>(&self, cx: &'a App) -> Option<(ProjectPath, &'a str)> {
        let (worktree, entry) = self.selected_sub_entry(cx)?;
        if entry.is_dir() {
            return None;
        }
        let worktree = worktree.read(cx);
        let project_path = ProjectPath { worktree_id: worktree.id(), path: Arc::clone(&entry.path) };
        Some((project_path, entry_file_name(entry, worktree.root_name())))
    }

    fn reveal_in_finder(&mut self, _: &RevealInFileManager, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.reveal_in_file_manager_path(cx) {
            self.project.update(cx, |project, cx| project.reveal_path(&path, cx));
        }
    }

    fn remove_from_project(&mut self, _: &RemoveFromProject, _window: &mut Window, cx: &mut Context<Self>) {
        for entry in &self.effective_entries() {
            let worktree_id = entry.worktree_id;
            self.project.update(cx, |project, cx| project.remove_worktree(worktree_id, cx));
        }
    }

    fn file_abs_paths_to_diff(&self, cx: &Context<Self>) -> Option<(PathBuf, PathBuf)> {
        let mut selections_abs_path = self
            .marked_entries
            .iter()
            .filter_map(|entry| {
                let project = self.project.read(cx);
                let worktree = project.worktree_for_id(entry.worktree_id, cx)?;
                let entry = worktree.read(cx).entry_for_id(entry.entry_id)?;
                if !entry.is_file() {
                    return None;
                }
                Some(worktree.read(cx).absolutize(&entry.path))
            })
            .rev();

        let last_path = selections_abs_path.next()?;
        let previous_to_last = selections_abs_path.next()?;
        Some((previous_to_last, last_path))
    }

    fn compare_marked_files(&mut self, _: &CompareMarkedFiles, window: &mut Window, cx: &mut Context<Self>) {
        let selected_files = self.file_abs_paths_to_diff(cx);
        if let Some((file_path1, file_path2)) = selected_files {
            self.workspace
                .update(cx, |workspace, cx| {
                    FileDiffView::open(file_path1, file_path2, None, workspace.weak_handle(), window, cx)
                        .detach_and_log_err(cx);
                })
                .ok();
        }
    }

    fn open_system(&mut self, _: &OpenWithSystem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_entry(cx) {
            let abs_path = worktree.absolutize(&entry.path);
            cx.open_with_system(&abs_path);
        }
    }

    fn open_in_terminal(&mut self, _: &OpenInTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_sub_entry(cx) {
            let abs_path = match &entry.canonical_path {
                Some(canonical_path) => canonical_path.to_path_buf(),
                None => worktree.read(cx).absolutize(&entry.path),
            };

            let working_directory =
                if entry.is_dir() { Some(abs_path) } else { abs_path.parent().map(|x| x.to_path_buf()) };
            if let Some(working_directory) = working_directory {
                window.dispatch_action(workspace::OpenTerminal { working_directory }.boxed_clone(), cx);
            }
        }
    }

    pub fn new_search_in_directory(&mut self, _: &NewSearchInDirectory, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((worktree, entry)) = self.selected_sub_entry(cx) {
            let dir_path = if entry.is_dir() {
                Arc::clone(&entry.path)
            } else {
                // entry is a file, use its parent directory
                if let Some(parent) = entry.path.parent() {
                    Arc::from(parent)
                } else {
                    // File at root, open search with empty filter
                    window.dispatch_action(Box::new(search_actions::NewSearchInDirectory::default()), cx);
                    return;
                }
            };

            let include_root = self.project.read(cx).visible_worktrees(cx).count() > 1;
            let dir_path =
                if include_root { worktree.read(cx).root_name().join(&dir_path) } else { dir_path.to_rel_path_buf() };

            let directory = dir_path.display().to_owned();
            window.dispatch_action(Box::new(search_actions::NewSearchInDirectory { directory }), cx);
        }
    }

    fn disjoint_effective_entries_excluding_roots(&self, cx: &App) -> BTreeSet<SelectedEntry> {
        let project = self.project.read(cx);
        let entries = self
            .effective_entries()
            .into_iter()
            .filter(|entry| !project.entry_is_worktree_root(entry.entry_id, cx))
            .collect();
        self.disjoint_entries(entries, cx)
    }

    fn disjoint_entries(&self, entries: BTreeSet<SelectedEntry>, cx: &App) -> BTreeSet<SelectedEntry> {
        let mut sanitized_entries = BTreeSet::new();
        if entries.is_empty() {
            return sanitized_entries;
        }

        let project = self.project.read(cx);
        let entries_by_worktree: FxHashMap<WorktreeId, Vec<SelectedEntry>> =
            entries.into_iter().fold(FxHashMap::default(), |mut map, entry| {
                map.entry(entry.worktree_id).or_default().push(entry);
                map
            });

        for (worktree_id, worktree_entries) in entries_by_worktree {
            if let Some(worktree) = project.worktree_for_id(worktree_id, cx) {
                let worktree = worktree.read(cx);
                let dir_paths = worktree_entries
                    .iter()
                    .filter_map(|entry| {
                        let entry = worktree.entry_for_id(entry.entry_id)?;
                        entry.is_dir().then(|| entry.path.as_ref())
                    })
                    .collect::<BTreeSet<_>>();

                sanitized_entries.extend(worktree_entries.into_iter().filter(|entry| {
                    let Some(entry_info) = worktree.entry_for_id(entry.entry_id) else {
                        return false;
                    };
                    let entry_path = entry_info.path.as_ref();
                    let inside_selected_dir =
                        dir_paths.iter().any(|&dir_path| entry_path != dir_path && entry_path.starts_with(dir_path));
                    !inside_selected_dir
                }));
            }
        }

        sanitized_entries
    }

    fn effective_entries(&self) -> BTreeSet<SelectedEntry> {
        if let Some(selection) = self.selection {
            let selection =
                SelectedEntry { entry_id: self.resolve_entry(selection.entry_id), worktree_id: selection.worktree_id };

            // Default to using just the selected item when nothing is marked.
            if self.marked_entries.is_empty() {
                return BTreeSet::from([selection]);
            }

            // Allow operating on the selected item even when something else is marked,
            // making it easier to perform one-off actions without clearing a mark.
            if self.marked_entries.len() == 1 && !self.marked_entries.contains(&selection) {
                return BTreeSet::from([selection]);
            }
        }

        // Return only marked entries since we've already handled special cases where
        // only selection should take precedence. At this point, marked entries may or
        // may not include the current selection, which is intentional.
        self.marked_entries
            .iter()
            .map(|entry| SelectedEntry { entry_id: self.resolve_entry(entry.entry_id), worktree_id: entry.worktree_id })
            .collect::<BTreeSet<_>>()
    }

    /// Finds the currently selected subentry for a given leaf entry id. If a given entry
    /// has no ancestors, the project entry ID that's passed in is returned as-is.
    fn resolve_entry(&self, id: ProjectEntryId) -> ProjectEntryId {
        self.state.ancestors.get(&id).and_then(FoldedAncestors::active_ancestor).unwrap_or(id)
    }

    pub fn selected_entry<'a>(&self, cx: &'a App) -> Option<(&'a Worktree, &'a project::Entry)> {
        let (worktree, entry) = self.selected_entry_handle(cx)?;
        Some((worktree.read(cx), entry))
    }

    pub fn selected_entry_project_path(&self, cx: &App) -> Option<ProjectPath> {
        let (worktree, entry) = self.selected_sub_entry(cx)?;
        Some(ProjectPath { worktree_id: worktree.read(cx).id(), path: Arc::clone(&entry.path) })
    }

    /// Compared to selected_entry, this function resolves to the currently
    /// selected subentry if dir auto-folding is enabled.
    fn selected_sub_entry<'a>(&self, cx: &'a App) -> Option<(Entity<Worktree>, &'a project::Entry)> {
        let (worktree, mut entry) = self.selected_entry_handle(cx)?;

        let resolved_id = self.resolve_entry(entry.id);
        if resolved_id != entry.id {
            let worktree = worktree.read(cx);
            entry = worktree.entry_for_id(resolved_id)?;
        }
        Some((worktree, entry))
    }

    fn reveal_in_file_manager_path(&self, cx: &App) -> Option<PathBuf> {
        if let Some((worktree, entry)) = self.selected_sub_entry(cx) {
            return Some(worktree.read(cx).absolutize(&entry.path));
        }

        let root_entry_id = self.state.last_worktree_root_id?;
        let project = self.project.read(cx);
        let worktree = project.worktree_for_entry(root_entry_id, cx)?;
        let worktree = worktree.read(cx);
        let root_entry = worktree.entry_for_id(root_entry_id)?;
        Some(worktree.absolutize(&root_entry.path))
    }

    fn selected_entry_handle<'a>(&self, cx: &'a App) -> Option<(Entity<Worktree>, &'a project::Entry)> {
        let selection = self.selection?;
        let project = self.project.read(cx);
        let worktree = project.worktree_for_id(selection.worktree_id, cx)?;
        let entry = worktree.read(cx).entry_for_id(selection.entry_id)?;
        Some((worktree, entry))
    }

    fn expand_to_selection(&mut self, cx: &Context<Self>) -> Option<()> {
        let (worktree, entry) = self.selected_entry(cx)?;
        let expanded_dir_ids = self.state.expanded_dir_ids.entry(worktree.id()).or_default();

        for path in entry.path.ancestors() {
            let Some(entry) = worktree.entry_for_path(path) else {
                continue;
            };
            if entry.is_dir()
                && let Err(idx) = expanded_dir_ids.binary_search(&entry.id)
            {
                expanded_dir_ids.insert(idx, entry.id);
            }
        }

        Some(())
    }

    fn expand_entry(&mut self, worktree_id: WorktreeId, entry_id: ProjectEntryId, cx: &mut Context<Self>) {
        self.project.update(cx, |project, cx| {
            // A worktree the panel hasn't listed yet (e.g. a just-opened library) has
            // no expanded dirs; start them here so the first reveal expands its path.
            if let Some(worktree) = project.worktree_for_id(worktree_id, cx) {
                let expanded_dir_ids = self.state.expanded_dir_ids.entry(worktree_id).or_default();
                project.expand_entry(worktree_id, entry_id, cx);
                let worktree = worktree.read(cx);

                if let Some(mut entry) = worktree.entry_for_id(entry_id) {
                    loop {
                        if entry.is_dir()
                            && let Err(ix) = expanded_dir_ids.binary_search(&entry.id)
                        {
                            expanded_dir_ids.insert(ix, entry.id);
                        }

                        if let Some(parent_entry) = entry.path.parent().and_then(|p| worktree.entry_for_path(p)) {
                            entry = parent_entry;
                        } else {
                            break;
                        }
                    }
                }
            }
        });
    }

    const fn is_copy_modifier_set(modifiers: Modifiers) -> bool {
        true && modifiers.alt || false && modifiers.control
    }

    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults();
        dispatch_context.add("ProjectPanel");
        dispatch_context.add("menu");

        let identifier =
            if self.filename_editor.focus_handle(cx).is_focused(window) { "editing" } else { "not_editing" };

        dispatch_context.add(identifier);
        dispatch_context
    }

    fn reveal_entry(
        &mut self,
        project: Entity<Project>,
        entry_id: ProjectEntryId,
        skip_ignored: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let worktree = project
            .read(cx)
            .worktree_for_entry(entry_id, cx)
            .context("can't reveal a non-existent entry in the project panel")?;
        let worktree = worktree.read(cx);
        let worktree_id = worktree.id();
        let is_ignored =
            worktree.entry_for_id(entry_id).is_none_or(|entry| entry.is_ignored && !entry.is_always_included);
        // Library sources are often gitignored (e.g. by an ignore file above the
        // registry), but opening one should still reveal it.
        if skip_ignored && is_ignored && worktree.library_name().is_none() {
            if self.index_for_entry(entry_id, worktree_id).is_none() {
                anyhow::bail!("can't reveal an ignored entry in the project panel");
            }

            self.selection = Some(SelectedEntry { worktree_id, entry_id });
            self.marked_entries.clear();
            self.marked_entries.push(SelectedEntry { worktree_id, entry_id });
            self.autoscroll(cx);
            cx.notify();
            return Ok(());
        }
        let is_active_item_file_diff_view = self
            .workspace
            .upgrade()
            .and_then(|ws| ws.read(cx).active_item(cx))
            .is_some_and(|item| item.act_as_type(TypeId::of::<FileDiffView>(), cx).is_some());
        if is_active_item_file_diff_view {
            return Ok(());
        }

        if worktree.library_name().is_some() {
            self.libraries_expanded = true;
        }
        self.expand_entry(worktree_id, entry_id, cx);
        self.update_visible_entries(Some((worktree_id, entry_id)), false, true, window, cx);
        self.marked_entries.clear();
        self.marked_entries.push(SelectedEntry { worktree_id, entry_id });
        cx.notify();
        Ok(())
    }
}

/// A single-file worktree's root entry has an empty path, so its name is the worktree's root name.
fn entry_file_name<'a>(entry: &'a Entry, root_name: &'a RelPath) -> &'a str {
    entry.path.file_name().unwrap_or_else(|| root_name.as_unix_str())
}

const fn item_width_estimate(depth: usize, item_text_chars: usize, is_symlink: bool) -> usize {
    const ICON_SIZE_FACTOR: usize = 2;
    let mut item_width = depth * ICON_SIZE_FACTOR + item_text_chars;
    if is_symlink {
        item_width += ICON_SIZE_FACTOR;
    }
    item_width
}

impl EventEmitter<Event> for ProjectPanel {}

impl EventEmitter<PanelEvent> for ProjectPanel {}

impl Panel for ProjectPanel {
    fn position(&self, _: &Window, _: &App) -> DockPosition {
        DockPosition::Left
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        DEFAULT_WIDTH
    }

    fn min_size(&self, _: &Window, _: &App) -> Option<Pixels> {
        Some(SIDEBAR_MIN_WIDTH)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::FileTree)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Project Panel")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn persistent_name() -> &'static str {
        "Project Panel"
    }

    fn panel_key() -> &'static str {
        PROJECT_PANEL_KEY
    }

    fn starts_open(&self, _: &Window, cx: &App) -> bool {
        let project = &self.project.read(cx);
        project.visible_worktrees(cx).any(|tree| tree.read(cx).root_entry().is_some_and(project::Entry::is_dir))
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            cx.defer_in(window, |panel, window, cx| {
                panel.preload_initial_entry_icons(window, cx);
            });
        }
    }

    fn activation_priority(&self) -> u32 {
        1
    }
}

impl Focusable for ProjectPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Directories first, then case-insensitive natural name order.
#[inline]
fn cmp_worktree_entries(a: &Entry, b: &Entry) -> cmp::Ordering {
    let a = (&*a.path, a.is_file());
    let b = (&*b.path, b.is_file());
    paths::compare_rel_paths_by(a, b, SortMode::DirectoriesFirst, SortOrder::Default)
}

fn sort_worktree_entries(entries: &mut [impl AsRef<Entry>]) {
    entries.sort_by(|lhs, rhs| cmp_worktree_entries(lhs.as_ref(), rhs.as_ref()));
}

/// Errors win over warnings; files with neither get no severity.
const fn strongest_diagnostic_severity(error_count: usize, warning_count: usize) -> Option<DiagnosticSeverity> {
    if error_count > 0 {
        Some(DiagnosticSeverity::ERROR)
    } else if warning_count > 0 {
        Some(DiagnosticSeverity::WARNING)
    } else {
        None
    }
}

/// Opens the menu downwards from the entry when the entry is in the upper half of the window
/// and upwards otherwise, always towards the center of the window (the panel is docked left),
/// so that the menu has the most room to grow into.
/// The menu's anchored corner sits on the diagonally opposite corner of the entry.
fn context_menu_placement(entry_bounds: Bounds<Pixels>, viewport_size: gpui::Size<Pixels>) -> (Point<Pixels>, Anchor) {
    if entry_bounds.center().y <= viewport_size.center().y {
        (entry_bounds.bottom_left(), Anchor::TopLeft)
    } else {
        (entry_bounds.origin, Anchor::BottomLeft)
    }
}

fn explain_move_error(err: anyhow::Error, names: Option<&(String, String)>) -> anyhow::Error {
    match names {
        Some((from, to)) if already_exists_error(&err) => {
            err.context(format!("Failed to move `{from}` to `{to}`. A file or folder already exists there."))
        }
        _ => err,
    }
}

fn libraries_header_entry() -> GitEntry {
    GitEntry {
        entry: Entry {
            id: LIBRARIES_ENTRY_ID,
            kind: EntryKind::Dir,
            path: RelPath::empty_arc(),
            inode: 0,
            mtime: None,
            canonical_path: None,
            is_ignored: false,
            is_hidden: false,
            is_always_included: false,
            is_external: false,
            size: 0,
            char_bag: CharBag::default(),
            is_fifo: false,
            is_writable: false,
        },
        git_summary: GitSummary::default(),
    }
}

#[cfg(test)]
mod tests {

    use super::Anchor;
    use super::Bounds;
    use super::DiagnosticSeverity;
    use super::Entry;
    use super::EntryKind;
    use super::GitEntry;
    use super::GitSummary;
    use super::ProjectEntryId;
    use super::RelPath;
    use super::context_menu_placement;
    use super::last_entry_in_subtree;
    use super::libraries_header_entry;
    use super::point;
    use super::px;
    use super::size;
    use super::sort_worktree_entries;
    use super::strongest_diagnostic_severity;

    #[test]
    fn last_entry_in_subtree_works() {
        let paths = ["", "a", "a/b", "a/b/c", "a/d", "ab", "e"].map(|path| RelPath::new_test(path).into_owned());
        let entries = || paths.iter().enumerate().map(|(ix, path)| (ProjectEntryId::from_usize(ix), path.as_ref()));
        let id = ProjectEntryId::from_usize;

        assert_eq!(last_entry_in_subtree(entries(), id(0)), Some(id(6)));
        // `ab` shares the `a` prefix but is not inside `a`.
        assert_eq!(last_entry_in_subtree(entries(), id(1)), Some(id(4)));
        assert_eq!(last_entry_in_subtree(entries(), id(2)), Some(id(3)));
        assert_eq!(last_entry_in_subtree(entries(), id(4)), Some(id(4)));
        assert_eq!(last_entry_in_subtree(entries(), id(6)), Some(id(6)));
        assert_eq!(last_entry_in_subtree(entries(), id(9)), None);
    }

    #[test]
    fn strongest_diagnostic_severity_works() {
        assert_eq!(strongest_diagnostic_severity(0, 0), None);
        assert_eq!(strongest_diagnostic_severity(0, 2), Some(DiagnosticSeverity::WARNING));
        assert_eq!(strongest_diagnostic_severity(1, 0), Some(DiagnosticSeverity::ERROR));
        assert_eq!(strongest_diagnostic_severity(1, 2), Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn context_menu_placement_works() {
        let viewport = size(px(800.), px(600.));
        let upper = Bounds::new(point(px(10.), px(100.)), size(px(200.), px(20.)));
        assert_eq!(context_menu_placement(upper, viewport), (upper.bottom_left(), Anchor::TopLeft));

        let lower = Bounds::new(point(px(10.), px(500.)), size(px(200.), px(20.)));
        assert_eq!(context_menu_placement(lower, viewport), (lower.origin, Anchor::BottomLeft));
    }

    #[test]
    fn sort_worktree_entries_lists_directories_first() {
        let entry = |id: usize, path: &str, kind: EntryKind| GitEntry {
            entry: Entry {
                id: ProjectEntryId::from_usize(id),
                kind,
                path: RelPath::new_test(path).into_owned().into(),
                ..libraries_header_entry().entry
            },
            git_summary: GitSummary::default(),
        };
        let mut entries = vec![
            entry(0, "b.txt", EntryKind::File),
            entry(1, "Zed", EntryKind::Dir),
            entry(2, "a10.txt", EntryKind::File),
            entry(3, "a2.txt", EntryKind::File),
            entry(4, "src", EntryKind::Dir),
        ];
        let expected = ["src", "Zed", "a2.txt", "a10.txt", "b.txt"];

        sort_worktree_entries(&mut entries);
        assert_eq!(entries.iter().map(|e| e.path.as_unix_str()).collect::<Vec<_>>(), expected);
    }
}
