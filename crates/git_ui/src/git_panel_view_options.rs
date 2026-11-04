use gpui::App;
use gpui::Global;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GitPanelSortBy {
    #[default]
    Path,
    Name,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum GitPanelGroupBy {
    None,
    #[default]
    Status,
    Staging,
}

/// How the git panel and the project diff list changed files. Session-wide and
/// not persisted: every window starts with the tree view grouped by status.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GitPanelViewOptions {
    pub sort_by: GitPanelSortBy,
    pub group_by: GitPanelGroupBy,
    pub tree_view: bool,
}

impl Default for GitPanelViewOptions {
    fn default() -> Self {
        Self { sort_by: GitPanelSortBy::Path, group_by: GitPanelGroupBy::Status, tree_view: true }
    }
}

impl Global for GitPanelViewOptions {}

impl GitPanelViewOptions {
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// Applies `update` and notifies observers only when the options changed.
    pub fn update(cx: &mut App, update: impl FnOnce(&mut Self)) {
        let current = Self::get(cx);
        let mut options = current;
        update(&mut options);
        if options != current {
            cx.set_global(options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_tree_grouped_by_status() {
        assert_eq!(
            GitPanelViewOptions::default(),
            GitPanelViewOptions { sort_by: GitPanelSortBy::Path, group_by: GitPanelGroupBy::Status, tree_view: true }
        );
    }
}
