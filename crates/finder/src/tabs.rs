use std::cmp::Reverse;
use std::sync::Arc;

use editor::items::entry_git_aware_label_color;
use fuzzy::StringMatchCandidate;
use gpui::AnyElement;
use gpui::App;
use gpui::Context;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::MouseButton;
use gpui::MouseUpEvent;
use gpui::ParentElement as _;
use gpui::Styled as _;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::WeakEntity;
use gpui::Window;
use picker::Picker;
use picker::PickerDelegate;
use project::Project;
use ui::ListItem;
use ui::ListItemSpacing;
use ui::Tooltip;
use ui::prelude::ButtonCommon as _;
use ui::prelude::Clickable as _;
use ui::prelude::Color;
use ui::prelude::Icon;
use ui::prelude::IconButton;
use ui::prelude::IconName;
use ui::prelude::IconSize;
use ui::prelude::InteractiveElement as _;
use ui::prelude::IntoElement as _;
use ui::prelude::SharedString;
use ui::prelude::Toggleable as _;
use ui::prelude::div;
use ui::prelude::h_flex;
use workspace::Event as WorkspaceEvent;
use workspace::Pane;
use workspace::SaveIntent;
use workspace::Workspace;
use workspace::item::ItemHandle;
use workspace::item::TabContentParams;
use workspace::pane::render_item_indicator;
use workspace::pane::tab_details;

use crate::CloseSelectedTab;

struct TabMatch {
    pane: WeakEntity<Pane>,
    item: Box<dyn ItemHandle>,
    detail: usize,
    preview: bool,
}

impl TabMatch {
    fn icon(&self, project: &Entity<Project>, selected: bool, window: &Window, cx: &App) -> Option<Icon> {
        let icon = self.item.tab_icon(window, cx)?;
        let git_status_color = self.item.project_path(cx).and_then(|path| {
            let project = project.read(cx);
            let entry = project.entry_for_path(&path, cx)?;
            let git_status = project
                .git_store()
                .read(cx)
                .status_for_project_path(&path, cx)
                .map_or_default(|status| status.summary());
            Some(entry_git_aware_label_color(git_status, entry.is_ignored, selected))
        });
        Some(icon.color(git_status_color.unwrap_or_default()))
    }
}

/// Lists the open tabs of every pane, most recently used first. Selecting a
/// tab has no effect until it is confirmed.
pub(crate) struct TabsDelegate {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    matches: Vec<TabMatch>,
    selected_index: usize,
}

impl TabsDelegate {
    pub(crate) const fn new(workspace: WeakEntity<Workspace>, project: Entity<Project>) -> Self {
        Self { workspace, project, matches: Vec::new(), selected_index: 0 }
    }

    /// Keeps the list in sync with tabs opened or closed while it is shown.
    pub(crate) fn subscribe_to_updates(&self, window: &Window, cx: &mut Context<Picker<Self>>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        cx.subscribe_in(&workspace, window, |picker, _, event, window, cx| {
            if matches!(
                event,
                WorkspaceEvent::ItemAdded { .. } | WorkspaceEvent::ItemRemoved { .. } | WorkspaceEvent::PaneRemoved
            ) {
                picker.refresh(window, cx);
            }
        })
        .detach();
    }

    pub(crate) fn close_selected(&self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.close_item_at(self.selected_index, window, cx);
    }

    fn close_item_at(&self, ix: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(tab_match) = self.matches.get(ix) else {
            return;
        };
        let item_id = tab_match.item.item_id();
        tab_match
            .pane
            .update(cx, |pane, cx| {
                pane.close_item_by_id(item_id, SaveIntent::Close, window, cx).detach_and_log_err(cx);
            })
            .ok();
    }
}

/// The index to select after the tab list was rebuilt for an empty query.
/// `previous` is where the previously selected tab ended up, `None` if it is
/// gone or (`had_selection` false) nothing was listed before.
fn reselect(previous: Option<usize>, had_selection: bool, old_index: usize, len: usize) -> usize {
    match previous {
        Some(ix) => ix,
        None if len == 0 => 0,
        None if had_selection => old_index.min(len - 1),
        // The first entry is the active tab, so preselect the one before it.
        None => usize::from(len > 1),
    }
}

