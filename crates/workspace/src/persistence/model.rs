use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use gpui::AsyncWindowContext;
use gpui::Axis;
use gpui::Entity;
use gpui::WeakEntity;
use project::Project;
use remote::RemoteConnectionOptions;
use serde::Deserialize;
use serde::Serialize;
use util::ResultExt;
use uuid::Uuid;

use super::WindowBoundsJson;
use crate::Member;
use crate::Pane;
use crate::PaneAxis;
use crate::SerializableItemRegistry;
use crate::Workspace;
use crate::item::ItemHandle;
use crate::path_list::PathList;

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum SerializedWorkspaceLocation {
    Local,
    Remote(RemoteConnectionOptions),
}

/// A project that was open when ed last quit.
#[derive(Debug, PartialEq, Clone)]
pub struct SessionWorkspace {
    pub location: SerializedWorkspaceLocation,
    pub paths: PathList,
}

/// Contents of a project's `.ed/workspace.json`.
#[derive(Debug, PartialEq, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SerializedWorkspace {
    #[serde(default)]
    pub(crate) center_group: SerializedPaneGroup,
    #[serde(default)]
    pub(crate) docks: DockStructure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_bounds: Option<WindowBoundsJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recent_navigation_history: Vec<PathBuf>,
}

#[derive(Debug, PartialEq, Clone, Default, Serialize, Deserialize)]
pub struct DockStructure {
    #[serde(default)]
    pub left: DockData,
    #[serde(default)]
    pub right: DockData,
    #[serde(default)]
    pub bottom: DockData,
}

