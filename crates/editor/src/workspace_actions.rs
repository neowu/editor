use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ed_actions::editor::RevealInFileManager;
use ed_actions::workspace::CopyPath;
use ed_actions::workspace::CopyRelativePath;
use gpui::Action as _;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncWindowContext;
use gpui::ClipboardItem;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use gpui::Window;
use itertools::Itertools as _;
use language::Capability;
use language::OffsetRangeExt as _;
use language::Point;
use multi_buffer::BufferOffset;
use multi_buffer::EXCERPT_CONTEXT_LINES;
use multi_buffer::MultiBuffer;
use multi_buffer::MultiBufferPoint;
use multi_buffer::PathKey;
use project::ProjectItem as _;
use project::ProjectTransaction;
use rustc_hash::FxHashMap;
use text::Bias;
use workspace::Item as _;
use workspace::OpenInTerminal;
use workspace::OpenTerminal;
use workspace::SplitDirection;
use workspace::Workspace;
use workspace::notifications::DetachAndPromptErr as _;

use super::Editor;
use super::EditorEvent;
use super::JumpData;
use super::MultibufferSelectionMode;
use super::SelectionEffects;
use crate::actions::CopyFileLocation;
use crate::actions::CopyFileName;
use crate::actions::CopyFileNameWithoutExtension;
use crate::actions::OpenSelectionsInMultibuffer;
use crate::actions::ReloadFile;
use crate::display_map::DisplayRow;
use crate::display_map::HighlightKey;
use crate::scroll::Autoscroll;
use crate::scroll::ScrollOffset;

impl Editor {
    pub fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        if let Some(buffer) = self.buffer().read(cx).as_singleton()
            && let Some(file) = buffer.read(cx).file().and_then(|f| f.as_local())
            && let Some(dir) = file.abs_path(cx).parent()
        {
            return Some(dir.to_owned());
        }

