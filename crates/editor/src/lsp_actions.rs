use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt as _;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Focusable as _;
use gpui::FontWeight;
use gpui::HighlightStyle;
use gpui::ParentElement as _;
use gpui::Styled as _;
use gpui::Task;
use gpui::Window;
use gpui::div;
use gpui::prelude::InteractiveElement as _;
use gpui::prelude::IntoElement as _;
use itertools::Itertools as _;
use language::LanguageAwareStyling;
use language::OffsetRangeExt as _;
use language::Selection;
use lsp_types::CodeActionKind;
use multi_buffer::Anchor;
use multi_buffer::AnchorRangeExt as _;
use multi_buffer::MultiBufferOffset;
use multi_buffer::ToOffset as _;
use project::Project;
use project::lsp_store::LspFormatTarget;
use rustc_hash::FxHashSet;
use text::Bias;
use text::BufferId;
use util::TryFutureExt as _;
use workspace::Workspace;

use super::CODE_ACTION_TIMEOUT;
use super::Editor;
use super::EditorEvent;
use super::EditorStyle;
use super::FORMAT_TIMEOUT;
use super::FormatTarget;
use super::RenameState;
use super::RenameTarget;
use super::SelectionEffects;
use super::make_inlay_hints_style;
use crate::actions;
use crate::actions::ConfirmRename;
use crate::actions::Format;
use crate::actions::FormatSelections;
use crate::actions::OrganizeImports;
use crate::actions::Rename;
use crate::actions::RestartLanguageServer;
use crate::actions::SelectAll;
use crate::actions::StopLanguageServer;
use crate::display_map::BlockContext;
use crate::display_map::BlockPlacement;
use crate::display_map::BlockProperties;
use crate::display_map::BlockStyle;
use crate::display_map::HighlightKey;
use crate::display_map::ToDisplayPoint as _;
// lint: expect(module_cycle)
use crate::element::EditorElement;
use crate::movement;
use crate::scroll::Autoscroll;

impl Editor {
    /// Returns the anchor to use as the rename target for a selection.
    ///
    /// In selection-based modes, like vim's visual mode and helix, the rendered
    /// block cursor sits one position to the left of the selection's head,
    /// since the head is the exclusive end of a forward selection. Using the
    /// head
    /// directly would place the rename one character past the symbol, for
    /// example, trailing whitespace, so for a non-empty forward selection we
    /// shift one point left to land back on the symbol under the cursor.
    fn rename_target_anchor(&self, selection: &Selection<Anchor>, cx: &mut App) -> Anchor {
        let head = selection.head();

        if self.cursor_offset_on_selection && !selection.reversed && selection.start != selection.end {
            let display_map = self.display_snapshot(cx);
            let display_head = head.to_display_point(&display_map);

            if display_head.column() > 0 {
                return display_map.display_point_to_anchor(movement::left(&display_map, display_head), Bias::Left);
            }
        }
        head
    }