impl PickerDelegate for TabsDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search open tabs...".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("No tabs".into())
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected_index = ix;
        cx.notify();
    }

    fn update_matches(&mut self, query: String, window: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Task::ready(());
        };
        let workspace = workspace.read(cx);
        let mut tabs = Vec::new();
        for pane_handle in workspace.panes() {
            let pane = pane_handle.read(cx);
            let items: Vec<Box<dyn ItemHandle>> = pane.items().map(|item| item.boxed_clone()).collect();
            let details = tab_details(&items, window, cx);
            for (item, detail) in items.into_iter().zip(details) {
                let preview = pane.is_active_preview_item(item.item_id());
                tabs.push(TabMatch { pane: pane_handle.downgrade(), item, detail, preview });
            }
        }

        if query.is_empty() {
            let history = workspace.recently_activated_items(cx);
            tabs.sort_by_key(|tab| Reverse(history.get(&tab.item.item_id()).copied()));
            let selected_item_id = self.matches.get(self.selected_index).map(|tab| tab.item.item_id());
            let previous = selected_item_id.and_then(|id| tabs.iter().position(|tab| tab.item.item_id() == id));
            self.selected_index = reselect(previous, selected_item_id.is_some(), self.selected_index, tabs.len());
            self.matches = tabs;
        } else {
            let candidates = tabs
                .iter()
                .enumerate()
                .map(|(ix, tab)| StringMatchCandidate::new(ix, tab.item.tab_content_text(0, cx)))
                .collect::<Vec<_>>();
            let mut tabs = tabs.into_iter().map(Some).collect::<Vec<_>>();
            self.matches =
                fuzzy::match_strings(&candidates, &query, fuzzy::Case::Smart, fuzzy::LengthPenalty::On, tabs.len())
                    .into_iter()
                    .filter_map(|m| tabs[m.candidate_id].take())
                    .collect();
            self.selected_index = 0;
        }
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(tab_match) = self.matches.get(self.selected_index) else {
            return;
        };
        tab_match
            .pane
            .update(cx, |pane, cx| {
                if let Some(index) = pane.index_for_item(tab_match.item.as_ref()) {
                    pane.activate_item(index, true, true, window, cx);
                }
            })
            .ok();
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let tab_match = self.matches.get(ix)?;

        let params = TabContentParams {
            detail: Some(tab_match.detail),
            selected: true,
            preview: tab_match.preview,
            deemphasized: false,
            max_title_len: Some(usize::MAX),
            truncate_title_middle: true,
        };
        let label = tab_match.item.tab_content(params, window, cx);
        let icon = tab_match.icon(&self.project, selected, window, cx);

        let indicator = render_item_indicator(tab_match.item.boxed_clone(), cx);
        let indicator_color = indicator.as_ref().map_or_else(Color::default, |indicator| indicator.color);
        let indicator = h_flex().flex_shrink_0().children(indicator).child(div().w_2()).into_any_element();
        let close_button = div()
            .id("close-button")
            .on_mouse_up(
                // A ctrl-click comes through as a right click on macOS.
                MouseButton::Right,
                cx.listener(move |picker, _: &MouseUpEvent, window, cx| {
                    cx.stop_propagation();
                    picker.delegate.close_item_at(ix, window, cx);
                }),
            )
            .child(
                IconButton::new("close_tab", IconName::Close)
                    .icon_size(IconSize::Small)
                    .icon_color(indicator_color)
                    .tooltip(Tooltip::for_action_title("Close", &CloseSelectedTab))
                    .on_click(cx.listener(move |picker, _, window, cx| {
                        cx.stop_propagation();
                        picker.delegate.close_item_at(ix, window, cx);
                    })),
            )
            .into_any_element();

        let item = ListItem::new(ix)
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .toggle_state(selected)
            .child(h_flex().w_full().min_w_0().overflow_hidden().child(label))
            .start_slot::<Icon>(icon);
        Some(if selected {
            item.end_slot::<AnyElement>(close_button)
        } else {
            item.end_slot::<AnyElement>(indicator).end_slot_on_hover::<AnyElement>(close_button)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::reselect;

    #[test]
    fn reselect_follows_the_previously_selected_tab() {
        assert_eq!(reselect(Some(3), true, 1, 5), 3);
    }

    #[test]
    fn reselect_keeps_the_index_when_the_selected_tab_is_gone() {
        assert_eq!(reselect(None, true, 1, 5), 1);
        assert_eq!(reselect(None, true, 4, 3), 2);
    }

    #[test]
    fn reselect_preselects_the_previous_tab_on_first_listing() {
        assert_eq!(reselect(None, false, 0, 5), 1);
        assert_eq!(reselect(None, false, 0, 1), 0);
    }

    #[test]
    fn reselect_handles_an_empty_list() {
        assert_eq!(reselect(None, false, 0, 0), 0);
        assert_eq!(reselect(None, true, 2, 0), 0);
    }
}