        None
    }

    pub fn target_file_abs_path(&self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let buffer = self.active_buffer(cx)?;
        let buffer = buffer.read(cx);
        if let Some(project_path) = buffer.project_path(cx) {
            let project = self.project()?.read(cx);
            project.absolute_path(&project_path, cx)
        } else {
            let file = buffer.file()?;
            file.as_local().map(|file| file.abs_path(cx))
        }
    }

    pub fn new_file(
        workspace: &mut Workspace,
        _: &workspace::NewFile,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::new_in_workspace(workspace, window, cx).detach_and_prompt_err(
            "Failed to create buffer",
            window,
            cx,
            |_| None,
        );
    }

    pub fn new_in_workspace(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<Result<Entity<Editor>>> {
        let project = workspace.project().clone();
        let create = project.update(cx, |project, cx| project.create_buffer(None, true, cx));

        cx.spawn_in(window, async move |workspace, cx| {
            let buffer = create.await?;
            workspace.update_in(cx, |workspace, window, cx| {
                let editor = cx.new(|cx| Editor::for_buffer(buffer, Some(project.clone()), window, cx));
                workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
                editor
            })
        })
    }

    pub(super) fn new_file_vertical(
        workspace: &mut Workspace,
        _: &workspace::NewFileSplitVertical,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::new_file_in_direction(workspace, SplitDirection::Right, window, cx);
    }

    pub(super) fn new_file_horizontal(
        workspace: &mut Workspace,
        _: &workspace::NewFileSplitHorizontal,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::new_file_in_direction(workspace, SplitDirection::Down, window, cx);
    }

    pub(super) fn new_file_split(
        workspace: &mut Workspace,
        action: &workspace::NewFileSplit,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        Self::new_file_in_direction(workspace, action.0, window, cx);
    }

    fn new_file_in_direction(
        workspace: &Workspace,
        direction: SplitDirection,
        window: &Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project().clone();
        let create = project.update(cx, |project, cx| project.create_buffer(None, true, cx));

        cx.spawn_in(window, async move |workspace, cx| {
            let buffer = create.await?;
            workspace.update_in(cx, move |workspace, window, cx| {
                workspace.split_item(
                    direction,
                    Box::new(cx.new(|cx| Editor::for_buffer(buffer, Some(project.clone()), window, cx))),
                    window,
                    cx,
                );
            })?;
            anyhow::Ok(())
        })
        .detach_and_prompt_err("Failed to create buffer", window, cx, |_| None);
    }

    pub(super) fn open_transaction_for_hidden_buffers(
        workspace: Entity<Workspace>,
        transaction: ProjectTransaction,
        title: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if transaction.0.is_empty() {
            return;
        }

        let edited_buffers_already_open = {
            let other_editors: Vec<Entity<Editor>> = workspace
                .read(cx)
                .panes()
                .iter()
                .flat_map(|pane| pane.read(cx).items_of_type::<Editor>())
                .filter(|editor| editor.entity_id() != cx.entity_id())
                .collect();

            transaction.0.keys().all(|buffer| {
                other_editors.iter().any(|editor| {
                    let multi_buffer = editor.read(cx).buffer();
                    multi_buffer.read(cx).is_singleton()
                        && multi_buffer
                            .read(cx)
                            .as_singleton()
                            .is_some_and(|singleton| singleton.entity_id() == buffer.entity_id())
                })
            })
        };
        if !edited_buffers_already_open {
            let workspace = workspace.downgrade();
            cx.defer_in(window, move |_, window, cx| {
                cx.spawn_in(window, async move |editor, cx| {
                    Self::open_project_transaction(&editor, workspace, transaction, title, cx).ok()
                })
                .detach();
            });
        }
    }

    pub fn open_project_transaction(
        editor: &WeakEntity<Editor>,
        workspace: WeakEntity<Workspace>,
        transaction: ProjectTransaction,
        title: String,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let mut entries = transaction.0.into_iter().collect::<Vec<_>>();
        cx.update(|_, cx| {
            entries.sort_unstable_by_key(|(buffer, _)| buffer.read(cx).file().map(|f| Arc::clone(f.path())));
        })?;
        if entries.is_empty() {
            return Ok(());
        }

        // If the project transaction's edits are all contained within this editor, then
        // avoid opening a new editor to display them.

        if let [(buffer, transaction)] = &*entries {
            let cursor_excerpt = editor.update(cx, |editor, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                let head = editor.selections.newest_anchor().head();
                let (buffer_snapshot, excerpt_range) = snapshot.excerpt_containing(head..head)?;
                if buffer_snapshot.buffer_id() != buffer.read(cx).buffer_id() {
                    return None;
                }
                Some(excerpt_range)
            })?;

            if let Some(excerpt_range) = cursor_excerpt {
                let all_edits_within_excerpt = buffer.read_with(cx, |buffer, _| {
                    let excerpt_range = excerpt_range.context.to_offset(buffer);
                    buffer
                        .edited_ranges_for_transaction::<usize>(transaction)
                        .all(|range| excerpt_range.start <= range.start && excerpt_range.end >= range.end)
                });

                if all_edits_within_excerpt {
                    return Ok(());
                }
            }
        }

        let mut ranges_to_highlight = Vec::new();
        let excerpt_buffer = cx.new(|cx| {
            let mut multibuffer = MultiBuffer::new(Capability::ReadWrite).with_title(title);
            for (buffer_handle, transaction) in &entries {
                let edited_ranges =
                    buffer_handle.read(cx).edited_ranges_for_transaction::<Point>(transaction).collect::<Vec<_>>();
                multibuffer.set_excerpts_for_path(
                    PathKey::for_buffer(buffer_handle, cx),
                    buffer_handle.clone(),
                    edited_ranges.clone(),
                    EXCERPT_CONTEXT_LINES,
                    cx,
                );
                let snapshot = multibuffer.snapshot(cx);
                let buffer_snapshot = buffer_handle.read(cx).snapshot();
                ranges_to_highlight.extend(edited_ranges.into_iter().filter_map(|range| {
                    let text_range = buffer_snapshot.anchor_range_inside(range);
                    let start = snapshot.anchor_in_buffer(text_range.start)?;
                    let end = snapshot.anchor_in_buffer(text_range.end)?;
                    Some(start..end)
                }));
            }
            multibuffer.push_transaction(entries.iter().map(|(b, t)| (b, t)), cx);
            multibuffer
        });

        workspace.update_in(cx, |workspace, window, cx| {
            let project = workspace.project().clone();
            let editor = cx.new(|cx| Editor::for_multibuffer(excerpt_buffer, Some(project), window, cx));
            workspace.add_item_to_active_pane(Box::new(editor.clone()), None, true, window, cx);
            editor.update(cx, |editor, cx| {
                editor.highlight_background(
                    HighlightKey::Editor,
                    &ranges_to_highlight,
                    |_, theme| theme.colors().editor_highlighted_line_background,
                    cx,
                );
            });
        })?;

        Ok(())
    }

    pub fn reload_file(&mut self, _: &ReloadFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let task = self.reload(project, window, cx);
        self.detach_and_notify_err(task, window, cx);
    }

    pub fn open_active_item_in_terminal(&mut self, _: &OpenInTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(working_directory) = self.active_buffer(cx).and_then(|buffer| {
            let project_path = buffer.read(cx).project_path(cx)?;
            let project = self.project()?.read(cx);
            let entry = project.entry_for_path(&project_path, cx)?;
            let parent = match &entry.canonical_path {
                Some(canonical_path) => canonical_path.to_path_buf(),
                None => project.absolute_path(&project_path, cx)?,
            }
            .parent()?
            .to_path_buf();
            Some(parent)
        }) {
            window.dispatch_action(OpenTerminal { working_directory }.boxed_clone(), cx);
        }
    }

    pub(super) fn target_file<'a>(&self, cx: &'a App) -> Option<&'a dyn language::LocalFile> {
        self.active_buffer(cx)?.read(cx).file().and_then(|f| f.as_local())
    }

    pub(super) fn reveal_in_finder(&mut self, _: &RevealInFileManager, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.target_file_abs_path(cx) {
            if let Some(project) = self.project() {
                project.update(cx, |project, cx| project.reveal_path(&path, cx));
            } else {
                cx.reveal_path(&path);
            }
        }
    }

    pub(super) fn copy_path(&mut self, _: &CopyPath, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.target_file_abs_path(cx)
            && let Some(path) = path.to_str()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(path.to_owned()));
        } else {
            cx.propagate();
        }
    }

    pub(super) fn copy_relative_path(&mut self, _: &CopyRelativePath, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_buffer(cx).and_then(|buffer| Some(Arc::clone(buffer.read(cx).file()?.path()))) {
            cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_owned()));
        } else {
            cx.propagate();
        }
    }

    pub fn copy_file_name_without_extension(
        &mut self,
        _: &CopyFileNameWithoutExtension,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(file_stem) = self.active_buffer(cx).and_then(|buffer| {
            let file = buffer.read(cx).file()?;
            Path::new(file.file_name(cx)).file_stem()?.to_str()
        }) {
            cx.write_to_clipboard(ClipboardItem::new_string(file_stem.to_owned()));
        }
    }

    pub fn copy_file_name(&mut self, _: &CopyFileName, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(file_name) = self.active_buffer(cx).and_then(|buffer| {
            let file = buffer.read(cx).file()?;
            Some(file.file_name(cx))
        }) {
            cx.write_to_clipboard(ClipboardItem::new_string(file_name.to_owned()));
        }
    }

    pub fn copy_file_location(&mut self, _: &CopyFileLocation, _: &mut Window, cx: &mut Context<Self>) {
        let selections = self.selections.all::<Point>(&self.display_snapshot(cx));
        if let Some(file_location) = self.file_location(selections.iter().map(|s| (s.range(), s.head())), cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(file_location));
        }
    }

    /// Copies `path:line` for the clicked gutter row, or the location of all
    /// selections when one of them covers that row.
    pub(super) fn copy_file_location_at_row(&mut self, row: DisplayRow, cx: &mut Context<Self>) {
        let display_snapshot = self.display_snapshot(cx);
        let point = display_snapshot.display_point_to_point(row.as_display_point(), Bias::Left);
        let selections = self.selections.all::<Point>(&display_snapshot);
        let file_location = if selections.iter().any(|s| (s.start.row..=s.end.row).contains(&point.row)) {
            self.file_location(selections.iter().map(|s| (s.range(), s.head())), cx)
        } else {
            self.file_location([(point..point, point)], cx)
        };
        if let Some(file_location) = file_location {
            cx.write_to_clipboard(ClipboardItem::new_string(file_location));
            self.gutter_copied_row = (
                Some(row),
                Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(GUTTER_COPIED_STATE_DURATION).await;
                    this.update(cx, |this, cx| {
                        this.gutter_copied_row = (None, None);
                        cx.notify();
                    })
                    .ok();
                })),
            );
            cx.notify();
        }
    }

    /// `ranges` are `(selection range, cursor)` pairs in buffer order.
    fn file_location(&self, ranges: impl IntoIterator<Item = (Range<Point>, Point)>, cx: &App) -> Option<String> {
        let multi_buffer_snapshot = self.buffer.read(cx).snapshot(cx);
        let mut lines = Vec::new();
        for (range, cursor) in ranges {
            let Some((buffer, range)) = multi_buffer_snapshot.range_to_buffer_range(range).or_else(|| {
                // A range that spans multiple buffers has no single location,
                // so fall back to the buffer the cursor is in.
                let (buffer, point) = multi_buffer_snapshot.point_to_buffer_point(cursor)?;
                Some((buffer, point..point))
            }) else {
                continue;
            };
            let Some(file) = buffer.file() else {
                continue;
            };

            let start_line = range.start.row + 1;
            let end_line = range.end.row + 1;
            let end_line = if range.end.column == 0 && end_line > start_line { end_line - 1 } else { end_line };
            lines.push((file.path().display(), start_line, end_line));
        }
        (!lines.is_empty()).then(|| format_file_locations(&lines))
    }

    pub fn open_selections_in_multibuffer(
        &mut self,
        _: &OpenSelectionsInMultibuffer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let multibuffer = self.buffer.read(cx);

        let Some(buffer) = multibuffer.as_singleton() else {
            return;
        };
        let buffer_snapshot = buffer.read(cx).snapshot();

        let Some(workspace) = self.workspace() else {
            return;
        };

        let title = multibuffer.title(cx).to_string();

        let locations = self
            .selections
            .all_anchors(&self.display_snapshot(cx))
            .iter()
            .map(|selection| {
                (
                    buffer.clone(),
                    (selection.start.text_anchor_in(&buffer_snapshot)..selection.end.text_anchor_in(&buffer_snapshot))
                        .to_point(buffer.read(cx)),
                )
            })
            .into_group_map();

        cx.spawn_in(window, async move |_, cx| {
            workspace.update_in(cx, |workspace, window, cx| {
                Self::open_locations_in_multibuffer(
                    workspace,
                    locations,
                    format!("Selections for '{title}'"),
                    false,
                    MultibufferSelectionMode::All,
                    window,
                    cx,
                );
            })
        })
        .detach();
    }

    /// Opens the buffer under the newest cursor at that cursor, e.g. when a
    /// commit view opens a file's revision right after moving its cursor there.
    pub fn open_excerpt_at_newest_cursor(&self, window: &Window, cx: &mut Context<Self>) {
        let head = self.selections.newest::<Point>(&self.display_snapshot(cx)).head();
        let mut new_selections_by_buffer = FxHashMap::default();
        if let Some((buffer, buffer_point)) = self.buffer.read(cx).point_to_buffer_point(head, cx) {
            let buffer_offset = BufferOffset(buffer.read(cx).point_to_offset(buffer_point));
            new_selections_by_buffer.insert(buffer, (vec![buffer_offset..buffer_offset], None));
        }
        self.open_selections_by_buffer(new_selections_by_buffer, false, window, cx);
    }

    /// Opens the single buffer `jump_data` points at, with one cursor there.
    pub(crate) fn open_excerpts_common(
        &self,
        jump_data: JumpData,
        split: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mut new_selections_by_buffer = FxHashMap::default();
        match jump_data {
            JumpData::MultiBufferPoint { anchor, position, line_offset_from_top } => {
                if let Some(buffer) = self.buffer.read(cx).buffer(anchor.buffer_id) {
                    let buffer_snapshot = buffer.read(cx).snapshot();
                    let jump_to_point = if buffer_snapshot.can_resolve(&anchor) {
                        language::ToPoint::to_point(&anchor, &buffer_snapshot)
                    } else {
                        buffer_snapshot.clip_point(position, Bias::Left)
                    };
                    let jump_to_offset = buffer_snapshot.point_to_offset(jump_to_point);
                    new_selections_by_buffer.insert(
                        buffer,
                        (vec![BufferOffset(jump_to_offset)..BufferOffset(jump_to_offset)], Some(line_offset_from_top)),
                    );
                }
            }
            JumpData::MultiBufferRow { row, line_offset_from_top } => {
                let point = MultiBufferPoint::new(row.0, 0);
                if let Some((buffer, buffer_point)) = self.buffer.read(cx).point_to_buffer_point(point, cx) {
                    let buffer_offset = BufferOffset(buffer.read(cx).point_to_offset(buffer_point));
                    new_selections_by_buffer
                        .insert(buffer, (vec![buffer_offset..buffer_offset], Some(line_offset_from_top)));
                }
            }
        }
        self.open_selections_by_buffer(new_selections_by_buffer, split, window, cx);
    }

    #[allow(clippy::type_complexity)]
    fn open_selections_by_buffer(
        &self,
        mut new_selections_by_buffer: FxHashMap<Entity<language::Buffer>, (Vec<Range<BufferOffset>>, Option<u32>)>,
        split: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.buffer.read(cx).is_singleton() {
            cx.propagate();
            return;
        }

        if self.delegate_open_excerpts {
            let selections_by_buffer: FxHashMap<_, _> = new_selections_by_buffer
                .into_iter()
                .map(|(buffer, value)| (buffer.read(cx).buffer_id(), value))
                .collect();
            if !selections_by_buffer.is_empty() {
                cx.emit(EditorEvent::OpenExcerptsRequested { selections_by_buffer, split });
            }
            return;
        }

        let Some(workspace) = self.workspace() else {
            cx.propagate();
            return;
        };

        new_selections_by_buffer.retain(|buffer, _| buffer.read(cx).file().is_none_or(|file| file.can_open()));

        if new_selections_by_buffer.is_empty() {
            return;
        }

        Self::open_buffers_in_workspace(workspace.downgrade(), new_selections_by_buffer, split, window, cx);
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn open_buffers_in_workspace(
        workspace: WeakEntity<Workspace>,
        new_selections_by_buffer: FxHashMap<Entity<language::Buffer>, (Vec<Range<BufferOffset>>, Option<u32>)>,
        split: bool,
        window: &Window,
        cx: &mut App,
    ) {
        // We defer the pane interaction because we ourselves are a workspace item
        // and activating a new item causes the pane to call a method on us reentrantly,
        // which panics if we're on the stack.
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    let workspace_pane =
                        if split { workspace.adjacent_pane(window, cx) } else { workspace.active_pane().clone() };

                    for (buffer, (ranges, scroll_offset)) in new_selections_by_buffer {
                        let buffer_read = buffer.read(cx);
                        let (has_file, is_project_file) = if let Some(file) = buffer_read.file() {
                            (true, project::File::from_dyn(Some(file)).is_some())
                        } else {
                            (false, false)
                        };
                        let in_history = workspace.mode().opens_in_history_pane(is_project_file);
                        let pane =
                            if in_history { workspace.git_history_pane().clone() } else { workspace_pane.clone() };
                        let target_pane = (split || in_history).then(|| pane.clone());

                        // If project file is none workspace.open_project_item will fail to open the excerpt
                        // in a pre existing workspace item if one exists, because Buffer entity_id will be None
                        // so we check if there's a tab match in that case first
                        let editor = (!has_file || !is_project_file)
                            .then(|| {
                                // Handle file-less buffers separately: those are not really the project items, so won't have a project path or entity id,
                                // so `workspace.open_project_item` will never find them, always opening a new editor.
                                // Instead, we try to activate the existing editor in the pane first.
                                let (editor, pane_item_index) =
                                    pane.read(cx).items().enumerate().find_map(|(i, item)| {
                                        let editor = item.downcast::<Editor>()?;
                                        let singleton_buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
                                        (singleton_buffer == buffer).then_some((editor, i))
                                    })?;
                                pane.update(cx, |pane, cx| pane.activate_item(pane_item_index, true, true, window, cx));
                                Some(editor)
                            })
                            .flatten()
                            .unwrap_or_else(|| {
                                workspace.open_project_item::<Self>(
                                    target_pane,
                                    buffer,
                                    true,
                                    true,
                                    false,
                                    true,
                                    window,
                                    cx,
                                )
                            });

                        editor.update(cx, |editor, cx| {
                            if has_file && !is_project_file {
                                editor.set_read_only(true);
                            }
                            let autoscroll = match scroll_offset {
                                Some(scroll_offset) => Autoscroll::top_relative(scroll_offset as ScrollOffset),
                                None => Autoscroll::newest(),
                            };
                            let nav_history = editor.nav_history.take();
                            let multibuffer_snapshot = editor.buffer().read(cx).snapshot(cx);
                            let Some(buffer_snapshot) = multibuffer_snapshot.as_singleton() else {
                                return;
                            };
                            editor.change_selections(SelectionEffects::scroll(autoscroll), window, cx, |s| {
                                s.select_ranges(ranges.into_iter().map(|range| {
                                    let range = buffer_snapshot.anchor_before(range.start)
                                        ..buffer_snapshot.anchor_after(range.end);
                                    multibuffer_snapshot
                                        .buffer_anchor_range_to_anchor_range(range)
                                        .expect("range is within singleton buffer")
                                }));
                            });
                            editor.nav_history = nav_history;
                        });
                    }
                })
                .ok();
        });
    }
}

