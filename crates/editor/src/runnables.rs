use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use clock::Global;
use gpui::App;
use gpui::AppContext as _;
use gpui::ClickEvent;
use gpui::Context;
use gpui::Entity;
use gpui::Focusable as _;
use gpui::MouseButton;
use gpui::Task;
use gpui::Window;
use language::Buffer;
use language::BufferRow;
use language::Point;
use lsp::LanguageServerName;
use multi_buffer::Anchor;
use multi_buffer::MultiBufferRow;
use multi_buffer::MultiBufferSnapshot;
use multi_buffer::ToPoint as _;
use project::Project;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use task::RunnableTag;
use task::SpawnInTerminal;
use text::Bias;
use text::BufferId;
use text::ToPoint as _;
use ui::Clickable as _;
use ui::Color;
use ui::IconButton;
use ui::IconSize;
use ui::Toggleable as _;
use workspace::tasks::ScheduledTaskResult;

use crate::CodeActionSource;
use crate::Editor;
use crate::EditorStyle;
use crate::RangeToAnchorExt as _;
use crate::ToggleCodeActions;
use crate::UPDATE_DEBOUNCE;
use crate::display_map::DisplayPoint;
use crate::display_map::DisplayRow;
use crate::display_map::DisplaySnapshot;
use crate::display_map::ToDisplayPoint as _;

#[derive(Debug)]
pub(super) struct RunnableData {
    runnables: FxHashMap<BufferId, (Global, BTreeMap<BufferRow, RunnableTasks>)>,
    task_statuses: FxHashMap<(BufferId, BufferRow), RunnableTaskStatus>,
    invalidate_buffer_data: FxHashSet<BufferId>,
    /// Singleton editors only: the buffer version and rows the cached runnables cover.
    queried_rows: Option<(Global, Range<BufferRow>)>,
    runnables_update_task: Task<()>,
}

impl RunnableData {
    pub(crate) fn new() -> Self {
        Self {
            runnables: FxHashMap::default(),
            task_statuses: FxHashMap::default(),
            invalidate_buffer_data: FxHashSet::default(),
            queried_rows: None,
            runnables_update_task: Task::ready(()),
        }
    }

    pub(crate) fn runnables(&self, (buffer_id, buffer_row): (BufferId, BufferRow)) -> Option<&RunnableTasks> {
        self.runnables.get(&buffer_id)?.1.get(&buffer_row)
    }

    pub(crate) fn all_runnables(&self) -> impl Iterator<Item = &RunnableTasks> {
        self.runnables.values().flat_map(|(_, tasks)| tasks.values())
    }

    pub(crate) fn task_status(&self, (buffer_id, buffer_row): (BufferId, BufferRow)) -> Option<RunnableTaskStatus> {
        self.task_statuses.get(&(buffer_id, buffer_row)).copied()
    }

    pub(crate) fn set_task_status(
        &mut self,
        (buffer_id, buffer_row): (BufferId, BufferRow),
        status: RunnableTaskStatus,
    ) {
        self.task_statuses.insert((buffer_id, buffer_row), status);
    }

    pub(crate) fn clear_task_status(&mut self, (buffer_id, buffer_row): (BufferId, BufferRow)) {
        self.task_statuses.remove(&(buffer_id, buffer_row));
    }

    pub(crate) fn has_cached(&self, buffer_id: BufferId, version: &Global) -> bool {
        self.runnables.get(&buffer_id).is_some_and(|(cached_version, _)| !version.changed_since(cached_version))
    }

    /// Whether the last query ran at `version` and covered `rows`.
    fn covers(&self, version: &Global, rows: &Range<BufferRow>) -> bool {
        self.queried_rows.as_ref().is_some_and(|(queried_version, queried)| {
            !version.changed_since(queried_version) && queried.start <= rows.start && rows.end <= queried.end
        })
    }
}

/// Whether `language`'s runnables come from its language server rather than its syntax query.
fn has_lsp_runnables(language: &Arc<language::Language>) -> bool {
    language.context_provider().is_some_and(|provider| provider.lsp_task_source().is_some())
}