    pub(crate) fn rename(&self, _: &Rename, window: &Window, cx: &mut Context<Self>) -> Option<Task<Result<()>>> {
        use language::ToOffset as _;

        if self.read_only(cx) {
            return None;
        }
        let provider = self.semantics_provider.clone()?;
        let selection = *self.selections.newest_anchor();
        let cursor = self.rename_target_anchor(&selection, cx);
        let (cursor_buffer, cursor_buffer_position) = self.buffer.read(cx).text_anchor_for_position(cursor, cx)?;
        let (head_buffer, head_buffer_position) =
            self.buffer.read(cx).text_anchor_for_position(selection.head(), cx)?;
        let (tail_buffer, cursor_buffer_position_end) =
            self.buffer.read(cx).text_anchor_for_position(selection.tail(), cx)?;
        if tail_buffer != cursor_buffer || head_buffer != cursor_buffer {
            return None;
        }

        let snapshot = cursor_buffer.read(cx).snapshot();
        let cursor_buffer_offset = cursor_buffer_position.to_offset(&snapshot);
        let head_buffer_offset = head_buffer_position.to_offset(&snapshot);
        let cursor_buffer_offset_end = cursor_buffer_position_end.to_offset(&snapshot);
        let prepare_rename = provider.range_for_rename(&cursor_buffer, cursor_buffer_position, cx);
        drop(snapshot);

        Some(cx.spawn_in(window, async move |this, cx| {
            let rename_target = prepare_rename.await?;
            if let Some(RenameTarget { range: rename_range, language_server_id }) = rename_target {
                this.update_in(cx, |this, window, cx| {
                    let snapshot = cursor_buffer.read(cx).snapshot();
                    let rename_buffer_range = rename_range.to_offset(&snapshot);
                    let cursor_offset_in_rename_range = cursor_buffer_offset.saturating_sub(rename_buffer_range.start);
                    let head_offset_in_rename_range = head_buffer_offset.saturating_sub(rename_buffer_range.start);
                    let tail_offset_in_rename_range =
                        cursor_buffer_offset_end.saturating_sub(rename_buffer_range.start);

                    this.take_rename(false, window, cx);
                    let buffer = this.buffer.read(cx).read(cx);
                    let cursor_offset = cursor.to_offset(&buffer);
                    let rename_start = cursor_offset.saturating_sub_usize(cursor_offset_in_rename_range);
                    let rename_end = rename_start + rename_buffer_range.len();
                    let range = buffer.anchor_before(rename_start)..buffer.anchor_after(rename_end);
                    let mut old_highlight_id = None;
                    let old_name: Arc<str> = buffer
                        .chunks(rename_start..rename_end, LanguageAwareStyling { tree_sitter: true, diagnostics: true })
                        .map(|chunk| {
                            if old_highlight_id.is_none() {
                                old_highlight_id = chunk.syntax_highlight_id;
                            }
                            chunk.text
                        })
                        .collect::<String>()
                        .into();

                    drop(buffer);

                    // Position the selection in the rename editor so that it matches the current selection.
                    this.show_local_selections = false;
                    let rename_editor = cx.new(|cx| {
                        let mut editor = Editor::single_line(window, cx);
                        editor.buffer.update(cx, |buffer, cx| {
                            buffer.edit(
                                [(MultiBufferOffset(0)..MultiBufferOffset(0), Arc::clone(&old_name))],
                                None,
                                cx,
                            );
                        });
                        let head_offset_in_rename_range = MultiBufferOffset(head_offset_in_rename_range);
                        let tail_offset_in_rename_range = MultiBufferOffset(tail_offset_in_rename_range);
                        let rename_selection_range = match head_offset_in_rename_range.cmp(&tail_offset_in_rename_range)
                        {
                            Ordering::Equal => {
                                editor.select_all(&SelectAll, window, cx);
                                return editor;
                            }
                            Ordering::Less => head_offset_in_rename_range..tail_offset_in_rename_range,
                            Ordering::Greater => tail_offset_in_rename_range..head_offset_in_rename_range,
                        };
                        if rename_selection_range.end.0 > old_name.len() {
                            editor.select_all(&SelectAll, window, cx);
                        } else {
                            editor.change_selections(SelectionEffects::default(), window, cx, |s| {
                                s.select_ranges([rename_selection_range]);
                            });
                        }
                        editor
                    });
                    cx.subscribe(&rename_editor, |_, _, e: &EditorEvent, cx| {
                        if e == &EditorEvent::Focused {
                            cx.emit(EditorEvent::FocusedIn);
                        }
                    })
                    .detach();

                    let write_highlights = this.clear_background_highlights(HighlightKey::DocumentHighlightWrite, cx);
                    let read_highlights = this.clear_background_highlights(HighlightKey::DocumentHighlightRead, cx);
                    let ranges = write_highlights
                        .iter()
                        .flat_map(|(_, ranges)| ranges.iter())
                        .chain(read_highlights.iter().flat_map(|(_, ranges)| ranges.iter()))
                        .cloned()
                        .collect();

                    this.highlight_text(
                        HighlightKey::Rename,
                        ranges,
                        HighlightStyle { fade_out: Some(0.6), ..Default::default() },
                        cx,
                    );
                    let rename_focus_handle = rename_editor.focus_handle(cx);
                    window.focus(&rename_focus_handle, cx);
                    let block_id = this.insert_blocks(
                        [BlockProperties {
                            style: BlockStyle::Flex,
                            placement: BlockPlacement::Below(range.start),
                            height: Some(1),
                            render: Arc::new({
                                let rename_editor = rename_editor.clone();
                                move |cx: &mut BlockContext| {
                                    let mut text_style = cx.editor_style.text.clone();
                                    if let Some(highlight_style) =
                                        old_highlight_id.and_then(|h| cx.editor_style.syntax.get(h).copied())
                                    {
                                        text_style = text_style.highlight(highlight_style);
                                    }
                                    div()
                                        .block_mouse_except_scroll()
                                        .pl(cx.anchor_x)
                                        // lint: expect(module_cycle)
                                        .child(EditorElement::new(
                                            &rename_editor,
                                            EditorStyle {
                                                background: theme::theme().system().transparent,
                                                cursor_colors: cx.editor_style.cursor_colors,
                                                text: text_style,
                                                scrollbar_width: cx.editor_style.scrollbar_width,
                                                syntax: cx.editor_style.syntax,
                                                status: cx.editor_style.status.clone(),
                                                inlay_hints_style: HighlightStyle {
                                                    font_weight: Some(FontWeight::BOLD),
                                                    ..make_inlay_hints_style()
                                                },
                                                ..EditorStyle::default()
                                            },
                                        ))
                                        .into_any_element()
                                }
                            }),
                            priority: 0,
                        }],
                        Some(Autoscroll::fit()),
                        cx,
                    )[0];
                    this.pending_rename =
                        Some(RenameState { range, old_name, editor: rename_editor, language_server_id, block_id });
                })?;
            }

            Ok(())
        }))
    }