#[derive(Debug, PartialEq, Clone, Default, Serialize, Deserialize)]
pub struct DockData {
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub active_panel: Option<String>,
    #[serde(default)]
    pub zoom: bool,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SerializedPaneGroup {
    Group {
        axis: Axis,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flexes: Option<Vec<f32>>,
        children: Vec<SerializedPaneGroup>,
    },
    Pane(SerializedPane),
}

impl Default for SerializedPaneGroup {
    fn default() -> SerializedPaneGroup {
        SerializedPaneGroup::Pane(SerializedPane { active: true, children: Vec::new(), pinned_count: 0 })
    }
}

impl SerializedPaneGroup {
    pub(crate) async fn deserialize(
        self,
        project: &Entity<Project>,
        workspace: WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> Option<(Member, Option<Entity<Pane>>, Vec<Option<Box<dyn ItemHandle>>>)> {
        match self {
            SerializedPaneGroup::Group { axis, children, flexes } => {
                let mut current_active_pane = None;
                let mut members = Vec::new();
                let mut items = Vec::new();
                for child in children {
                    if let Some((new_member, active_pane, new_items)) =
                        Box::pin(child.deserialize(project, workspace.clone(), cx)).await
                    {
                        members.push(new_member);
                        items.extend(new_items);
                        current_active_pane = current_active_pane.or(active_pane);
                    }
                }

                if members.is_empty() {
                    return None;
                }

                if members.len() == 1 {
                    return Some((members.remove(0), current_active_pane, items));
                }

                Some((Member::Axis(PaneAxis::load(axis, members, flexes)), current_active_pane, items))
            }
            SerializedPaneGroup::Pane(serialized_pane) => {
                let pane = workspace
                    .update_in(cx, |workspace, window, cx| workspace.add_pane(window, cx).downgrade())
                    .log_err()?;
                let active = serialized_pane.active;
                let new_items = serialized_pane
                    .deserialize_to(project, &pane, workspace.clone(), cx)
                    .await
                    .context("Could not deserialize pane)")
                    .log_err()?;

                if pane.read_with(cx, |pane, _| pane.items_len() != 0).log_err()? {
                    let pane = pane.upgrade()?;
                    Some((Member::Pane(pane.clone()), active.then_some(pane), new_items))
                } else {
                    let pane = pane.upgrade()?;
                    workspace
                        .update_in(cx, |workspace, window, cx| workspace.force_remove_pane(&pane, &None, window, cx))
                        .log_err()?;
                    None
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Default, Clone, Serialize, Deserialize)]
pub struct SerializedPane {
    #[serde(default)]
    pub(crate) active: bool,
    #[serde(default, rename = "items")]
    pub(crate) children: Vec<SerializedItem>,
    #[serde(default)]
    pub(crate) pinned_count: usize,
}

impl SerializedPane {
    pub const fn new(children: Vec<SerializedItem>, active: bool, pinned_count: usize) -> Self {
        SerializedPane { children, active, pinned_count }
    }

    pub async fn deserialize_to(
        &self,
        project: &Entity<Project>,
        pane: &WeakEntity<Pane>,
        workspace: WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> Result<Vec<Option<Box<dyn ItemHandle>>>> {
        let mut item_tasks = Vec::new();
        let mut active_item_index = None;
        let mut preview_item_index = None;
        for (index, item) in self.children.iter().enumerate() {
            let project = project.clone();
            item_tasks.push(pane.update_in(cx, |_, window, cx| {
                SerializableItemRegistry::deserialize(
                    &item.kind,
                    project,
                    workspace.clone(),
                    item.state.clone(),
                    window,
                    cx,
                )
            })?);
            if item.active {
                active_item_index = Some(index);
            }
            if item.preview {
                preview_item_index = Some(index);
            }
        }

        let mut items = Vec::new();
        for item_handle in futures::future::join_all(item_tasks).await {
            let item_handle = item_handle.log_err();
            items.push(item_handle.clone());

            if let Some(item_handle) = item_handle {
                pane.update_in(cx, |pane, window, cx| {
                    pane.add_item(item_handle.clone(), true, true, None, window, cx);
                })?;
            }
        }

        if let Some(active_item) = active_item_index.and_then(|index| items.get(index)?.clone()) {
            pane.update_in(cx, |pane, window, cx| {
                if let Some(index) = pane.index_for_item(active_item.as_ref()) {
                    pane.activate_item(index, false, false, window, cx);
                }
            })?;
        }

        if let Some(preview_item) = preview_item_index.and_then(|index| items.get(index)?.clone()) {
            pane.update(cx, |pane, cx| {
                pane.set_preview_item_id(Some(preview_item.item_id()), cx);
            })?;
        }

        // `items` keeps a `None` for every item that failed to deserialize, and those
        // were never added to the pane. Counting them would leave the pinned count
        // pointing past the pinned tabs and pin unpinned ones in their place.
        let pinned_count = items.iter().take(self.pinned_count).filter(|item| item.is_some()).count();
        pane.update(cx, |pane, _| {
            pane.set_pinned_count(pinned_count);
        })?;

        anyhow::Ok(items)
    }
}

pub type ItemId = u64;

/// A tab: its kind, which picks the item type that restores it, and the state
/// that item type saved.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct SerializedItem {
    pub kind: String,
    #[serde(default)]
    pub state: serde_json::Value,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::Axis;

    use super::DockData;
    use super::DockStructure;
    use super::SerializedItem;
    use super::SerializedPane;
    use super::SerializedPaneGroup;
    use super::SerializedWorkspace;
    use crate::persistence::WindowBoundsJson;
    use crate::persistence::parse_workspace_state;

    #[test]
    fn test_serialized_workspace_round_trip() {
        let workspace = SerializedWorkspace {
            center_group: SerializedPaneGroup::Group {
                axis: Axis::Horizontal,
                flexes: Some(vec![0.5, 0.5]),
                children: vec![
                    SerializedPaneGroup::Pane(SerializedPane::new(
                        vec![SerializedItem {
                            kind: "Editor".into(),
                            state: serde_json::json!({"abs_path": "/p/a.rs"}),
                            active: true,
                            preview: false,
                        }],
                        true,
                        1,
                    )),
                    SerializedPaneGroup::default(),
                ],
            },
            docks: DockStructure {
                left: DockData { visible: true, active_panel: Some("ProjectPanel".into()), zoom: false },
                ..DockStructure::default()
            },
            window_bounds: Some(WindowBoundsJson::Windowed { x: 1, y: 2, width: 3, height: 4 }),
            display: None,
            recent_navigation_history: vec![PathBuf::from("/p/a.rs")],
        };
        let json = serde_json::to_string_pretty(&workspace).unwrap();
        assert_eq!(parse_workspace_state(&json), Some(workspace));
    }

    #[test]
    fn test_parse_workspace_state_tolerates_missing_fields() {
        assert_eq!(parse_workspace_state("{}"), Some(SerializedWorkspace::default()));
        assert_eq!(parse_workspace_state("garbage"), None);
        let workspace =
            parse_workspace_state(r#"{"center_group": {"type": "pane", "items": [{"kind": "Editor"}]}}"#).unwrap();
        let SerializedPaneGroup::Pane(pane) = workspace.center_group else {
            panic!("expected a pane");
        };
        assert_eq!(pane.children[0].state, serde_json::Value::Null);
        assert!(!pane.children[0].active);
    }
}
