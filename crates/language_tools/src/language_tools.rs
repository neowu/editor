pub mod lsp_button;
pub mod lsp_log_view;

use gpui::App;
use gpui::AppContext as _;
use gpui::Entity;
use ui::Context;
use ui::Window;
use workspace::Item;
use workspace::ItemHandle as _;
use workspace::SplitDirection;
use workspace::Workspace;
use workspace::WorkspaceActions;

pub fn init(workspace_actions: &mut WorkspaceActions, cx: &mut App) {
    lsp_log_view::init(cx);
    lsp_button::init(workspace_actions);
}

fn get_or_create_tool<T>(
    workspace: &mut Workspace,
    destination: SplitDirection,
    window: &mut Window,
    cx: &mut Context<Workspace>,
    new_tool: impl FnOnce(&mut Window, &mut Context<T>) -> T,
) -> Entity<T>
where
    T: Item,
{
    if let Some(item) = workspace.item_of_type::<T>(cx) {
        return item;
    }

    let new_tool = cx.new(|cx| new_tool(window, cx));
    match workspace.find_pane_in_direction(destination, cx) {
        Some(right_pane) => {
            workspace.add_item(right_pane, new_tool.boxed_clone(), None, true, true, window, cx);
        }
        None => {
            workspace.split_item(destination, new_tool.boxed_clone(), window, cx);
        }
    }
    new_tool
}
