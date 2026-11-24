use std::collections::HashSet;
use std::ops::Neg as _;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use editor::items::entry_diagnostic_aware_icon_decoration_and_color;
use editor::items::entry_diagnostic_aware_icon_name_and_color;
use editor::items::entry_git_aware_label_color;
use file_icons::FileIcons;
use git::status::GitSummary;
use gpui::Action as _;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext as _;
use gpui::Bounds;
use gpui::Context;
use gpui::Div;
use gpui::DragMoveEvent;
use gpui::Focusable as _;
use gpui::Hsla;
use gpui::InteractiveElement as _;
use gpui::ListHorizontalSizingBehavior;
use gpui::ListSizingBehavior;
use gpui::ModifiersChangedEvent;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseExitEvent;
use gpui::ParentElement as _;
use gpui::Pixels;
use gpui::Point;
use gpui::Render;
use gpui::ScrollStrategy;
use gpui::Stateful;
use gpui::Styled as _;
use gpui::Window;
use gpui::anchored;
use gpui::deferred;
use gpui::div;
use gpui::hsla;
use gpui::linear_color_stop;
use gpui::linear_gradient;
use gpui::point;
use gpui::px;
use gpui::size;
use gpui::transparent_white;
use gpui::uniform_list;
use project::Entry;
use project::EntryKind;
use project::ProjectEntryId;
use project::Worktree;
use project::WorktreeId;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use ui::DecoratedIcon;
use ui::IconDecoration;
use ui::IconDecorationKind;
use ui::IndentGuideColors;
use ui::IndentGuideLayout;
use ui::KeyBinding;
use ui::ListItem;
use ui::ListItemSpacing;
use ui::ProjectEmptyState;
use ui::ScrollAxes;
use ui::ScrollableHandle as _;
use ui::Scrollbars;
use ui::StickyCandidate;
use ui::Tooltip;
use ui::WithScrollbar as _;
use ui::font;
use ui::prelude::Color;
use ui::prelude::Element as _;
use ui::prelude::ElementId;
use ui::prelude::FluentBuilder as _;
use ui::prelude::Icon;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::IntoElement;
use ui::prelude::Label;
use ui::prelude::LabelCommon as _;
use ui::prelude::LabelSize;
use ui::prelude::SharedString;
use ui::prelude::StatefulInteractiveElement as _;
use ui::prelude::h_flex;
use ui::prelude::v_flex;
use util::ResultExt as _;
use util::paths::PathExt as _;
use util::rel_path::RelPath;
use workspace::DraggedSelection;
use workspace::SelectedEntry;

use super::ContextMenuPlacement;
use super::DiagnosticMark;
use super::DragTarget;
use super::DraggedProjectEntryView;
use super::EntryDetails;
use super::FoldedAncestors;
use super::FoldedDirectoryDragTarget;
use super::INDENT_SIZE;
use super::NewFile;
use super::ProjectPanel;
use super::StickyDetails;
use super::TITLE_TOOLTIP_DELAY;
use super::ValidationState;
use super::entry_file_name;
use super::last_entry_in_subtree;

