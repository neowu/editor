use editor::Editor;
use gpui::AnyElement;
use gpui::EventEmitter;
use gpui::WeakEntity;
use ui::prelude::*;
use workspace::ItemHandle;
use workspace::ToolbarItemEvent;
use workspace::ToolbarItemLocation;
use workspace::ToolbarItemView;
use workspace::Workspace;

use crate::TabularDataPreviewPane;

/// Adds the tab bar's preview button for CSV-like files. Detection is by file
/// extension, so it is resolved on each tab bar render.
pub struct TabularDataPreviewButton {
    workspace: WeakEntity<Workspace>,
    active_item: Option<Box<dyn ItemHandle>>,
}

impl TabularDataPreviewButton {
    pub fn new(workspace: WeakEntity<Workspace>) -> TabularDataPreviewButton {
        TabularDataPreviewButton { workspace, active_item: None }
    }
}

impl Render for TabularDataPreviewButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<TabularDataPreviewButton>) -> impl IntoElement {
        gpui::Empty
    }
}

impl EventEmitter<ToolbarItemEvent> for TabularDataPreviewButton {}

impl ToolbarItemView for TabularDataPreviewButton {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<TabularDataPreviewButton>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(|item| item.boxed_clone());
        ToolbarItemLocation::Hidden
    }

    fn tab_bar_button(&self, cx: &App) -> Option<AnyElement> {
        let item = self.active_item.as_ref()?;
        let editor =
            item.act_as::<Editor>(cx).filter(|editor| TabularDataPreviewPane::is_tabular_data_file(editor, cx))?;
        Some(workspace::pane::render_tab_bar_preview_button(
            "Preview Tabular Data",
            &crate::OpenPreview,
            self.workspace.clone(),
            item.as_ref(),
            move |workspace, pane, to_the_side, window, cx| {
                if to_the_side {
                    TabularDataPreviewPane::open_preview_to_the_side_of_pane(
                        workspace,
                        editor.clone(),
                        pane,
                        window,
                        cx,
                    );
                } else {
                    TabularDataPreviewPane::open_preview_in_pane(editor.clone(), pane, window, cx);
                }
            },
        ))
    }
}