    pub(crate) fn confirm_rename(
        &mut self,
        _: &ConfirmRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        if self.read_only(cx) {
            return None;
        }
        let rename = self.take_rename(false, window, cx)?;
        let new_name = rename.editor.read(cx).text(cx);
        if new_name.trim().is_empty() {
            return Some(Task::ready(Ok(())));
        }

        let workspace = self.workspace()?.downgrade();
        let (buffer, start) = self.buffer.read(cx).text_anchor_for_position(rename.range.start, cx)?;
        let (end_buffer, _) = self.buffer.read(cx).text_anchor_for_position(rename.range.end, cx)?;
        if buffer != end_buffer {
            return None;
        }

        let old_name = rename.old_name;

        let rename = self.semantics_provider.as_ref()?.perform_rename(
            &buffer,
            start,
            new_name.clone(),
            rename.language_server_id,
            cx,
        )?;

        Some(cx.spawn_in(window, async move |editor, cx| {
            let project_transaction = rename.await?;
            Self::open_project_transaction(
                &editor,
                workspace,
                project_transaction,
                format!("Rename: {old_name} → {new_name}"),
                cx,
            )?;

            editor.update(cx, |editor, cx| {
                editor.refresh_document_highlights(cx);
            })?;
            Ok(())
        }))
    }

    pub(super) fn take_rename(
        &mut self,
        moving_cursor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<RenameState> {
        let rename = self.pending_rename.take()?;
        if rename.editor.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        self.remove_blocks(std::iter::once(rename.block_id).collect(), Some(Autoscroll::fit()), cx);
        self.clear_highlights(HighlightKey::Rename, cx);
        self.show_local_selections = true;

        if moving_cursor {
            let cursor_in_rename_editor = rename.editor.update(cx, |editor, cx| {
                editor.selections.newest::<MultiBufferOffset>(&editor.display_snapshot(cx)).head()
            });

            // Update the selection to match the position of the selection inside
            // the rename editor.
            let snapshot = self.buffer.read(cx).read(cx);
            let rename_range = rename.range.to_offset(&snapshot);
            let cursor_in_editor =
                snapshot.clip_offset(rename_range.start + cursor_in_rename_editor, Bias::Left).min(rename_range.end);
            drop(snapshot);

            self.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                s.select_ranges(vec![cursor_in_editor..cursor_in_editor]);
            });
        } else {
            self.refresh_document_highlights(cx);
        }

