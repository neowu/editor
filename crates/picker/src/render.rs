use std::sync::Arc;

use gpui::Action as _;
use gpui::Focusable as _;
use gpui::canvas;
use ui::Color;
use ui::Context;
use ui::Disableable;
use ui::Divider;
use ui::DocumentationAside;
use ui::DocumentationSide;
use ui::FluentBuilder;
use ui::InteractiveElement;
use ui::IntoElement;
use ui::Label;
use ui::LabelCommon;
use ui::ListItem;
use ui::ListItemSpacing;
use ui::ParentElement;
use ui::Render;
use ui::ScrollAxes;
use ui::Scrollbars;
use ui::Styled;
use ui::StyledExt;
use ui::Tooltip;
use ui::Window;
use ui::WithScrollbar;
use ui::div;
use ui::font;
use ui::h_flex;
use ui::prelude::*;
use ui::rems_from_px;
use ui::utils::WithRemSize;
use ui::v_flex;
use ui_input::ErasedEditor;

use crate::ElementContainer;
use crate::Picker;
use crate::PickerDelegate;
use crate::PickerEditorPosition;
use crate::ToggleMultiSelect;
use crate::head::Head;

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().when(self.draws_own_container(), |this| this.elevation_3(cx)).child(self.render_results(window, cx))
    }
}

impl<D: PickerDelegate> Picker<D> {
    fn render_editor(&self, editor: &Arc<dyn ErasedEditor>, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        if let Some(custom) = self.delegate.render_editor(editor, window, cx) {
            return custom;
        }
        let editor_position = self.delegate.editor_position();

        v_flex()
            .when(editor_position == PickerEditorPosition::End, |this| this.child(Divider::horizontal()))
            .child(
                h_flex()
                    .h_9()
                    .px_2p5()
                    .flex_none()
                    .overflow_hidden()
                    .child(div().flex_1().child(editor.render(window, cx)))
                    .children(self.delegate.searchbar_trailer(window, cx))
                    .when(self.delegate.supports_multi_select(), |this| {
                        this.child(self.render_multi_select_toggle(cx))
                    }),
            )
            .when(editor_position == PickerEditorPosition::Start, |this| this.child(Divider::horizontal()))
    }

    /// The multi-select toggle is picker-owned so it can reflect the mode,
    /// which delegates don't know about.
    fn render_multi_select_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        let active = self.select_instead_of_open;
        let focus_handle = self.focus_handle(cx);
        IconButton::new("picker-multi-select-toggle", IconName::FileMultiple)
            .icon_size(IconSize::Small)
            .toggle_state(active)
            .tooltip(move |_window, cx| {
                Tooltip::for_action_in("Toggle Multi Select", &ToggleMultiSelect, &focus_handle, cx)
            })
            .on_click(cx.listener(|_, _, window, cx| {
                window.dispatch_action(ToggleMultiSelect.boxed_clone(), cx);
            }))
    }

    pub(crate) fn render_results(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font_size = font::ui_font_size(cx);
        let window_size = window.viewport_size();
        let rem_size = window.rem_size();
        let is_wide_window = window_size.width / rem_size > rems_from_px(800_f32).0;

        let aside = self.delegate.documentation_aside(window, cx);

        let editor_position = self.delegate.editor_position();
        let picker_bounds = self.picker_bounds.clone();

        let menu = v_flex()
            .key_context("Picker")
            .relative()
            .w(self.shape.width(window))
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        picker_bounds.set(Some(bounds));
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .size_full()
                .absolute()
                .top_0()
                .left_0(),
            )
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::select_child))
            .on_action(cx.listener(Self::select_parent))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::secondary_confirm))
            .on_action(cx.listener(Self::confirm_completion))
            .on_action(cx.listener(Self::confirm_input))
            .on_action(cx.listener(Self::toggle_multi_select))
            .on_action(cx.listener(Self::multi_select_next))
            .children(match &self.head {
                Head::Editor(editor) => {
                    if editor_position == PickerEditorPosition::Start {
                        let editor = editor.clone();
                        Some(self.render_editor(&editor, window, cx))
                    } else {
                        None
                    }
                }
                Head::Empty(empty_head) => Some(h_flex().child(empty_head.clone())),
            })
            .when(self.delegate.match_count() > 0, |el| {
                el.child(
                    v_flex()
                        .id("element-container")
                        .relative()
                        .flex_grow_1()
                        .min_h_0()
                        .max_h(self.shape.max_height(window))
                        .overflow_hidden()
                        .children(self.delegate.render_header(window, cx))
                        .child(self.render_element_container(cx))
                        .when(self.show_scrollbar, |this| {
                            let base_scrollbar_config = Scrollbars::new(ScrollAxes::Vertical);

                            this.map(|this| match &self.element_container {
                                ElementContainer::List(state) => this.custom_scrollbars(
                                    base_scrollbar_config.tracked_scroll_handle(state),
                                    window,
                                    cx,
                                ),
                                ElementContainer::UniformList(state) => this.custom_scrollbars(
                                    base_scrollbar_config.tracked_scroll_handle(state),
                                    window,
                                    cx,
                                ),
                            })
                        }),
                )
            })
            .when(self.delegate.match_count() == 0, |el| {
                el.when_some(self.delegate.no_matches_text(window, cx), |el, text| {
                    el.child(
                        v_flex().flex_grow_1().py_2().child(
                            ListItem::new("empty_state")
                                .inset(true)
                                .spacing(ListItemSpacing::Sparse)
                                .disabled(true)
                                .child(Label::new(text).color(Color::Muted)),
                        ),
                    )
                })
            })
            .children(self.delegate.render_footer(window, cx))
            .children(match &self.head {
                Head::Editor(editor) => {
                    if editor_position == PickerEditorPosition::End {
                        let editor = editor.clone();
                        Some(self.render_editor(&editor, window, cx))
                    } else {
                        None
                    }
                }
                Head::Empty(empty_head) => Some(div().child(empty_head.clone())),
            });

        let Some(aside) = aside else {
            return menu;
        };

        let render_aside = |aside: DocumentationAside, cx: &mut Context<Self>| {
            WithRemSize::new(ui_font_size)
                .occlude()
                .elevation_2(cx)
                .w_full()
                .p_2()
                .overflow_hidden()
                .when(is_wide_window, |this| this.max_w_96())
                .when(!is_wide_window, |this| this.max_w_48())
                .child((aside.render)(cx))
        };

        if is_wide_window {
            let aside_index = self.delegate.documentation_aside_index();
            let picker_bounds = self.picker_bounds.get();
            let item_bounds = aside_index.and_then(|ix| self.item_bounds.borrow().get(&ix).copied());

            let item_position = match (picker_bounds, item_bounds) {
                (Some(picker_bounds), Some(item_bounds)) => {
                    let relative_top = item_bounds.origin.y - picker_bounds.origin.y;
                    let height = item_bounds.size.height;
                    Some((relative_top, height))
                }
                _ => None,
            };

            div()
                .relative()
                .child(menu)
                // Only render the aside once we have bounds to avoid flicker
                .when_some(item_position, |this, (top, height)| {
                    this.child(
                        h_flex()
                            .absolute()
                            .when(aside.side == DocumentationSide::Left, |el| el.right_full().mr_1())
                            .when(aside.side == DocumentationSide::Right, |el| el.left_full().ml_1())
                            .top(top)
                            .h(height)
                            .child(render_aside(aside, cx)),
                    )
                })
        } else {
            v_flex().w_full().gap_1().justify_end().child(render_aside(aside, cx)).child(menu)
        }
    }
}