impl ProjectPanel {
    fn calculate_depth_and_difference(
        entry: &Entry,
        visible_worktree_entries: &HashSet<Arc<RelPath>>,
    ) -> (usize, usize) {
        let (depth, difference) = entry
            .path
            .ancestors()
            .skip(1) // Skip the entry itself
            .find_map(|ancestor| {
                if let Some(parent_entry) = visible_worktree_entries.get(ancestor) {
                    let entry_path_components_count = entry.path.components().count();
                    let parent_path_components_count = parent_entry.components().count();
                    let difference = entry_path_components_count - parent_path_components_count;
                    let depth = parent_entry
                        .ancestors()
                        .skip(1)
                        .filter(|ancestor| visible_worktree_entries.contains(*ancestor))
                        .count();
                    Some((depth + 1, difference))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| (0, entry.path.components().count()));

        (depth, difference)
    }

    fn highlight_entry_for_selection_drag(
        &self,
        target_entry: &Entry,
        target_worktree: &Worktree,
        drag_state: &DraggedSelection,
        cx: &Context<Self>,
    ) -> Option<ProjectEntryId> {
        let target_parent_path = target_entry.path.parent();

        // In case of single item drag, we do not highlight existing
        // directory which item belongs too
        if drag_state.items().count() == 1 && drag_state.active_selection.worktree_id == target_worktree.id() {
            let active_entry_path = self.project.read(cx).path_for_entry(drag_state.active_selection.entry_id, cx)?;

            if let Some(active_parent_path) = active_entry_path.path.parent() {
                // Do not highlight active entry parent
                if active_parent_path == target_entry.path.as_ref() {
                    return None;
                }

                // Do not highlight active entry sibling files
                if Some(active_parent_path) == target_parent_path && target_entry.is_file() {
                    return None;
                }
            }
        }

        // Always highlight directory or parent directory if it's file
        if target_entry.is_dir() {
            Some(target_entry.id)
        } else {
            target_parent_path
                .and_then(|parent_path| target_worktree.entry_for_path(parent_path))
                .map(|parent_entry| parent_entry.id)
        }
    }

    fn should_highlight_background_for_selection_drag(
        &self,
        drag_state: &DraggedSelection,
        last_root_id: ProjectEntryId,
        cx: &App,
    ) -> bool {
        // Always highlight for multiple entries
        if drag_state.items().count() > 1 {
            return true;
        }

        // Since root will always have empty relative path
        if let Some(entry_path) = self.project.read(cx).path_for_entry(drag_state.active_selection.entry_id, cx)
            && let Some(parent_path) = entry_path.path.parent()
            && !parent_path.is_empty()
        {
            return true;
        }

        // If parent is empty, check if different worktree
        if let Some(last_root_worktree_id) = self.project.read(cx).worktree_id_for_entry(last_root_id, cx)
            && drag_state.active_selection.worktree_id != last_root_worktree_id
        {
            return true;
        }

        false
    }

    fn render_entry(
        &self,
        entry_id: ProjectEntryId,
        details: EntryDetails,
        marked_selections: Arc<[SelectedEntry]>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        const GROUP_NAME: &str = "project_entry";

        let kind = details.kind;
        let is_sticky = details.sticky.is_some();
        let sticky_index = details.sticky.as_ref().map(|this| this.sticky_index);
        let show_editor = details.is_editing && !details.is_processing;

        let selection = SelectedEntry { worktree_id: details.worktree_id, entry_id };

        let is_marked = self.marked_entries.contains(&selection);
        let is_active = self.selection.is_some_and(|selection| selection.entry_id == entry_id);

        let file_name = details.filename.clone();

        let mut icon = details.icon.clone();
        if show_editor && details.kind.is_file() {
            let filename = self.filename_editor.read(cx).text(cx);
            if filename.len() > 2 {
                icon = FileIcons::get_icon(Path::new(&filename), cx);
            }
        }

        let filename_text_color = details.filename_text_color;
        let diagnostic_severity = details.diagnostic_severity;
        let diagnostic_mark = details.diagnostic_mark;
        let diagnostic_count = details.diagnostic_count;
        let item_colors = get_item_color(is_sticky);

        let canonical_path = details.canonical_path.clone();
        let path = Arc::clone(&details.path);

        let depth = details.depth;
        let worktree_id = details.worktree_id;

        let bg_color = if is_marked { item_colors.marked } else { item_colors.default };

        let bg_hover_color = if is_marked { item_colors.marked } else { item_colors.hover };

        let validation_color_and_message = if show_editor {
            match self.state.edit_state.as_ref().map_or(ValidationState::None, |e| e.validation_state.clone()) {
                ValidationState::Error(msg) => Some((Color::Error.color(), msg)),
                ValidationState::Warning(msg) => Some((Color::Warning.color(), msg)),
                ValidationState::None => None,
            }
        } else {
            None
        };

        let show_focused_border = !self.mouse_down && is_active && self.contains_focus(window, cx);

        let border_color = if show_focused_border {
            match validation_color_and_message {
                Some((color, _)) => color,
                None => item_colors.focused,
            }
        } else {
            bg_color
        };

        let border_hover_color = if show_focused_border {
            match validation_color_and_message {
                Some((color, _)) => color,
                None => item_colors.focused,
            }
        } else {
            bg_hover_color
        };

        let folded_directory_drag_target = self.folded_directory_drag_target;
        let highlight_entry_id = self.drag_highlight_entry_id();
        let is_highlighted = {
            if let Some(highlight_entry_id) = highlight_entry_id {
                // Highlight if same entry or it's children
                if entry_id == highlight_entry_id {
                    true
                } else {
                    (|| {
                        let worktree = self.project.read(cx).worktree_for_id(worktree_id, cx)?;
                        let highlight_entry = worktree.read(cx).entry_for_id(highlight_entry_id)?;
                        Some(path.starts_with(&highlight_entry.path))
                    })()
                    .unwrap_or(false)
                }
            } else {
                false
            }
        };
        let show_drag_highlight = is_highlighted && !is_sticky && folded_directory_drag_target.is_none();

        let id: ElementId = if is_sticky {
            SharedString::from(format!("project_panel_sticky_item_{}", entry_id.to_usize())).into()
        } else {
            entry_id.to_usize().into()
        };

        let tooltip_path = self
            .project
            .read(cx)
            .worktree_for_id(worktree_id, cx)
            .map(|worktree| worktree.read(cx).absolutize(&path).compact().to_string_lossy().into_owned());

        div()
            .id(id.clone())
            .relative()
            .group(GROUP_NAME)
            .when_some(tooltip_path, |this, path| {
                this.tooltip_show_delay(TITLE_TOOLTIP_DELAY).tooltip(Tooltip::text(path))
            })
            .cursor_pointer()
            .rounded_none()
            .bg(bg_color)
            .border_1()
            .border_r_2()
            .border_color(border_color)
            .hover(|style| style.bg(bg_hover_color).border_color(border_hover_color))
            .when(is_sticky, gpui::InteractiveElement::block_mouse_except_scroll)
            .when(!is_sticky, |this| {
                this.when(show_drag_highlight, |this| this.border_color(transparent_white()).bg(transparent_white()))
                    .map(|this| {
                        let path_for_dragged_selection = Arc::clone(&path);
                        let dragged_selection = DraggedSelection {
                            active_selection: selection,
                            marked_selections: Arc::clone(&marked_selections),
                        };

                        this.on_drag_move::<DraggedSelection>(cx.listener(
                            move |this, event: &DragMoveEvent<DraggedSelection>, window, cx| {
                                let is_current_target = this.drag_target_entry.as_ref().and_then(|entry| match entry {
                                    DragTarget::Entry { entry_id: target_id, .. } => Some(*target_id),
                                    DragTarget::Background => None,
                                }) == Some(entry_id);

                                if !event.bounds.contains(&event.event.position) {
                                    // Entry responsible for setting drag target is also responsible to
                                    // clear it up after drag is out of bounds
                                    if is_current_target {
                                        this.drag_target_entry = None;
                                    }
                                    return;
                                }

                                if is_current_target {
                                    return;
                                }

                                let drag_state = event.drag(cx);

                                if drag_state.items().count() == 1 {
                                    this.marked_entries.clear();
                                    this.marked_entries.push(drag_state.active_selection);
                                }

                                let Some((entry_id, highlight_entry_id)) = (|| {
                                    let target_worktree =
                                        this.project.read(cx).worktree_for_id(selection.worktree_id, cx)?.read(cx);
                                    let target_entry = target_worktree.entry_for_path(&path_for_dragged_selection)?;
                                    let highlight_entry_id = this.highlight_entry_for_selection_drag(
                                        target_entry,
                                        target_worktree,
                                        drag_state,
                                        cx,
                                    )?;
                                    Some((target_entry.id, highlight_entry_id))
                                })() else {
                                    return;
                                };

                                this.drag_target_entry = Some(DragTarget::Entry { entry_id, highlight_entry_id });

                                this.hover_expand_task.take();

                                if !kind.is_dir()
                                    || this
                                        .state
                                        .expanded_dir_ids
                                        .get(&details.worktree_id)
                                        .is_some_and(|ids| ids.binary_search(&entry_id).is_ok())
                                {
                                    return;
                                }

                                let bounds = event.bounds;
                                this.hover_expand_task = Some(cx.spawn_in(window, async move |this, cx| {
                                    cx.background_executor().timer(Duration::from_millis(500)).await;
                                    this.update_in(cx, |this, window, cx| {
                                        this.hover_expand_task.take();
                                        if this.drag_target_entry.as_ref().and_then(|entry| match entry {
                                            DragTarget::Entry { entry_id: target_id, .. } => Some(*target_id),
                                            DragTarget::Background => None,
                                        }) == Some(entry_id)
                                            && bounds.contains(&window.mouse_position())
                                        {
                                            this.expand_entry(worktree_id, entry_id, cx);
                                            this.update_visible_entries(
                                                Some((worktree_id, entry_id)),
                                                false,
                                                false,
                                                window,
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                }));
                            },
                        ))
                        .on_drag(dragged_selection, {
                            let active_component = self
                                .state
                                .ancestors
                                .get(&entry_id)
                                .and_then(|ancestors| ancestors.active_component(&details.filename));
                            move |selection, click_offset, _window, cx| {
                                let filename = active_component.as_ref().unwrap_or(&details.filename);
                                cx.new(|_| DraggedProjectEntryView {
                                    icon: details.icon.clone(),
                                    filename: filename.clone(),
                                    click_offset,
                                    selection: selection.active_selection,
                                    selections: Arc::clone(&selection.marked_selections),
                                })
                            }
                        })
                        .on_drop(cx.listener(
                            move |this, selections: &DraggedSelection, window, cx| {
                                this.clear_drag_state(cx);
                                if folded_directory_drag_target.is_some() {
                                    return;
                                }
                                this.drag_onto(selections, entry_id, kind.is_file(), window, cx);
                            },
                        ))
                    })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.mouse_down = true;
                    cx.propagate();
                }),
            )
            .on_click(cx.listener(move |project_panel, event: &gpui::ClickEvent, window, cx| {
                if event.is_right_click() || show_editor {
                    return;
                }
                if event.standard_click() {
                    project_panel.mouse_down = false;
                }
                cx.stop_propagation();

                if let Some(selection) = project_panel.selection.filter(|_| event.modifiers().shift) {
                    let current_selection = project_panel.index_for_selection(selection);
                    let clicked_entry = SelectedEntry { entry_id, worktree_id };
                    let target_selection = project_panel.index_for_selection(clicked_entry);
                    if let Some(((_, _, source_index), (_, _, target_index))) = current_selection.zip(target_selection)
                    {
                        let range_start = source_index.min(target_index);
                        let range_end = source_index.max(target_index) + 1;
                        let mut new_selections = Vec::new();
                        project_panel.for_each_visible_entry(
                            range_start..range_end,
                            window,
                            cx,
                            &mut |entry_id, details, _, _| {
                                new_selections.push(SelectedEntry { entry_id, worktree_id: details.worktree_id });
                            },
                        );

                        for new_selection in &new_selections {
                            if !project_panel.marked_entries.contains(new_selection) {
                                project_panel.marked_entries.push(*new_selection);
                            }

                            let is_intermediate_entry = new_selection.entry_id != selection.entry_id
                                && new_selection.entry_id != clicked_entry.entry_id;

                            if is_intermediate_entry {
                                project_panel.select_outermost_folded_ancestor(new_selection.entry_id);
                            }
                        }

                        project_panel.selection = Some(clicked_entry);
                        if !project_panel.marked_entries.contains(&clicked_entry) {
                            project_panel.marked_entries.push(clicked_entry);
                        }
                    }
                } else if event.modifiers().secondary() {
                    if event.click_count() > 1 {
                        project_panel.split_entry(entry_id, false, None, cx);
                    } else {
                        project_panel.selection = Some(selection);
                        if let Some(position) = project_panel.marked_entries.iter().position(|e| *e == selection) {
                            project_panel.marked_entries.remove(position);
                        } else {
                            project_panel.marked_entries.push(selection);
                        }
                    }
                } else if kind.is_dir() {
                    project_panel.marked_entries.clear();
                    if is_sticky && let Some((_, _, index)) = project_panel.index_for_entry(entry_id, worktree_id) {
                        project_panel.scroll_handle.scroll_to_item_strict_with_offset(
                            index,
                            ScrollStrategy::Top,
                            sticky_index.unwrap_or(0),
                        );
                        cx.notify();
                        // move down by 1px so that clicked item
                        // don't count as sticky anymore
                        cx.on_next_frame(window, |_, window, cx| {
                            cx.on_next_frame(window, |this, _, cx| {
                                let mut offset = this.scroll_handle.offset();
                                offset.y += px(1.);
                                this.scroll_handle.set_offset(offset);
                                cx.notify();
                            });
                        });
                        return;
                    }
                    if event.modifiers().alt {
                        project_panel.toggle_expand_all(entry_id, window, cx);
                    } else {
                        project_panel.toggle_expanded(entry_id, window, cx);
                    }
                } else {
                    let click_count = event.click_count();
                    let focus_opened_item = click_count > 1;
                    let allow_preview = click_count == 1;
                    project_panel.open_entry(entry_id, focus_opened_item, allow_preview, cx);
                }
            }))
            .on_aux_click(cx.listener(move |project_panel, event: &gpui::ClickEvent, _, cx| {
                if !event.is_middle_click() || show_editor || !kind.is_file() {
                    return;
                }

                project_panel.open_entry(entry_id, true, false, cx);
                cx.stop_propagation();
            }))
            .child(
                ListItem::new(id)
                    .indent_level(depth)
                    .indent_step_size(INDENT_SIZE)
                    .spacing(ListItemSpacing::Dense)
                    .selectable(false)
                    .when(canonical_path.is_some() || diagnostic_count.is_some(), |this| {
                        let symlink_element = canonical_path.map(|path| {
                            div()
                                .id("symlink_icon")
                                .tooltip(move |_window, cx| {
                                    Tooltip::with_meta(path.to_string_lossy().into_owned(), None, "Symbolic Link", cx)
                                })
                                .child(
                                    Icon::new(IconName::ArrowUpRight)
                                        .size(IconSize::Indicator)
                                        .color(filename_text_color),
                                )
                        });
                        this.end_slot::<AnyElement>(
                            h_flex()
                                .gap_1()
                                .flex_none()
                                .pr_3()
                                .when_some(diagnostic_count, |this, count| {
                                    this.when(count.error_count > 0, |this| {
                                        this.child(
                                            Label::new(count.capped_error_count())
                                                .size(LabelSize::Small)
                                                .color(Color::Error),
                                        )
                                    })
                                    .when(count.warning_count > 0, |this| {
                                        this.child(
                                            Label::new(count.capped_warning_count())
                                                .size(LabelSize::Small)
                                                .color(Color::Warning),
                                        )
                                    })
                                })
                                .when_some(symlink_element, gpui::ParentElement::child)
                                .into_any_element(),
                        )
                    })
                    .map(|this| {
                        let decoration_color = entry_diagnostic_aware_icon_decoration_and_color(diagnostic_severity)
                            .map(|(_, color)| color);
                        let decorated = |glyph: SharedString, decoration_kind: IconDecorationKind, color: Color| {
                            DecoratedIcon::new(
                                Icon::from_path(glyph).color(Color::Muted),
                                Some(
                                    IconDecoration::new(decoration_kind, bg_color)
                                        .group_name(Some(GROUP_NAME.into()))
                                        .knockout_hover_color(bg_hover_color)
                                        .color(color.color())
                                        .position(Point { x: px(-2.), y: px(-2.) }),
                                ),
                            )
                            .into_any_element()
                        };

                        this.child(if let Some(icon) = &icon {
                            match diagnostic_mark.zip(decoration_color) {
                                Some((DiagnosticMark::OnIcon(decoration_kind), color)) => {
                                    div().child(decorated(icon.clone(), decoration_kind, color))
                                }
                                _ => h_flex().child(Icon::from_path(icon.to_string()).color(Color::Muted)),
                            }
                        } else if let Some(DiagnosticMark::Standalone(icon_name)) = diagnostic_mark {
                            let color = entry_diagnostic_aware_icon_name_and_color(diagnostic_severity)
                                .map_or(Color::Error, |(_, color)| color);
                            h_flex()
                                .size(IconSize::default().rems())
                                .child(Icon::new(icon_name).color(color).size(IconSize::Small))
                        } else {
                            h_flex().size(IconSize::default().rems()).invisible().flex_none()
                        })
                    })
                    .child(if show_editor {
                        h_flex().h_6().w_full().child(self.filename_editor.clone())
                    } else {
                        h_flex().h_6().map(|this| match self.state.ancestors.get(&entry_id) {
                            Some(folded_ancestors) => this.children(self.render_folder_elements(
                                folded_ancestors,
                                entry_id,
                                file_name,
                                is_sticky,
                                kind.is_file(),
                                is_active || is_marked,
                                item_colors.drag_over,
                                folded_directory_drag_target,
                                filename_text_color,
                                cx,
                            )),

                            None => this.child(
                                Label::new(file_name).single_line().color(filename_text_color).into_any_element(),
                            ),
                        })
                    })
                    .on_secondary_mouse_down(cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        // Stop propagation to prevent the catch-all context menu for the project
                        // panel from being deployed.
                        cx.stop_propagation();
                        // Some context menu actions apply to all marked entries. If the user
                        // right-clicks on an entry that is not marked, they may not realize the
                        // action applies to multiple entries. To avoid inadvertent changes, all
                        // entries are unmarked.
                        if !this.marked_entries.contains(&selection) {
                            this.marked_entries.clear();
                        }
                        this.deploy_context_menu(ContextMenuPlacement::AtMouse(event.position), entry_id, window, cx);
                    }))
                    .overflow_x(),
            )
            .when_some(validation_color_and_message, |this, (color, message)| {
                this.relative().child(deferred(
                    div()
                        .occlude()
                        .absolute()
                        .top_full()
                        .left(px(-1.)) // Used px over rem so that it doesn't change with font size
                        .right(px(-0.5))
                        .py_1()
                        .px_2()
                        .border_1()
                        .border_color(color)
                        .bg(theme::theme().colors().background)
                        .child(Label::new(message).color(Color::from(color)).size(LabelSize::Small)),
                ))
            })
            .when(show_drag_highlight, |this| {
                // One rounded box around the whole target subtree: each row draws the sides, the
                // target row the top and its last visible descendant the bottom. Offsets
                // cover the row's own border.
                this.child(
                    div()
                        .absolute()
                        .top(px(-1.))
                        .bottom(px(-1.))
                        .left(px(-1.))
                        .right(px(-2.))
                        .bg(item_colors.drag_over)
                        .border_l_1()
                        .border_r_1()
                        .when(highlight_entry_id == Some(entry_id), |this| {
                            this.border_t_1().rounded_t(workspace::CARD_RADIUS)
                        })
                        .when(self.drag_highlight_last_entry_id == Some(entry_id), |this| {
                            this.border_b_1().rounded_b(workspace::CARD_RADIUS)
                        })
                        .border_color(item_colors.drag_over_border),
                )
            })
    }

    fn drag_highlight_entry_id(&self) -> Option<ProjectEntryId> {
        let drag_target = self.drag_target_entry.as_ref()?;
        match drag_target {
            DragTarget::Entry { highlight_entry_id, .. } => Some(*highlight_entry_id),
            DragTarget::Background => self.state.last_worktree_root_id,
        }
    }

    fn find_drag_highlight_last_entry_id(&self) -> Option<ProjectEntryId> {
        let highlight_entry_id = self.drag_highlight_entry_id()?;
        self.state.visible_entries.iter().find_map(|visible| {
            last_entry_in_subtree(
                visible.entries.iter().map(|entry| (entry.id, entry.path.as_ref())),
                highlight_entry_id,
            )
        })
    }

    fn render_folder_elements(
        &self,
        folded_ancestors: &FoldedAncestors,
        entry_id: ProjectEntryId,
        file_name: String,
        is_sticky: bool,
        is_file: bool,
        is_active_or_marked: bool,
        drag_over_color: Hsla,
        folded_directory_drag_target: Option<FoldedDirectoryDragTarget>,
        filename_text_color: Color,
        cx: &Context<Self>,
    ) -> impl Iterator<Item = AnyElement> {
        let components = Path::new(&file_name)
            .components()
            .map(|comp| comp.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let active_index = folded_ancestors.active_index();
        let components_len = components.len();
        let delimiter = SharedString::new("/");

        let path_component_elements = components.into_iter().enumerate().map(move |(index, component)| {
            div()
                .id(SharedString::from(format!("project_panel_path_component_{}_{index}", entry_id.to_usize())))
                .when(index == 0, gpui::Styled::ml_neg_0p5)
                .px_0p5()
                .rounded_xs()
                .hover(|style| style.bg(theme::theme().colors().element_active))
                .when(!is_sticky, |div| {
                    div.when(index != components_len - 1, |div| {
                        let target_entry_id = folded_ancestors.ancestors.get(components_len - 1 - index).copied();
                        let drag_target = FoldedDirectoryDragTarget { entry_id, index, is_delimiter_target: false };
                        div.map(|div| {
                            div.on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedSelection>, _, _| {
                                this.update_folded_directory_drag_target(
                                    event.bounds.contains(&event.event.position),
                                    drag_target,
                                );
                            }))
                            .on_drop(cx.listener(move |this, selections: &DraggedSelection, window, cx| {
                                this.clear_drag_state(cx);
                                if let Some(target_entry_id) = target_entry_id {
                                    this.drag_onto(selections, target_entry_id, is_file, window, cx);
                                }
                            }))
                            .when(
                                folded_directory_drag_target
                                    .is_some_and(|target| target.entry_id == entry_id && target.index == index),
                                |this| this.bg(drag_over_color),
                            )
                        })
                    })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(folds) = this.state.ancestors.get_mut(&entry_id)
                            && folds.set_active_index(index)
                        {
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(folds) = this.state.ancestors.get_mut(&entry_id)
                            && folds.set_active_index(index)
                        {
                            cx.notify();
                        }
                    }),
                )
                .child(
                    Label::new(component)
                        .single_line()
                        .color(filename_text_color)
                        .when(index == active_index && is_active_or_marked, ui::LabelCommon::underline),
                )
                .into_any()
        });

        let mut separator_index = 0;
        itertools::intersperse_with(path_component_elements, move || {
            separator_index += 1;
            self.render_entry_path_separator(
                entry_id,
                separator_index,
                components_len,
                is_sticky,
                is_file,
                filename_text_color,
                &delimiter,
                folded_ancestors,
                cx,
            )
            .into_any()
        })
    }

    fn render_entry_path_separator(
        &self,
        entry_id: ProjectEntryId,
        index: usize,
        components_len: usize,
        is_sticky: bool,
        is_file: bool,
        filename_text_color: Color,
        delimiter: &SharedString,
        folded_ancestors: &FoldedAncestors,
        cx: &Context<Self>,
    ) -> Div {
        let delimiter_target_index = index - 1;
        let target_entry_id = folded_ancestors.ancestors.get(components_len - 1 - delimiter_target_index).copied();
        let drag_target =
            FoldedDirectoryDragTarget { entry_id, index: delimiter_target_index, is_delimiter_target: true };
        div()
            .when(!is_sticky, |div| {
                div.map(|div| {
                    div.on_drop(cx.listener(move |this, selections: &DraggedSelection, window, cx| {
                        this.clear_drag_state(cx);
                        if let Some(target_entry_id) = target_entry_id {
                            this.drag_onto(selections, target_entry_id, is_file, window, cx);
                        }
                    }))
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedSelection>, _, _| {
                            this.update_folded_directory_drag_target(
                                event.bounds.contains(&event.event.position),
                                drag_target,
                            );
                        },
                    ))
                })
            })
            .child(Label::new(delimiter.clone()).single_line().color(filename_text_color))
    }

