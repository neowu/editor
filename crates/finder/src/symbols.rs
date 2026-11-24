use std::cmp::Reverse;
use std::sync::Arc;

use editor::Bias;
use editor::Editor;
use editor::SelectionEffects;
use editor::scroll::Autoscroll;
use editor::styled_runs_for_code_label;
use fuzzy::StringMatch;
use fuzzy::StringMatchCandidate;
use gpui::App;
use gpui::Context;
use gpui::DismissEvent;
use gpui::Entity;
use gpui::HighlightStyle;
use gpui::ParentElement as _;
use gpui::SharedString;
use gpui::Styled as _;
use gpui::StyledText;
use gpui::Task;
use gpui::TaskExt as _;
use gpui::TextStyle;
use gpui::WeakEntity;
use gpui::Window;
use gpui::relative;
use ordered_float::OrderedFloat;
use picker::Picker;
use picker::PickerDelegate;
use project::Project;
use project::Symbol;
use project::lsp_store::SymbolLocation;
use ui::LabelLike;
use ui::ListItem;
use ui::ListItemSpacing;
use ui::font;
use ui::prelude::Color;
use ui::prelude::Label;
use ui::prelude::LabelCommon as _;
use ui::prelude::LabelSize;
use ui::prelude::Toggleable as _;
use ui::prelude::h_flex;
use util::ResultExt as _;
use workspace::Workspace;

/// Searches the project's symbols through the language servers.
pub(crate) struct SymbolsDelegate {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    selected_match_index: usize,
    symbols: Vec<Symbol>,
    visible_match_candidates: Vec<StringMatchCandidate>,
    external_match_candidates: Vec<StringMatchCandidate>,
    show_worktree_root_name: bool,
    matches: Vec<StringMatch>,
}

impl SymbolsDelegate {
    pub(crate) fn new(workspace: WeakEntity<Workspace>, project: Entity<Project>) -> Self {
        Self {
            workspace,
            project,
            selected_match_index: 0,
            symbols: Vec::default(),
            visible_match_candidates: Vec::default(),
            external_match_candidates: Vec::default(),
            matches: Vec::default(),
            show_worktree_root_name: false,
        }
    }

    fn filter(&mut self, query: &str, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        const MAX_MATCHES: usize = 100;
        let mut visible_matches = fuzzy::match_strings(
            &self.visible_match_candidates,
            query,
            fuzzy::Case::Ignore,
            fuzzy::LengthPenalty::On,
            MAX_MATCHES,
        );
        let mut external_matches = fuzzy::match_strings(
            &self.external_match_candidates,
            query,
            fuzzy::Case::Ignore,
            fuzzy::LengthPenalty::On,
            MAX_MATCHES - visible_matches.len().min(MAX_MATCHES),
        );
        let sort_key_for_match = |mat: &StringMatch| {
            let symbol = &self.symbols[mat.candidate_id];
            (Reverse(OrderedFloat(mat.score)), symbol.label.filter_text())
        };

        visible_matches.sort_unstable_by_key(sort_key_for_match);
        external_matches.sort_unstable_by_key(sort_key_for_match);
        let mut matches = visible_matches;
        matches.append(&mut external_matches);

        for mat in &mut matches {
            let symbol = &self.symbols[mat.candidate_id];
            let filter_start = symbol.label.filter_range.start;
            for position in &mut mat.positions {
                *position += filter_start;
            }
        }

        self.matches = matches;
        self.set_selected_index(0, window, cx);
    }
}