/// The rows a syntax query for `visible` rows covers: one screen above and below,
/// so scrolling re-queries about once per screen.
fn query_rows(visible: &Range<BufferRow>, max_row: BufferRow) -> Range<BufferRow> {
    let margin = visible.end.saturating_sub(visible.start);
    visible.start.saturating_sub(margin)..visible.end.saturating_add(margin).min(max_row).saturating_add(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunnableTaskStatus {
    Running,
    Passed,
    Failed,
}

impl From<ScheduledTaskResult> for RunnableTaskStatus {
    fn from(result: ScheduledTaskResult) -> Self {
        match result {
            ScheduledTaskResult::Success => Self::Passed,
            ScheduledTaskResult::Failure | ScheduledTaskResult::SpawnFailed | ScheduledTaskResult::Cancelled => {
                Self::Failed
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum RunnableSource {
    /// Tasks the language server already built.
    Lsp(Vec<SpawnInTerminal>),
    /// A tree-sitter runnable; its language builds the tasks when the icon is clicked.
    Syntax { language: Arc<language::Language>, tags: SmallVec<[RunnableTag; 1]>, captures: FxHashMap<String, String> },
}

#[derive(Clone, Debug)]
pub(crate) struct RunnableTasks {
    pub source: RunnableSource,
    pub offset: Anchor,
    /// Column of the runnable on its row, where the tasks are built for.
    pub column: u32,
}

#[derive(Clone)]
pub struct ResolvedTasks {
    pub tasks: Vec<SpawnInTerminal>,
    pub position: Anchor,
}

impl Editor {
    pub fn refresh_runnables(
        &mut self,
        invalidate_buffer_data: Option<BufferId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.mode().is_full() || !self.enable_runnables {
            self.clear_runnables(None);
            return;
        }
        if let Some(buffer) = self.buffer().read(cx).as_singleton() {
            let buffer_read = buffer.read(cx);
            if buffer_read.file().is_none() {
                self.clear_runnables(None);
                return;
            }
            let buffer_id = buffer_read.buffer_id();
            let version = buffer_read.version();
            if invalidate_buffer_data != Some(buffer_id) {
                let visible_rows = self.visible_buffer_rows(cx);
                if self.runnables.covers(&version, &visible_rows) {
                    return;
                }
            }
        }
        if let Some(buffer_id) = invalidate_buffer_data {
            self.runnables.invalidate_buffer_data.insert(buffer_id);
        }

        let project = self.project().map(Entity::downgrade);
        let lsp_task_sources = self.lsp_task_sources(true, invalidate_buffer_data.is_none(), cx);
        self.runnables.runnables_update_task = cx.spawn_in(window, async move |editor, cx| {
            cx.background_executor().timer(UPDATE_DEBOUNCE).await;
            let Some(project) = project.and_then(|p| p.upgrade()) else {
                return;
            };

            let lsp_tasks = if lsp_task_sources.is_empty() {
                Vec::new()
            } else {
                let Ok(lsp_tasks) = cx.update(|_, cx| crate::lsp_tasks(project.clone(), &lsp_task_sources, None, cx))
                else {
                    return;
                };
                lsp_tasks.await
            };
            // Each language has one source of runnables: its language server when it
            // provides them (covering the whole buffer), else the syntax query, which
            // only runs over the rows around the viewport.
            let Some((multi_buffer_snapshot, multi_buffer_query_range, queried_rows)) = editor
                .update(cx, |editor, cx| {
                    let display_snapshot = editor.display_map.update(cx, |map, cx| map.snapshot(cx));
                    let visible_range = editor.multi_buffer_visible_range(&display_snapshot, cx);
                    let multi_buffer_snapshot = display_snapshot.buffer().clone();
                    let Some(buffer_snapshot) = multi_buffer_snapshot.as_singleton() else {
                        let query_range = visible_range.to_anchors(&multi_buffer_snapshot);
                        return (multi_buffer_snapshot, Some(query_range), None);
                    };
                    let version = buffer_snapshot.version().clone();
                    if buffer_snapshot.language().is_some_and(has_lsp_runnables) {
                        return (multi_buffer_snapshot, None, Some((version, 0..BufferRow::MAX)));
                    }
                    let rows = query_rows(
                        &(visible_range.start.row..visible_range.end.row),
                        multi_buffer_snapshot.max_point().row,
                    );
                    let query_range = (Point::new(rows.start, 0)
                        ..multi_buffer_snapshot.clip_point(Point::new(rows.end, 0), Bias::Left))
                        .to_anchors(&multi_buffer_snapshot);
                    (multi_buffer_snapshot, Some(query_range), Some((version, rows)))
                })
                .ok()
            else {
                return;
            };
            let new_rows = match multi_buffer_query_range {
                Some(query_range) => {
                    cx.background_spawn(async move {
                        multi_buffer_snapshot
                            .runnable_ranges(query_range)
                            .filter(|(_, runnable)| !has_lsp_runnables(&runnable.runnable.language))
                            .collect()
                    })
                    .await
                }
                None => Vec::new(),
            };

            let Ok(multi_buffer_snapshot) = editor.update(cx, |editor, cx| editor.buffer().read(cx).snapshot(cx))
            else {
                return;
            };
            let Ok(lsp_tasks_by_rows) = cx.update(|_, cx| {
                lsp_tasks.into_iter().fold(FxHashMap::default(), |mut acc, (location, task)| {
                    let buffer_snapshot = location.target.buffer.read(cx).snapshot();
                    if let Some(offset) = multi_buffer_snapshot.anchor_in_excerpt(location.target.range.start) {
                        let start = location.target.range.start.to_point(&buffer_snapshot);
                        let tasks = acc.entry((buffer_snapshot.buffer_id(), start.row)).or_insert_with(|| {
                            RunnableTasks { source: RunnableSource::Lsp(Vec::new()), offset, column: start.column }
                        });
                        if let RunnableSource::Lsp(tasks) = &mut tasks.source {
                            tasks.push(task);
                        }
                    }
                    acc
                })
            }) else {
                return;
            };

            let rows = Self::runnable_rows(&multi_buffer_snapshot, new_rows);
            editor
                .update(cx, |editor, cx| {
                    for buffer_id in std::mem::take(&mut editor.runnables.invalidate_buffer_data) {
                        editor.clear_runnables(Some(buffer_id));
                    }
                    editor.runnables.queried_rows = queried_rows;

                    // Each language has one source, so syntax and LSP rows never collide.
                    for ((buffer_id, row), new_tasks) in rows.into_iter().chain(lsp_tasks_by_rows) {
                        let Some(buffer) = editor.buffer().read(cx).buffer(buffer_id) else {
                            continue;
                        };
                        editor.insert_runnables(buffer_id, buffer.read(cx).version(), row, new_tasks);
                    }
                })
                .ok();
        });
    }

    /// Fetches runnables again for the buffers whose language server provides them.
    pub(crate) fn refresh_lsp_runnables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let buffer_ids = self.lsp_task_sources(true, false, cx).into_values().flatten().collect::<Vec<_>>();
        for buffer_id in buffer_ids {
            self.refresh_runnables(Some(buffer_id), window, cx);
        }
    }

    /// The buffer rows on screen, for a singleton editor.
    fn visible_buffer_rows(&self, cx: &mut Context<Self>) -> Range<BufferRow> {
        let display_snapshot = self.display_map.update(cx, |map, cx| map.snapshot(cx));
        let visible_range = self.multi_buffer_visible_range(&display_snapshot, cx);
        visible_range.start.row..visible_range.end.row
    }

    pub fn clear_runnables(&mut self, for_buffer: Option<BufferId>) {
        if let Some(buffer_id) = for_buffer {
            self.runnables.runnables.remove(&buffer_id);
            self.runnables.task_statuses.retain(|(status_buffer_id, _), _| *status_buffer_id != buffer_id);
        } else {
            self.runnables.runnables.clear();
            self.runnables.task_statuses.clear();
        }
        self.runnables.invalidate_buffer_data.clear();
        self.runnables.queried_rows = None;
        self.runnables.runnables_update_task = Task::ready(());
    }

    pub(crate) fn runnable_task_status(
        &self,
        buffer_id: BufferId,
        buffer_row: BufferRow,
    ) -> Option<RunnableTaskStatus> {
        self.runnables.task_status((buffer_id, buffer_row))
    }

    pub(crate) fn set_runnable_task_status(
        &mut self,
        buffer_id: BufferId,
        buffer_row: BufferRow,
        status: RunnableTaskStatus,
        cx: &mut Context<Self>,
    ) {
        self.runnables.set_task_status((buffer_id, buffer_row), status);
        cx.notify();
    }

    pub(crate) fn clear_runnable_task_status(
        &mut self,
        buffer_id: BufferId,
        buffer_row: BufferRow,
        cx: &mut Context<Self>,
    ) {
        self.runnables.clear_task_status((buffer_id, buffer_row));
        cx.notify();
    }

    pub(crate) fn runnable_task_key_for_display_row(
        &self,
        display_row: DisplayRow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(BufferId, BufferRow)> {
        let snapshot = self.snapshot(window, cx);
        let multibuffer_row = MultiBufferRow(DisplayPoint::new(display_row, 0).to_point(&snapshot).row);
        let (buffer_snapshot, range) = snapshot.buffer_snapshot().buffer_line_for_row(multibuffer_row)?;
        Some((buffer_snapshot.buffer_id(), range.start.row))
    }

    pub fn lsp_task_sources(
        &self,
        visible_only: bool,
        skip_cached: bool,
        cx: &mut Context<Self>,
    ) -> FxHashMap<LanguageServerName, Vec<BufferId>> {
        if !self.lsp_data_enabled() {
            return FxHashMap::default();
        }
        let buffers = if visible_only {
            self.visible_buffers(cx)
                .into_iter()
                .filter(|buffer| self.is_lsp_relevant(buffer.read(cx).file(), cx))
                .collect()
        } else {
            self.buffer().read(cx).all_buffers()
        };

        buffers
            .into_iter()
            .filter_map(|buffer| {
                let lsp_tasks_source = buffer.read(cx).language()?.context_provider()?.lsp_task_source()?;
                let buffer_id = buffer.read(cx).buffer_id();
                if skip_cached && self.runnables.has_cached(buffer_id, &buffer.read(cx).version()) {
                    None
                } else {
                    Some((lsp_tasks_source, buffer_id))
                }
            })
            .fold(FxHashMap::default(), |mut acc, (lsp_task_source, buffer_id)| {
                acc.entry(lsp_task_source).or_insert_with(Vec::new).push(buffer_id);
                acc
            })
    }

    pub(crate) fn render_run_indicator(
        &self,
        _style: &EditorStyle,
        is_active: bool,
        task_status: Option<RunnableTaskStatus>,
        row: DisplayRow,
        cx: &Context<Self>,
    ) -> IconButton {
        let (icon, color) = match task_status {
            Some(RunnableTaskStatus::Running) => (ui::IconName::PlayOutlined, Color::Accent),
            Some(RunnableTaskStatus::Passed) => (ui::IconName::Check, Color::Success),
            Some(RunnableTaskStatus::Failed) => (ui::IconName::XCircle, Color::Error),
            None => (ui::IconName::PlayOutlined, Color::Muted),
        };

        IconButton::new(("run_indicator", row.0 as usize), icon)
            .shape(ui::IconButtonShape::Square)
            .icon_size(IconSize::XSmall)
            .icon_color(color)
            .toggle_state(is_active)
            .on_click(cx.listener(move |editor, e: &ClickEvent, window, cx| {
                let quick_launch = match e {
                    ClickEvent::Keyboard(_) => true,
                    ClickEvent::Mouse(e) => e.down.button == MouseButton::Left,
                };

                window.focus(&editor.focus_handle(cx), cx);
                editor.toggle_code_actions(
                    &ToggleCodeActions { deployed_from: Some(CodeActionSource::RunMenu(row)), quick_launch },
                    window,
                    cx,
                );
            }))
            .on_right_click(cx.listener(move |editor, event: &ClickEvent, window, cx| {
                editor.set_gutter_context_menu(row, None, event.position(), window, cx);
            }))
    }

    fn insert_runnables(&mut self, buffer: BufferId, version: Global, row: BufferRow, new_tasks: RunnableTasks) {
        let (old_version, tasks) = self.runnables.runnables.entry(buffer).or_default();
        if !old_version.changed_since(&version) {
            *old_version = version;
            tasks.insert(row, new_tasks);
        }
    }

    fn runnable_rows(
        snapshot: &MultiBufferSnapshot,
        runnable_ranges: Vec<(Range<Anchor>, language::RunnableRange)>,
    ) -> Vec<((BufferId, BufferRow), RunnableTasks)> {
        runnable_ranges
            .into_iter()
            .filter(|(_, runnable)| !runnable.runnable.tags.is_empty())
            .filter_map(|(run_range, runnable)| {
                let point = run_range.start.to_point(snapshot);
                let (_, range) = snapshot.buffer_line_for_row(MultiBufferRow(point.row))?;
                let language::Runnable { tags, language, buffer } = runnable.runnable;
                Some((
                    (buffer, range.start.row),
                    RunnableTasks {
                        source: RunnableSource::Syntax { language, tags, captures: runnable.extra_captures },
                        offset: run_range.start,
                        column: point.column,
                    },
                ))
            })
            .collect()
    }

    pub(super) fn active_run_indicators(
        &self,
        range: Range<DisplayRow>,
        snapshot: &DisplaySnapshot,
        cx: &App,
    ) -> FxHashSet<DisplayRow> {
        if self.runnables.all_runnables().next().is_none() {
            return FxHashSet::default();
        }
        let offset_range_start = snapshot.display_point_to_point(DisplayPoint::new(range.start, 0), Bias::Left);

        let offset_range_end = snapshot.display_point_to_point(DisplayPoint::new(range.end, 0), Bias::Right);

        self.runnables
            .all_runnables()
            .filter_map(|tasks| {
                let multibuffer_point = tasks.offset.to_point(snapshot.buffer_snapshot());
                if multibuffer_point < offset_range_start || multibuffer_point > offset_range_end {
                    return None;
                }
                let multibuffer_row = MultiBufferRow(multibuffer_point.row);
                let buffer_folded = snapshot
                    .buffer_snapshot()
                    .buffer_line_for_row(multibuffer_row)
                    .map(|(buffer_snapshot, _)| buffer_snapshot.buffer_id())
                    .is_some_and(|buffer_id| self.is_buffer_folded(buffer_id, cx));
                if buffer_folded {
                    return None;
                }

                if snapshot.is_line_folded(multibuffer_row) {
                    // Skip folded indicators, unless it's the starting line of a fold.
                    if multibuffer_row
                        .0
                        .checked_sub(1)
                        .is_some_and(|previous_row| snapshot.is_line_folded(MultiBufferRow(previous_row)))
                    {
                        return None;
                    }
                }

                let display_row = multibuffer_point.to_display_point(snapshot).row();
                Some(display_row)
            })
            .collect()
    }

    /// The tasks a runnable offers, built by its language for a tree-sitter one.
    pub(super) fn runnable_tasks(
        project: &Entity<Project>,
        buffer: &Entity<Buffer>,
        buffer_row: u32,
        tasks: &RunnableTasks,
        cx: &App,
    ) -> Task<Vec<SpawnInTerminal>> {
        match &tasks.source {
            RunnableSource::Lsp(tasks) => Task::ready(tasks.clone()),
            RunnableSource::Syntax { language, tags, captures } => {
                let position = buffer.read(cx).anchor_at(Point::new(buffer_row, tasks.column), Bias::Right);
                project.read(cx).language_runnables(buffer, position, language, tags, captures, cx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_rows_adds_a_screen_each_side() {
        assert_eq!(query_rows(&(100..140), 1000), 60..181);
        // Clamped to the buffer.
        assert_eq!(query_rows(&(10..50), 60), 0..61);
        // Nothing laid out yet: only the top row.
        assert_eq!(query_rows(&(0..0), 1000), 0..1);
    }

    #[test]
    fn covers_needs_same_version_and_rows_inside() {
        let mut data = RunnableData::new();
        let mut version = Global::default();
        assert!(!data.covers(&version, &(0..10)));

        data.queried_rows = Some((version.clone(), 60..181));
        assert!(data.covers(&version, &(100..140)));
        assert!(data.covers(&version, &(60..181)));
        assert!(!data.covers(&version, &(50..90)));
        assert!(!data.covers(&version, &(150..190)));

        version.observe(clock::Lamport { value: 1 });
        assert!(!data.covers(&version, &(100..140)));
    }
}