    pub(super) fn entry_icon(entry: &Entry, is_expanded: bool, cx: &App) -> Option<SharedString> {
        match entry.kind {
            EntryKind::File => FileIcons::get_icon(entry.path.as_std_path(), cx),
            _ => FileIcons::get_folder_icon(is_expanded, entry.path.as_std_path(), cx),
        }
    }

    pub(super) fn details_for_entry(
        &self,
        entry: &Entry,
        worktree_id: WorktreeId,
        root_name: &RelPath,
        entries_paths: &HashSet<Arc<RelPath>>,
        git_status: GitSummary,
        sticky: Option<StickyDetails>,
        _window: &mut Window,
        cx: &Context<Self>,
    ) -> EntryDetails {
        let expanded_entry_ids = self.state.expanded_dir_ids.get(&worktree_id).map_or(&[][..], Vec::as_slice);
        let is_expanded = expanded_entry_ids.binary_search(&entry.id).is_ok();

        let mut icon = Self::entry_icon(entry, is_expanded, cx);
        let (depth, difference) = ProjectPanel::calculate_depth_and_difference(entry, entries_paths);
        let depth = depth
            + self
                .state
                .visible_entries
                .iter()
                .find(|visible| visible.worktree_id == worktree_id)
                .map_or(0, |visible| visible.depth_offset);

        let library_name = if entry.path.is_empty() {
            self.project
                .read(cx)
                .worktree_for_id(worktree_id, cx)
                .and_then(|worktree| worktree.read(cx).library_name().cloned())
        } else {
            None
        };
        if library_name.is_some() {
            icon = Some(IconName::Box.path().into());
        }

        let filename = if let Some(library_name) = library_name {
            library_name.to_string()
        } else if difference > 1 {
            entry.path.last_n_components(difference).map_or(String::new(), |suffix| suffix.display().to_owned())
        } else {
            entry_file_name(entry, root_name).to_owned()
        };

        let selection = SelectedEntry { worktree_id, entry_id: entry.id };
        let is_marked = self.marked_entries.contains(&selection);
        let is_selected = self.selection == Some(selection);

        let diagnostic_severity = self.diagnostics.get(&(worktree_id, Arc::clone(&entry.path))).copied();

        let diagnostic_count = self.diagnostic_counts.get(&(worktree_id, Arc::clone(&entry.path))).copied();

        let diagnostic_mark = if icon.is_some() {
            entry_diagnostic_aware_icon_decoration_and_color(diagnostic_severity).map(|(kind, _)| {
                DiagnosticMark::OnIcon(if entry.kind.is_file() { kind } else { IconDecorationKind::Dot })
            })
        } else {
            entry_diagnostic_aware_icon_name_and_color(diagnostic_severity)
                .map(|(name, _)| DiagnosticMark::Standalone(name))
        };

        let filename_text_color = entry_git_aware_label_color(git_status, entry.is_ignored, is_marked);

        let is_cut = self.clipboard.as_ref().is_some_and(|e| e.is_cut() && e.items().contains(&selection));

        EntryDetails {
            filename,
            icon,
            path: Arc::clone(&entry.path),
            depth,
            kind: entry.kind,
            is_ignored: entry.is_ignored,
            is_expanded,
            is_selected,
            is_marked,
            is_editing: false,
            is_processing: false,
            is_cut,
            sticky,
            filename_text_color,
            diagnostic_severity,
            diagnostic_mark,
            diagnostic_count,
            git_status,
            worktree_id,
            canonical_path: entry.canonical_path.clone(),
        }
    }

