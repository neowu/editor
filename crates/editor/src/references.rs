//! Find-references results that can hide the references in test code.

use std::ops::Range;

use futures::future;
use gpui::App;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::Window;
use language::Buffer;
use language::BufferSnapshot;
use language::Point;
use multi_buffer::PathKey;
use text::OffsetRangeExt as _;
use text::ToOffset as _;

use crate::EXCERPT_CONTEXT_LINES;
use crate::Editor;
use crate::HighlightKey;
use crate::actions::ToggleTestReferences;

pub(crate) struct ReferenceFile {
    buffer: Entity<Buffer>,
    path_key: PathKey,
    ranges: Vec<Range<text::Anchor>>,
    /// The references left while test references are hidden, if that dropped any.
    filtered: Option<Vec<Range<text::Anchor>>>,
}

impl ReferenceFile {
    pub(crate) fn new(buffer: Entity<Buffer>, path_key: PathKey, ranges: &[Range<Point>], cx: &App) -> Self {
        let snapshot = buffer.read(cx).text_snapshot();
        let ranges = ranges.iter().map(|range| snapshot.anchor_range_inside(range.clone())).collect();
        Self { buffer, path_key, ranges, filtered: None }
    }

    fn visible_ranges(&self) -> &[Range<text::Anchor>] {
        self.filtered.as_deref().unwrap_or(&self.ranges)
    }
}

/// Whether each of `ranges` is test code: by the file's path, else by enclosing test items.
fn test_flags(snapshot: &BufferSnapshot, ranges: &[Range<text::Anchor>]) -> Vec<bool> {
    if snapshot.file().is_some_and(|file| language::is_test_path(file.path().as_unix_str())) {
        return vec![true; ranges.len()];
    }
    // Only the root layer: a reference inside a macro call sits in an injected layer
    // that doesn't reach the enclosing items.
    let Some(root) = snapshot.syntax_layers().next().map(|layer| layer.node()) else {
        return vec![false; ranges.len()];
    };
    ranges
        .iter()
        .map(|range| {
            let range = range.start.to_offset(snapshot)..range.end.to_offset(snapshot);
            language::in_test_item(root, range, |range| snapshot.text_for_range(range).collect())
        })
        .collect()
}

/// `ranges` without those flagged as test code, or `None` when none is.
fn without_tests<T: Clone>(ranges: &[T], is_test: &[bool]) -> Option<Vec<T>> {
    is_test
        .contains(&true)
        .then(|| ranges.iter().zip(is_test).filter(|(_, is_test)| !**is_test).map(|(range, _)| range.clone()).collect())
}

/// Drops every file's filter, returning the indices of the files that had one.
fn clear_filter(files: &mut [ReferenceFile]) -> Vec<usize> {
    let mut changed = Vec::new();
    for (ix, file) in files.iter_mut().enumerate() {
        if file.filtered.take().is_some() {
            changed.push(ix);
        }
    }
    changed
}

/// The header menu entry that toggles test references.
pub(crate) const fn test_references_label(hidden: bool) -> &'static str {
    if hidden { "Show Test References" } else { "Hide Test References" }
}

/// The tab title, which says when test references are hidden. The marker leads, since
/// tab titles are cut to `MAX_TAB_TITLE_LEN`.
fn results_title(title: &str, tests_hidden: bool) -> String {
    match title.strip_prefix("References") {
        Some(rest) if tests_hidden => format!("Non-test refs{rest}"),
        _ => title.to_owned(),
    }
}

pub(crate) struct ReferenceResults {
    files: Vec<ReferenceFile>,
    title: String,
    hide_tests: bool,
    /// Parses the result files, then hides their test references.
    hide_task: Task<()>,
}

impl ReferenceResults {
    pub(crate) const fn new(files: Vec<ReferenceFile>, title: String) -> Self {
        Self { files, title, hide_tests: false, hide_task: Task::ready(()) }
    }
}

impl Editor {
    /// Whether test references are hidden; `None` unless this editor shows find-references results.
    pub fn hides_test_references(&self) -> Option<bool> {
        self.reference_results.as_ref().map(|results| results.hide_tests)
    }