pub(super) const GUTTER_COPIED_STATE_DURATION: Duration = Duration::from_secs(2);

/// Formats `(path, start_line, end_line)` entries as `path:1,3,5-7`, one line per
/// run of entries sharing a path. Overlapping or adjacent line ranges are merged.
fn format_file_locations(lines: &[(&str, u32, u32)]) -> String {
    let mut text = String::new();
    let mut ix = 0;
    while ix < lines.len() {
        let (path, mut start, mut end) = lines[ix];
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(path);
        let mut separator = ':';
        loop {
            ix += 1;
            let next = lines.get(ix).filter(|next| next.0 == path);
            if let Some(&(_, next_start, next_end)) = next
                && next_start <= end + 1
            {
                start = start.min(next_start);
                end = end.max(next_end);
                continue;
            }
            text.push(separator);
            separator = ',';
            text.push_str(&start.to_string());
            if end > start {
                text.push('-');
                text.push_str(&end.to_string());
            }
            let Some(&(_, next_start, next_end)) = next else {
                break;
            };
            (start, end) = (next_start, next_end);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_file_locations_single() {
        assert_eq!(format_file_locations(&[("a.rs", 3, 3)]), "a.rs:3");
        assert_eq!(format_file_locations(&[("a.rs", 3, 7)]), "a.rs:3-7");
    }

    #[test]
    fn format_file_locations_multiple_selections() {
        assert_eq!(format_file_locations(&[("a.rs", 1, 1), ("a.rs", 3, 3), ("a.rs", 5, 5)]), "a.rs:1,3,5");
        assert_eq!(format_file_locations(&[("a.rs", 1, 2), ("a.rs", 5, 5), ("a.rs", 8, 10)]), "a.rs:1-2,5,8-10");
    }

    #[test]
    fn format_file_locations_merges_adjacent_and_overlapping() {
        assert_eq!(format_file_locations(&[("a.rs", 1, 1), ("a.rs", 2, 2), ("a.rs", 3, 3)]), "a.rs:1-3");
        assert_eq!(format_file_locations(&[("a.rs", 1, 4), ("a.rs", 4, 4), ("a.rs", 6, 6)]), "a.rs:1-4,6");
        assert_eq!(format_file_locations(&[("a.rs", 2, 2), ("a.rs", 2, 2)]), "a.rs:2");
    }

    #[test]
    fn format_file_locations_multiple_files() {
        assert_eq!(
            format_file_locations(&[("a.rs", 1, 1), ("a.rs", 3, 3), ("b.rs", 2, 4), ("b.rs", 5, 5), ("a.rs", 9, 9)]),
            "a.rs:1,3\nb.rs:2-5\na.rs:9"
        );
    }
}