    fn find_active_indent_guide(&self, indent_guides: &[IndentGuideLayout], cx: &App) -> Option<usize> {
        let (worktree, entry) = self.selected_entry(cx)?;

        // Find the parent entry of the indent guide, this will either be the
        // expanded folder we have selected, or the parent of the currently
        // selected file/collapsed directory
        let mut entry = entry;
        loop {
            let is_expanded_dir = entry.is_dir()
                && self
                    .state
                    .expanded_dir_ids
                    .get(&worktree.id())
                    .is_some_and(|ids| ids.binary_search(&entry.id).is_ok());
            if is_expanded_dir {
                break;
            }
            entry = worktree.entry_for_path(entry.path.parent()?)?;
        }

        let (active_indent_range, depth) = {
            let (worktree_ix, child_offset, ix) = self.index_for_entry(entry.id, worktree.id())?;
            let child_paths = &self.state.visible_entries[worktree_ix].entries;
            let mut child_count = 0;
            let depth = entry.path.ancestors().count();
            while let Some(entry) = child_paths.get(child_offset + child_count + 1) {
                if entry.path.ancestors().count() <= depth {
                    break;
                }
                child_count += 1;
            }

            let start = ix + 1;
            let end = start + child_count;

            let visible_worktree = &self.state.visible_entries[worktree_ix];
            let visible_worktree_entries = visible_worktree
                .index
                .get_or_init(|| visible_worktree.entries.iter().map(|e| Arc::clone(&e.path)).collect());

            // Calculate the actual depth of the entry, taking into account that directories can be auto-folded.
            let (depth, _) = Self::calculate_depth_and_difference(entry, visible_worktree_entries);
            (start..end, depth + visible_worktree.depth_offset)
        };

        let candidates = indent_guides.iter().enumerate().filter(|(_, indent_guide)| indent_guide.offset.x == depth);

        for (i, indent) in candidates {
            // Find matches that are either an exact match, partially on screen, or inside the enclosing indent
            if active_indent_range.start <= indent.offset.y + indent.length
                && indent.offset.y <= active_indent_range.end
            {
                return Some(i);
            }
        }
        None
    }