        Some(rename)
    }

    pub const fn pending_rename(&self) -> Option<&RenameState> {
        self.pending_rename.as_ref()
    }

    pub(super) fn can_format_selections(&self, cx: &App) -> bool {
        if !self.mode.is_full() {
            return false;
        }

        let Some(project) = &self.project else {
            return false;
        };

        let project = project.read(cx);
        let multi_buffer = self.buffer.read(cx);
        let snapshot = multi_buffer.snapshot(cx);

        self.selections
            .disjoint_anchor_ranges()
            .flat_map(|range| [range.start, range.end])
            .filter_map(|anchor| snapshot.anchor_to_buffer_anchor(anchor))
            .filter_map(|(_, buffer_snapshot)| multi_buffer.buffer(buffer_snapshot.buffer_id()))
            .any(|buffer| project.supports_range_formatting(&buffer, cx))
    }

    pub(super) fn format(&self, _: &Format, window: &Window, cx: &mut Context<Self>) -> Option<Task<Result<()>>> {
        if self.read_only(cx) {
            return None;
        }

        let project = self.project.clone()?;

        Some(self.perform_format(project, FormatTarget::Buffers(self.buffer.read(cx).all_buffers()), window, cx))
    }

    pub(super) fn format_selections(
        &self,
        _: &FormatSelections,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        if self.read_only(cx) {
            return None;
        }

        let project = self.project.clone()?;

        let ranges = self
            .selections
            .all_adjusted(&self.display_snapshot(cx))
            .into_iter()
            .map(|selection| selection.range())
            .collect_vec();

        Some(self.perform_format(project, FormatTarget::Ranges(ranges), window, cx))
    }

    pub(super) fn perform_format(
        &self,
        project: Entity<Project>,
        target: FormatTarget,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if self.read_only(cx) {
            return Task::ready(Ok(()));
        }
        let buffer = self.buffer.clone();
        let (mut buffers, target) = match target {
            FormatTarget::Buffers(buffers) => (buffers, LspFormatTarget::Buffers),
            FormatTarget::Ranges(selection_ranges) => {
                let multi_buffer = buffer.read(cx);
                let snapshot = multi_buffer.read(cx);
                let mut buffers = FxHashSet::default();
                let mut buffer_id_to_ranges: BTreeMap<BufferId, Vec<Range<text::Anchor>>> = BTreeMap::new();
                for selection_range in selection_ranges {
                    for (buffer_snapshot, buffer_range, _) in snapshot.range_to_buffer_ranges(selection_range) {
                        let buffer_id = buffer_snapshot.buffer_id();
                        let start = buffer_snapshot.anchor_before(buffer_range.start);
                        let end = buffer_snapshot.anchor_after(buffer_range.end);
                        buffers.insert(multi_buffer.buffer(buffer_id).expect("buffer is in multibuffer"));
                        buffer_id_to_ranges
                            .entry(buffer_id)
                            .and_modify(|buffer_ranges| buffer_ranges.push(start..end))
                            .or_insert_with(|| vec![start..end]);
                    }
                }
                (buffers, LspFormatTarget::Ranges(buffer_id_to_ranges))
            }
        };

        buffers.retain(|buffer| !buffer.read(cx).read_only());

        let transaction_id_prev = buffer.read(cx).last_transaction_id(cx);
        let (selections_prev, add_selections_state_prev) = transaction_id_prev
            .and_then(|transaction_id_prev| {
                // default to selections as they were after the last edit, if we have them,
                // instead of how they are now.
                // This will make it so that editing, moving somewhere else, formatting, then undoing the format
                // will take you back to where you made the last edit, instead of staying where you scrolled
                self.selection_history
                    .transaction(transaction_id_prev)
                    .map(|transaction| (Arc::clone(&transaction.undo), transaction.undo_add_selections_state.clone()))
            })
            .unwrap_or_else(|| (self.selections.disjoint_anchors_arc(), self.add_selections_state.clone()));

        let mut timeout = cx.background_executor().timer(FORMAT_TIMEOUT).fuse();
        let format = project.update(cx, |project, cx| project.format(buffers, target, true, cx));

        cx.spawn_in(window, async move |editor, cx| {
            let transaction = futures::select_biased! {
                transaction = format.log_err().fuse() => transaction,
                () = timeout => {
                    log::warn!("timed out waiting for formatting");
                    None
                }
            };

            buffer.update(cx, |buffer, cx| {
                if let Some(transaction) = transaction
                    && !buffer.is_singleton()
                {
                    buffer.push_transaction(&transaction.0, cx);
                }
                cx.notify();
            });

            if let Some(transaction_id_now) = buffer.read_with(cx, |b, cx| b.last_transaction_id(cx)) {
                let has_new_transaction = transaction_id_prev != Some(transaction_id_now);
                if has_new_transaction {
                    editor
                        .update(cx, |editor, _| {
                            editor.selection_history.insert_transaction(
                                transaction_id_now,
                                selections_prev,
                                add_selections_state_prev,
                            );
                        })
                        .ok();
                }
            }

            Ok(())
        })
    }

    pub(super) fn organize_imports(
        &self,
        _: &OrganizeImports,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<()>>> {
        if self.read_only(cx) {
            return None;
        }
        let project = self.project.clone()?;
        Some(self.perform_code_action_kind(project, CodeActionKind::SOURCE_ORGANIZE_IMPORTS, window, cx))
    }

    fn perform_code_action_kind(
        &self,
        project: Entity<Project>,
        kind: CodeActionKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let buffer = self.buffer.clone();
        let buffers = buffer.read(cx).all_buffers();
        let mut timeout = cx.background_executor().timer(CODE_ACTION_TIMEOUT).fuse();
        let apply_action = project.update(cx, |project, cx| project.apply_code_action_kind(buffers, kind, true, cx));
        cx.spawn_in(window, async move |_, cx| {
            let transaction = futures::select_biased! {
                () = timeout => {
                    log::warn!("timed out waiting for executing code action");
                    None
                }
                transaction = apply_action.log_err().fuse() => transaction,
            };
            buffer.update(cx, |buffer, cx| {
                // check if we need this
                if let Some(transaction) = transaction
                    && !buffer.is_singleton()
                {
                    buffer.push_transaction(&transaction.0, cx);
                }
                cx.notify();
            });
            Ok(())
        })
    }

    pub(super) fn restart_language_server(
        &mut self,
        _: &RestartLanguageServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project.clone() {
            self.buffer.update(cx, |multi_buffer, cx| {
                project.update(cx, |project, cx| {
                    project.restart_language_servers_for_buffers(
                        multi_buffer.all_buffers().into_iter().collect(),
                        FxHashSet::default(),
                        cx,
                    );
                });
            });
        }
    }

    pub(super) fn stop_language_server(&mut self, _: &StopLanguageServer, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.project.clone() {
            self.buffer.update(cx, |multi_buffer, cx| {
                project.update(cx, |project, cx| {
                    project.stop_language_servers_for_buffers(
                        multi_buffer.all_buffers().into_iter().collect(),
                        FxHashSet::default(),
                        cx,
                    );
                });
            });
        }
    }

    pub(super) fn cancel_language_server_work(
        workspace: &mut Workspace,
        _: &actions::CancelLanguageServerWork,
        _: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let project = workspace.project();
        let buffers = workspace
            .active_item(cx)
            .and_then(|item| item.act_as::<Editor>(cx))
            .map_or_else(FxHashSet::default, |editor| editor.read(cx).buffer.read(cx).all_buffers());
        project.update(cx, |project, cx| {
            project.cancel_language_server_work_for_buffers(buffers, cx);
        });
    }
}