impl PickerDelegate for SymbolsDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search project symbols...".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("No symbols".into())
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(symbol) =
            self.matches.get(self.selected_match_index).map(|mat| self.symbols[mat.candidate_id].clone())
        {
            let buffer = self.project.update(cx, |project, cx| project.open_buffer_for_symbol(&symbol, cx));
            let symbol = symbol.clone();
            let workspace = self.workspace.clone();
            cx.spawn_in(window, async move |_, cx| {
                let buffer = buffer.await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    let position = buffer.read(cx).clip_point_utf16(symbol.range.start, Bias::Left);
                    let pane =
                        if secondary { workspace.adjacent_pane(window, cx) } else { workspace.active_pane().clone() };

                    let editor = workspace.open_project_item::<Editor>(
                        secondary.then_some(pane),
                        buffer,
                        true,
                        true,
                        true,
                        true,
                        window,
                        cx,
                    );

                    editor.update(cx, |editor, cx| {
                        let multibuffer_snapshot = editor.buffer().read(cx).snapshot(cx);
                        let Some(buffer_snapshot) = multibuffer_snapshot.as_singleton() else {
                            return;
                        };
                        let text_anchor = buffer_snapshot.anchor_before(position);
                        let Some(anchor) = multibuffer_snapshot.anchor_in_buffer(text_anchor) else {
                            return;
                        };
                        editor.change_selections(
                            SelectionEffects::scroll(Autoscroll::center()).unfold(),
                            window,
                            cx,
                            |s| s.select_ranges([anchor..anchor]),
                        );
                    });
                })?;
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
            cx.emit(DismissEvent);
        }
    }

    fn dismissed(&mut self, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {}

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_match_index
    }

    fn set_selected_index(&mut self, ix: usize, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {
        self.selected_match_index = ix;
    }

    fn update_matches(&mut self, query: String, window: &mut Window, cx: &mut Context<Picker<Self>>) -> Task<()> {
        // Try to support rust-analyzer's path based symbols feature which
        // allows to search by rust path syntax, in that case we only want to
        // filter names by the last segment
        // Ideally this was a first class LSP feature (rich queries)
        let query_filter = query.rsplit_once("::").map_or(&*query, |(_, suffix)| suffix).to_owned();
        self.filter(&query_filter, window, cx);
        self.show_worktree_root_name = self.project.read(cx).visible_worktrees(cx).count() > 1;
        let symbols = self.project.update(cx, |project, cx| project.symbols(&query, cx));
        cx.spawn_in(window, async move |this, cx| {
            let symbols = symbols.await.log_err();
            if let Some(symbols) = symbols {
                this.update_in(cx, |this, window, cx| {
                    let delegate = &mut this.delegate;
                    let project = delegate.project.read(cx);
                    let (visible_match_candidates, external_match_candidates) = symbols
                        .iter()
                        .enumerate()
                        .map(|(id, symbol)| StringMatchCandidate::new(id, symbol.label.filter_text()))
                        .partition(|candidate| {
                            if let SymbolLocation::InProject(path) = &symbols[candidate.id].path {
                                project.entry_for_path(path, cx).is_some_and(|e| !e.is_ignored)
                            } else {
                                false
                            }
                        });

                    delegate.visible_match_candidates = visible_match_candidates;
                    delegate.external_match_candidates = external_match_candidates;
                    delegate.symbols = symbols;
                    delegate.filter(&query_filter, window, cx);
                })
                .log_err();
            }
        })
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let string_match = &self.matches.get(ix)?;
        let symbol = &self.symbols.get(string_match.candidate_id)?;
        let theme = theme::theme();
        let cursor_colors = theme.cursor_colors();
        let syntax_runs = styled_runs_for_code_label(&symbol.label, theme.syntax(), &cursor_colors);

        let path = match &symbol.path {
            SymbolLocation::InProject(project_path) => {
                let project = self.project.read(cx);
                let mut path = project_path.path.to_rel_path_buf();
                if self.show_worktree_root_name
                    && let Some(worktree) = project.worktree_for_id(project_path.worktree_id, cx)
                {
                    path = worktree.read(cx).root_name().join(&path);
                }
                path.display().to_owned().into()
            }
            SymbolLocation::OutsideProject { abs_path } => abs_path.to_string_lossy(),
        };
        let label = symbol.label.text.clone();
        let line_number = symbol.range.start.0.row + 1;
        let path = path.into_owned();

        let text_style = TextStyle {
            color: theme::theme().colors().text,
            font_family: font::buffer_font().family.clone(),
            font_features: font::buffer_font().features.clone(),
            font_size: font::buffer_font_size(cx).into(),
            font_weight: font::buffer_font().weight,
            line_height: relative(1.),
            ..Default::default()
        };

        let highlight_style = HighlightStyle {
            background_color: Some(theme::theme().colors().text_accent.alpha(0.3)),
            ..Default::default()
        };
        let custom_highlights =
            string_match.positions.iter().map(|pos| (*pos..label.ceil_char_boundary(pos + 1), highlight_style));

        let highlights = gpui::combine_highlights(custom_highlights, syntax_runs);

        // One line per symbol, like the other finder modes: the label, then
        // its location filling the rest of the row.
        Some(
            ListItem::new(ix).inset(true).spacing(ListItemSpacing::Sparse).toggle_state(selected).child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1p5()
                    .overflow_hidden()
                    .child(
                        LabelLike::new()
                            .child(StyledText::new(&label).with_default_highlights(&text_style, highlights)),
                    )
                    .child(
                        Label::new(location_label(&path, line_number))
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .flex_1()
                            .truncate_start(),
                    ),
            ),
        )
    }
}

fn location_label(path: &str, line_number: u32) -> String {
    format!("{path}:{line_number}")
}

#[cfg(test)]
mod tests {
    use super::location_label;

    #[test]
    fn location_label_joins_path_and_line() {
        assert_eq!(location_label("src/main.rs", 12), "src/main.rs:12");
    }
}