    fn render_sticky_entries(
        &self,
        child: StickyProjectPanelCandidate,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> SmallVec<[AnyElement; 8]> {
        let project = self.project.read(cx);

        let Some((worktree_id, entry_ref)) = self.entry_at_index(child.index) else {
            return SmallVec::new();
        };

        let Some(visible) = self.state.visible_entries.iter().find(|worktree| worktree.worktree_id == worktree_id)
        else {
            return SmallVec::new();
        };

        let Some(worktree) = project.worktree_for_id(worktree_id, cx) else {
            return SmallVec::new();
        };
        let worktree = worktree.read(cx).snapshot();

        let paths = visible.index.get_or_init(|| visible.entries.iter().map(|e| Arc::clone(&e.path)).collect());

        let mut sticky_parents = Vec::new();
        let mut current_path = Arc::clone(&entry_ref.path);

        'outer: loop {
            if let Some(parent_path) = current_path.parent() {
                for ancestor_path in parent_path.ancestors() {
                    if paths.contains(ancestor_path)
                        && let Some(parent_entry) = worktree.entry_for_path(ancestor_path)
                    {
                        sticky_parents.push(parent_entry.clone());
                        current_path = Arc::clone(&parent_entry.path);
                        continue 'outer;
                    }
                }
            }
            break 'outer;
        }

        if sticky_parents.is_empty() {
            return SmallVec::new();
        }

        sticky_parents.reverse();

        let root_name = worktree.root_name();

        let git_summaries_by_id = visible.entries.iter().map(|e| (e.id, e.git_summary)).collect::<FxHashMap<_, _>>();

        // already checked if non empty above
        let last_item_index = sticky_parents.len() - 1;
        let marked_selections: Arc<[SelectedEntry]> = Arc::from(self.marked_entries.clone());
        sticky_parents
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let git_status = git_summaries_by_id.get(&entry.id).copied().unwrap_or_default();
                let sticky_details = Some(StickyDetails { sticky_index: index });
                let details = self.details_for_entry(
                    entry,
                    worktree_id,
                    root_name,
                    paths,
                    git_status,
                    sticky_details,
                    window,
                    cx,
                );
                self.render_entry(entry.id, details, Arc::clone(&marked_selections), window, cx)
                    .when(index == last_item_index, |this| {
                        let shadow_color_top = hsla(0.0, 0.0, 0.0, 0.1);
                        let shadow_color_bottom = hsla(0.0, 0.0, 0.0, 0.);
                        let sticky_shadow =
                            div().absolute().left_0().bottom_neg_1p5().h_1p5().w_full().bg(linear_gradient(
                                0.,
                                linear_color_stop(shadow_color_top, 1.),
                                linear_color_stop(shadow_color_bottom, 0.),
                            ));
                        this.child(sticky_shadow)
                    })
                    .into_any()
            })
            .collect()
    }
}