    pub fn toggle_test_references(&mut self, _: &ToggleTestReferences, _: &mut Window, cx: &mut Context<Self>) {
        let Some(results) = self.reference_results.as_mut() else {
            return;
        };
        results.hide_tests = !results.hide_tests;
        if !results.hide_tests {
            results.hide_task = Task::ready(());
            let changed = clear_filter(&mut results.files);
            self.apply_reference_filter(&changed, cx);
            return;
        }
        // Telling tests apart needs the syntax tree, and a buffer no editor has shown
        // has its parse deferred. Only pay for it here, not when the results open.
        let parsing: Vec<_> = results
            .files
            .iter()
            .map(|file| {
                file.buffer.update(cx, |buffer, cx| {
                    buffer.ensure_parsed(cx);
                    buffer.parsing_idle()
                })
            })
            .collect();
        results.hide_task = cx.spawn(async move |editor, cx| {
            future::join_all(parsing).await;
            editor
                .update(cx, |editor, cx| {
                    let Some(results) = editor.reference_results.as_mut() else {
                        return;
                    };
                    for file in &mut results.files {
                        let snapshot = file.buffer.read(cx).snapshot();
                        file.filtered = without_tests(&file.ranges, &test_flags(&snapshot, &file.ranges));
                    }
                    // Nothing but tests: keep showing them rather than an empty tab.
                    if results.files.iter().all(|file| file.visible_ranges().is_empty()) {
                        results.hide_tests = false;
                        clear_filter(&mut results.files);
                        cx.notify();
                        return;
                    }
                    let changed: Vec<_> =
                        (0..results.files.len()).filter(|ix| results.files[*ix].filtered.is_some()).collect();
                    editor.apply_reference_filter(&changed, cx);
                })
                .ok();
        });
    }

    /// Rebuilds the excerpts of the `changed` files (indices) to match the filter.
    fn apply_reference_filter(&mut self, changed: &[usize], cx: &mut Context<Self>) {
        let Some(results) = self.reference_results.as_ref() else {
            return;
        };
        let title = results_title(&results.title, results.hide_tests);
        let mut highlights = Vec::new();
        self.buffer.update(cx, |multibuffer, cx| {
            for file in changed.iter().map(|ix| &results.files[*ix]) {
                let visible = file.visible_ranges();
                if visible.is_empty() {
                    multibuffer.remove_excerpts(file.path_key.clone(), cx);
                } else {
                    let snapshot = file.buffer.read(cx).text_snapshot();
                    multibuffer.set_excerpts_for_path(
                        file.path_key.clone(),
                        file.buffer.clone(),
                        visible.iter().map(|range| range.to_point(&snapshot)),
                        EXCERPT_CONTEXT_LINES,
                        cx,
                    );
                }
            }
            let snapshot = multibuffer.snapshot(cx);
            for file in &results.files {
                highlights.extend(file.visible_ranges().iter().filter_map(|range| {
                    Some(snapshot.anchor_in_buffer(range.start)?..snapshot.anchor_in_buffer(range.end)?)
                }));
            }
            highlights.sort_by(|a, b| a.start.cmp(&b.start, &snapshot));
            multibuffer.set_title(title, cx);
        });
        self.highlight_background(
            HighlightKey::Editor,
            &highlights,
            |_, theme| theme.colors().editor_highlighted_line_background,
            cx,
        );
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_tests_keeps_the_rest() {
        assert_eq!(without_tests(&[1, 2, 3, 4], &[false, true, false, true]), Some(vec![1, 3]));
        assert_eq!(without_tests(&[1], &[true]), Some(vec![]));
        assert_eq!(without_tests(&[1, 2], &[false, false]), None);
    }

    #[test]
    fn references_label_names_the_next_state() {
        assert_eq!(test_references_label(false), "Hide Test References");
        assert_eq!(test_references_label(true), "Show Test References");
    }

    #[test]
    fn results_title_marks_hidden_tests() {
        assert_eq!(results_title("References to poll", false), "References to poll");
        assert_eq!(results_title("References to poll", true), "Non-test refs to poll");
        assert_eq!(results_title("References", true), "Non-test refs");
    }
}