struct ItemColors {
    default: Hsla,
    hover: Hsla,
    drag_over: Hsla,
    drag_over_border: Hsla,
    marked: Hsla,
    focused: Hsla,
}

fn get_item_color(is_sticky: bool) -> ItemColors {
    let colors = theme::theme().colors();

    ItemColors {
        default: if is_sticky { colors.panel_overlay_background } else { colors.panel_background },
        hover: if is_sticky { colors.panel_overlay_hover } else { colors.element_hover },
        marked: colors.element_selected,
        focused: colors.panel_focused_border,
        drag_over: colors.drop_target_background(),
        drag_over_border: colors.drop_target_border(),
    }
}

#[derive(Clone)]
struct StickyProjectPanelCandidate {
    index: usize,
    depth: usize,
}

impl StickyCandidate for StickyProjectPanelCandidate {
    fn depth(&self) -> usize {
        self.depth
    }
}

static PROJECT_PANEL_ACTIONS: std::sync::LazyLock<gpui::ActionTable<ProjectPanel>> = std::sync::LazyLock::new(|| {
    let mut t = gpui::ActionTable::<ProjectPanel>::default();
    t.add(ProjectPanel::scroll_up);
    t.add(ProjectPanel::scroll_down);
    t.add(ProjectPanel::scroll_cursor_center);
    t.add(ProjectPanel::scroll_cursor_top);
    t.add(ProjectPanel::scroll_cursor_bottom);
    t.add(ProjectPanel::select_next);
    t.add(ProjectPanel::select_previous);
    t.add(ProjectPanel::select_first);
    t.add(ProjectPanel::select_last);
    t.add(ProjectPanel::select_parent);
    t.add(ProjectPanel::select_next_git_entry);
    t.add(ProjectPanel::select_prev_git_entry);
    t.add(ProjectPanel::select_next_diagnostic);
    t.add(ProjectPanel::select_prev_diagnostic);
    t.add(ProjectPanel::select_next_directory);
    t.add(ProjectPanel::select_prev_directory);
    t.add(ProjectPanel::expand_selected_entry);
    t.add(ProjectPanel::collapse_selected_entry);
    t.add(ProjectPanel::collapse_all_entries);
    t.add(ProjectPanel::expand_all_entries);
    t.add(ProjectPanel::collapse_selected_entry_and_children);
    t.add(ProjectPanel::expand_selected_entry_and_children);
    t.add(ProjectPanel::open);
    t.add(ProjectPanel::open_permanent);
    t.add(ProjectPanel::open_split_vertical);
    t.add(ProjectPanel::open_split_horizontal);
    t.add(ProjectPanel::open_markdown_preview);
    t.add(ProjectPanel::open_svg_preview);
    // Without a selection the workspace decides, from the active editor's file.
    t.add_if(|panel, _| panel.selection.is_some(), ProjectPanel::file_history);
    t.add(ProjectPanel::new_file);
    t.add(ProjectPanel::undo);
    t.add(ProjectPanel::redo);
    t.add(ProjectPanel::reveal_in_finder);
    t.add(ProjectPanel::confirm);
    t.add(ProjectPanel::cancel);
    t.add(ProjectPanel::copy_path);
    t.add(ProjectPanel::copy_relative_path);
    t.add(ProjectPanel::new_search_in_directory);
    t.add(ProjectPanel::unfold_directory);
    t.add(ProjectPanel::fold_directory);
    t.add(ProjectPanel::remove_from_project);
    t.add(ProjectPanel::compare_marked_files);
    t.add(ProjectPanel::open_context_menu);
    t.add(ProjectPanel::new_directory);
    t.add(ProjectPanel::rename);
    t.add(ProjectPanel::delete);
    t.add(ProjectPanel::cut);
    t.add(ProjectPanel::copy);
    t.add(ProjectPanel::paste);
    t.add(ProjectPanel::duplicate);
    t.add(ProjectPanel::restore_file);
    t.add(ProjectPanel::add_to_gitignore);
    t.add(ProjectPanel::add_to_git_info_exclude);
    t.add(ProjectPanel::open_system);
    t.add(ProjectPanel::open_in_terminal);
    t
});

impl Render for ProjectPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.clear_drag_state(cx);
        }

        let has_worktree = !self.state.visible_entries.is_empty();
        let show_sticky_entries = self.scroll_handle.is_scrollable() && self.scroll_handle.offset().y < px(0.);

        if has_worktree {
            let item_count = self.state.visible_entries.iter().map(|worktree| worktree.entries.len()).sum();

            fn handle_drag_move<T: 'static>(
                this: &mut ProjectPanel,
                e: &DragMoveEvent<T>,
                window: &mut Window,
                cx: &mut Context<ProjectPanel>,
            ) {
                if let Some(previous_position) = this.previous_drag_position {
                    // Refresh cursor only when an actual drag happens,
                    // because modifiers are not updated when the cursor is not moved.
                    if e.event.position != previous_position {
                        this.refresh_drag_cursor_style(e.event.modifiers, window, cx);
                    }
                }
                this.previous_drag_position = Some(e.event.position);

                if !e.bounds.contains(&e.event.position) {
                    this.clear_drag_state(cx);
                    return;
                }
                this.hover_scroll_task.take();
                let panel_height = e.bounds.size.height;
                if panel_height <= px(0.) {
                    return;
                }

                let event_offset = e.event.position.y - e.bounds.origin.y;
                // How far along in the project panel is our cursor? (0. is the top of a list, 1. is the bottom)
                let hovered_region_offset = event_offset / panel_height;

                // We want the scrolling to be a bit faster when the cursor is closer to the edge of a list.
                // These pixels offsets were picked arbitrarily.
                let vertical_scroll_offset = if hovered_region_offset <= 0.05 {
                    8.
                } else if hovered_region_offset <= 0.15 {
                    5.
                } else if hovered_region_offset >= 0.95 {
                    -8.
                } else if hovered_region_offset >= 0.85 {
                    -5.
                } else {
                    return;
                };
                let adjustment = point(px(0.), px(vertical_scroll_offset));
                this.hover_scroll_task = Some(cx.spawn_in(window, async move |this, cx| {
                    loop {
                        let should_stop_scrolling = this
                            .update(cx, |this, cx| {
                                this.hover_scroll_task.as_ref()?;
                                let handle = this.scroll_handle.0.borrow_mut();
                                let offset = handle.base_handle.offset();

                                handle.base_handle.set_offset(offset + adjustment);
                                cx.notify();
                                Some(())
                            })
                            .ok()
                            .flatten()
                            .is_some();
                        if should_stop_scrolling {
                            return;
                        }
                        cx.background_executor().timer(Duration::from_millis(16)).await;
                    }
                }));
            }
            h_flex()
                .id("project-panel")
                .group("project-panel")
                .map(|this| {
                    this.on_drag_move(cx.listener(handle_drag_move::<DraggedSelection>)).on_mouse_exit(cx.listener(
                        |this, _: &MouseExitEvent, window, cx| {
                            cx.stop_active_drag(window);
                            this.clear_drag_state(cx);
                        },
                    ))
                })
                .size_full()
                .relative()
                .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                    this.refresh_drag_cursor_style(event.modifiers, window, cx);
                }))
                .key_context(self.dispatch_context(window, cx))
                .action_table(&cx.entity(), &PROJECT_PANEL_ACTIONS)
                .track_focus(&self.focus_handle(cx))
                .child(
                    v_flex()
                        .child(
                            uniform_list("entries", item_count, {
                                cx.processor(|this, range: Range<usize>, window, cx| {
                                    this.rendered_entries_len = range.end - range.start;
                                    let mut items = Vec::with_capacity(this.rendered_entries_len);
                                    let marked_selections: Arc<[SelectedEntry]> =
                                        Arc::from(this.marked_entries.clone());
                                    this.drag_highlight_last_entry_id = this.find_drag_highlight_last_entry_id();
                                    this.for_each_visible_entry(range, window, cx, &mut |id, details, window, cx| {
                                        items.push(this.render_entry(
                                            id,
                                            details,
                                            Arc::clone(&marked_selections),
                                            window,
                                            cx,
                                        ));
                                    });
                                    items
                                })
                            })
                            .map(|list| {
                                list.with_decoration(
                                    ui::indent_guides(INDENT_SIZE, IndentGuideColors::panel())
                                        .with_compute_indents_fn(cx.entity(), |this, range, window, cx| {
                                            let mut items = SmallVec::with_capacity(range.end - range.start);
                                            this.iter_visible_entries(
                                                range,
                                                window,
                                                cx,
                                                &mut |entry, _, entries, depth_offset, _, _| {
                                                    let (depth, _) =
                                                        Self::calculate_depth_and_difference(entry, entries);
                                                    items.push(depth + depth_offset);
                                                },
                                            );
                                            items
                                        })
                                        .on_click(cx.listener(
                                            |this, active_indent_guide: &IndentGuideLayout, window, cx| {
                                                if window.modifiers().secondary() {
                                                    let ix = active_indent_guide.offset.y;
                                                    let Some((target_entry, worktree)) = (|| {
                                                        let (worktree_id, entry) = this.entry_at_index(ix)?;
                                                        let worktree =
                                                            this.project.read(cx).worktree_for_id(worktree_id, cx)?;
                                                        let target_entry =
                                                            worktree.read(cx).entry_for_path(entry.path.parent()?)?;
                                                        Some((target_entry, worktree))
                                                    })(
                                                    ) else {
                                                        return;
                                                    };

                                                    this.collapse_entry(target_entry.clone(), worktree, window, cx);
                                                }
                                            },
                                        ))
                                        .with_render_fn(cx.entity(), move |this, params, _, cx| {
                                            const LEFT_OFFSET: Pixels = ui::LIST_ITEM_INDENT_GUIDE_LEFT_OFFSET;
                                            const PADDING_Y: Pixels = px(4.);
                                            const HITBOX_OVERDRAW: Pixels = px(3.);

                                            let active_indent_guide_index =
                                                this.find_active_indent_guide(&params.indent_guides, cx);

                                            let indent_size = params.indent_size;
                                            let item_height = params.item_height;

                                            params
                                                .indent_guides
                                                .into_iter()
                                                .enumerate()
                                                .map(|(idx, layout)| {
                                                    let offset =
                                                        if layout.continues_offscreen { px(0.) } else { PADDING_Y };
                                                    let bounds = Bounds::new(
                                                        point(
                                                            layout.offset.x * indent_size + LEFT_OFFSET,
                                                            layout.offset.y * item_height + offset,
                                                        ),
                                                        size(px(1.), layout.length * item_height - offset * 2.),
                                                    );
                                                    ui::RenderedIndentGuide {
                                                        bounds,
                                                        layout,
                                                        is_active: Some(idx) == active_indent_guide_index,
                                                        hitbox: Some(Bounds::new(
                                                            point(bounds.origin.x - HITBOX_OVERDRAW, bounds.origin.y),
                                                            size(
                                                                bounds.size.width + HITBOX_OVERDRAW * 2.,
                                                                bounds.size.height,
                                                            ),
                                                        )),
                                                    }
                                                })
                                                .collect()
                                        }),
                                )
                            })
                            .when(show_sticky_entries, |list| {
                                let sticky_items = ui::sticky_items(
                                    cx.entity(),
                                    |this, range, window, cx| {
                                        let mut items = SmallVec::with_capacity(range.end - range.start);
                                        this.iter_visible_entries(
                                            range,
                                            window,
                                            cx,
                                            &mut |entry, index, entries, depth_offset, _, _| {
                                                let (depth, _) = Self::calculate_depth_and_difference(entry, entries);
                                                let candidate =
                                                    StickyProjectPanelCandidate { index, depth: depth + depth_offset };
                                                items.push(candidate);
                                            },
                                        );
                                        items
                                    },
                                    |this, marker_entry, window, cx| {
                                        let sticky_entries = this.render_sticky_entries(marker_entry, window, cx);
                                        this.sticky_items_count = sticky_entries.len();
                                        sticky_entries
                                    },
                                );
                                list.with_decoration(sticky_items.with_decoration(
                                    ui::indent_guides(INDENT_SIZE, IndentGuideColors::panel()).with_render_fn(
                                        cx.entity(),
                                        move |_, params, _, _| {
                                            const LEFT_OFFSET: Pixels = ui::LIST_ITEM_INDENT_GUIDE_LEFT_OFFSET;

                                            let indent_size = params.indent_size;
                                            let item_height = params.item_height;

                                            params
                                                .indent_guides
                                                .into_iter()
                                                .map(|layout| {
                                                    let bounds = Bounds::new(
                                                        point(
                                                            layout.offset.x * indent_size + LEFT_OFFSET,
                                                            layout.offset.y * item_height,
                                                        ),
                                                        size(px(1.), layout.length * item_height),
                                                    );
                                                    ui::RenderedIndentGuide {
                                                        bounds,
                                                        layout,
                                                        is_active: false,
                                                        hitbox: None,
                                                    }
                                                })
                                                .collect()
                                        },
                                    ),
                                ))
                            })
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                            .with_width_from_item(self.state.max_width_item_index)
                            .track_scroll(&self.scroll_handle),
                        )
                        .child(
                            div()
                                .id("project-panel-blank-area")
                                .block_mouse_except_scroll()
                                // `block_mouse_except_scroll` prevents the dock's own
                                // focus-follows-mouse hover handler from seeing this area,
                                // so handle it here directly.
                                .flex_grow_1()
                                .on_scroll_wheel({
                                    let scroll_handle = self.scroll_handle.clone();
                                    let entity_id = cx.entity().entity_id();
                                    move |event, window, cx| {
                                        let state = scroll_handle.0.borrow();
                                        let base_handle = &state.base_handle;
                                        let current_offset = base_handle.offset();
                                        let max_offset = base_handle.max_offset();
                                        let delta = event.delta.pixel_delta(window.line_height());
                                        let new_offset =
                                            (current_offset + delta).clamp(&max_offset.neg(), &Point::default());

                                        if new_offset != current_offset {
                                            base_handle.set_offset(new_offset);
                                            cx.notify(entity_id);
                                        }
                                    }
                                })
                                .when(
                                    self.drag_target_entry.as_ref().is_some_and(|entry| match entry {
                                        DragTarget::Background => true,
                                        DragTarget::Entry { highlight_entry_id, .. } => self
                                            .state
                                            .last_worktree_root_id
                                            .is_some_and(|root_id| *highlight_entry_id == root_id),
                                    }),
                                    |div| div.bg(theme::theme().colors().drop_target_background()),
                                )
                                .on_drag_move::<DraggedSelection>(cx.listener(
                                    move |this, event: &DragMoveEvent<DraggedSelection>, _, cx| {
                                        let Some(last_root_id) = this.state.last_worktree_root_id else {
                                            return;
                                        };
                                        if event.bounds.contains(&event.event.position) {
                                            let drag_state = event.drag(cx);
                                            if this.should_highlight_background_for_selection_drag(
                                                drag_state,
                                                last_root_id,
                                                cx,
                                            ) {
                                                this.drag_target_entry = Some(DragTarget::Background);
                                            }
                                        } else if this
                                            .drag_target_entry
                                            .as_ref()
                                            .is_some_and(|e| matches!(e, DragTarget::Background))
                                        {
                                            this.drag_target_entry = None;
                                        }
                                    },
                                ))
                                .on_drop(cx.listener(move |this, selections: &DraggedSelection, window, cx| {
                                    this.clear_drag_state(cx);
                                    if let Some(entry_id) = this.state.last_worktree_root_id {
                                        this.drag_onto(selections, entry_id, false, window, cx);
                                    }
                                    cx.stop_propagation();
                                }))
                                .on_click(cx.listener(|this, event, window, cx| {
                                    if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    this.selection = None;
                                    this.marked_entries.clear();
                                    this.focus_handle(cx).focus(window, cx);
                                    cx.notify();
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        // When deploying the context menu anywhere below the last project entry,
                                        // act as if the user clicked the root of the last worktree.
                                        if let Some(entry_id) = this.state.last_worktree_root_id {
                                            this.deploy_context_menu(
                                                ContextMenuPlacement::AtMouse(event.position),
                                                entry_id,
                                                window,
                                                cx,
                                            );
                                        }
                                    }),
                                )
                                .map(|el| {
                                    el.on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                                        if event.click_count() > 1
                                            && let Some(entry_id) = this.state.last_worktree_root_id
                                        {
                                            let project = this.project.read(cx);

                                            let worktree_id =
                                                if let Some(worktree) = project.worktree_for_entry(entry_id, cx) {
                                                    worktree.read(cx).id()
                                                } else {
                                                    return;
                                                };

                                            this.selection = Some(SelectedEntry { worktree_id, entry_id });

                                            this.new_file(&NewFile, window, cx);
                                        }
                                    }))
                                }),
                        )
                        .size_full(),
                )
                .custom_scrollbars(
                    Scrollbars::new(ScrollAxes::Both)
                        .tracked_scroll_handle(&self.scroll_handle)
                        .with_track_along(ScrollAxes::Horizontal, theme::theme().colors().panel_background)
                        .notify_content(),
                    window,
                    cx,
                )
                .children(self.context_menu.as_ref().map(|context_menu| {
                    deferred(
                        anchored()
                            .position(context_menu.position)
                            .when_some(context_menu.anchor, gpui::Anchored::anchor)
                            .child(context_menu.menu.clone()),
                    )
                    .with_priority(3)
                }))
        } else {
            let focus_handle = self.focus_handle(cx);
            let workspace = self.workspace.clone();
            let workspace_clone = self.workspace.clone();

            v_flex().id("empty-project_panel-wrapper").size_full().child(
                ProjectEmptyState::new(
                    "Project Panel",
                    focus_handle.clone(),
                    KeyBinding::for_action_in(&workspace::Open::default(), &focus_handle, cx),
                )
                .on_open_project(move |_, window, cx| {
                    workspace
                        .update(cx, |_, cx| {
                            window.dispatch_action(workspace::Open::default().boxed_clone(), cx);
                        })
                        .log_err();
                })
                .on_clone_repo(move |_, window, cx| {
                    workspace_clone
                        .update(cx, |_, cx| {
                            window.dispatch_action(git::Clone.boxed_clone(), cx);
                        })
                        .log_err();
                }),
            )
        }
    }
}

impl Render for DraggedProjectEntryView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let ui_font = font::ui_font().clone();
        h_flex().font(ui_font).pl(self.click_offset.x + px(12.)).pt(self.click_offset.y + px(12.)).child(
            div().flex().gap_1().items_center().py_1().px_2().rounded_lg().bg(theme::theme().colors().background).map(
                |this| {
                    if self.selections.len() > 1 && self.selections.contains(&self.selection) {
                        this.child(Label::new(format!("{} entries", self.selections.len())))
                    } else {
                        this.child(
                            h_flex()
                                .gap_0p5()
                                .when_some(self.icon.clone(), |this, icon| this.child(Icon::from_path(icon))),
                        )
                        .child(Label::new(self.filename.clone()))
                    }
                },
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

    fn conditional<V: 'static>(table: &gpui::ActionTable<V>) -> Vec<TypeId> {
        let types = table.action_types().iter().zip(table.conditional());
        types.filter_map(|(action_type, conditional)| conditional.then_some(*action_type)).collect()
    }

    #[test]
    fn only_file_history_is_conditional() {
        assert_eq!(conditional(&PROJECT_PANEL_ACTIONS), [TypeId::of::<git::FileHistory>()]);
        assert!(PROJECT_PANEL_ACTIONS.action_types().is_sorted_by(|a, b| a < b), "action handled twice");
    }
}
