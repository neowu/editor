//! Go to definition and find references in file revisions. Language servers
//! only know the project as it is now, so a revision asks them through the
//! file's current version: at the same spot when the line is unchanged since
//! the revision, otherwise by the name under the cursor.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::result;
use std::sync::Arc;

use anyhow::Result;
use collections::HashMap;
use collections::HashSet;
use editor::GotoDefinitionKind;
use editor::RenameTarget;
use editor::SemanticsProvider;
use futures::future::Shared;
use git::repository::RepoPath;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use language::Anchor;
use language::Bias;
use language::Buffer;
use language::BufferId;
use language::BufferRow;
use language::BufferSnapshot;
use language::CharKind;
use language::OffsetRangeExt as _;
use language::Point;
use language::ToOffset as _;
use language::ToPoint as _;
use lsp::LanguageServerId;
use project::DocumentHighlight;
use project::InvalidationStrategy;
use project::Location;
use project::LocationLink;
use project::Project;
use project::ProjectPath;
use project::ProjectTransaction;
use project::Symbol;
use project::git_store::Repository;
use project::lsp_store::BufferSemanticTokens;
use project::lsp_store::CacheInlayHints;
use project::lsp_store::OpenLspBufferHandle;
use project::lsp_store::SymbolLocation;
use util::ResultExt as _;

use crate::commit_view::project_file_path;

/// The project's semantics for a commit's file revisions, in its diff and
/// revision tabs, with definitions looked up through each revision's file in
/// the project (if it still is).
pub(crate) struct RevisionSemanticsProvider {
    project: WeakEntity<Project>,
    repository: WeakEntity<Repository>,
    live_buffers: Rc<RefCell<HashMap<ProjectPath, LiveBuffer>>>,
}

/// A file's current buffer, kept open with its language servers for as long
/// as the provider is.
struct LiveBuffer {
    buffer: Entity<Buffer>,
    _lsp_handle: OpenLspBufferHandle,
}

impl RevisionSemanticsProvider {
    pub(crate) fn new(project: WeakEntity<Project>, repository: WeakEntity<Repository>) -> RevisionSemanticsProvider {
        RevisionSemanticsProvider { project, repository, live_buffers: Rc::default() }
    }

    /// The project file that `revision` is a version of, while it still is one.
    fn live_path(&self, revision: &Entity<Buffer>, cx: &App) -> Option<ProjectPath> {
        let file = revision.read(cx).file()?;
        let project = self.project.upgrade()?;
        let repository = self.repository.upgrade()?;
        project_file_path(&project, &repository, &RepoPath::from_rel_path(file.path()), cx)
    }

    /// Opens the current file at `live_path` and registers it with its
    /// language servers, once.
    fn open_live_buffer(&self, live_path: Option<ProjectPath>, cx: &mut App) -> Option<Task<Result<Entity<Buffer>>>> {
        let live_path = live_path?;
        if let Some(live_buffer) = self.live_buffers.borrow().get(&live_path) {
            return Some(Task::ready(Ok(live_buffer.buffer.clone())));
        }
        let project = self.project.upgrade()?;
        let open = project.update(cx, |project, cx| project.open_buffer(live_path.clone(), cx));
        let cache = self.live_buffers.clone();
        Some(cx.spawn(async move |cx| {
            let buffer = open.await?;
            if !cache.borrow().contains_key(&live_path) {
                let lsp_handle =
                    project.update(cx, |project, cx| project.register_buffer_with_language_servers(&buffer, cx));
                cache.borrow_mut().insert(live_path, LiveBuffer { buffer: buffer.clone(), _lsp_handle: lsp_handle });
            }
            Ok(buffer)
        }))
    }
}

/// The row that `row` of the old text is at in the new text, or `None` when
/// a hunk of `hunks` (old and new row ranges, in order) changed it.
fn map_unchanged_row(hunks: &[(Range<u32>, Range<u32>)], row: u32) -> Option<u32> {
    let mut new_row = i64::from(row);
    for (old_rows, new_rows) in hunks {
        if row < old_rows.start {
            break;
        }
        if row < old_rows.end {
            return None;
        }
        new_row += i64::from(new_rows.end - new_rows.start) - i64::from(old_rows.end - old_rows.start);
    }
    u32::try_from(new_row).ok()
}

/// Moves `origin`, a range on `live_row` of the current file, onto the same
/// line at `revision_row`. `None` when it spans other lines.
fn map_origin_to_revision(origin: Range<Point>, live_row: u32, revision_row: u32) -> Option<Range<Point>> {
    (origin.start.row == live_row && origin.end.row == live_row)
        .then(|| Point::new(revision_row, origin.start.column)..Point::new(revision_row, origin.end.column))
}

/// The symbols named exactly `name`, keeping only those in `live_path` when
/// the revision's own file defines it.
fn exact_symbol_matches(symbols: Vec<Symbol>, name: &str, live_path: Option<&ProjectPath>) -> Vec<Symbol> {
    let named: Vec<Symbol> = symbols.into_iter().filter(|symbol| symbol.name == name).collect();
    let in_live_file = |symbol: &Symbol| {
        live_path.is_some_and(|live_path| matches!(&symbol.path, SymbolLocation::InProject(path) if path == live_path))
    };
    if named.iter().any(in_live_file) { named.into_iter().filter(in_live_file).collect() } else { named }
}

/// The spot in the current file at `point` of the revision, with its row,
/// when the revision's line there is unchanged.
async fn live_position(
    live_buffer: Task<Result<Entity<Buffer>>>,
    revision: &BufferSnapshot,
    point: Point,
    cx: &AsyncApp,
) -> Result<Option<(Entity<Buffer>, Anchor, u32)>> {
    let live_buffer = live_buffer.await?;
    let live = live_buffer.read_with(cx, |buffer, _| buffer.snapshot());
    let revision_text = revision.text();
    let live_text = live.text();
    let hunks = cx.background_spawn(async move { language::line_diff(&revision_text, &live_text) }).await;
    let Some(live_row) = map_unchanged_row(&hunks, point.row) else {
        return Ok(None);
    };
    let live_point = live.clip_point(Point::new(live_row, point.column), Bias::Left);
    Ok(Some((live_buffer, live.anchor_before(live_point), live_row)))
}

/// Definitions at `point` of the current file, when the revision's line there
/// is unchanged, with their origins moved back onto the revision.
async fn definitions_at_live_line(
    project: WeakEntity<Project>,
    live_buffer: Task<Result<Entity<Buffer>>>,
    revision_buffer: Entity<Buffer>,
    revision: BufferSnapshot,
    point: Point,
    kind: GotoDefinitionKind,
    cx: &AsyncApp,
) -> Result<Option<Vec<LocationLink>>> {
    let Some((live_buffer, live_anchor, live_row)) = live_position(live_buffer, &revision, point, cx).await? else {
        return Ok(None);
    };
    let task = cx.update(|cx| project.definitions(&live_buffer, live_anchor, kind, cx));
    let Some(task) = task else {
        return Ok(None);
    };
    let Some(links) = task.await? else {
        return Ok(None);
    };
    let links = cx.update(|cx| {
        links
            .into_iter()
            .map(|link| {
                let origin = link.origin.and_then(|origin| {
                    let range = origin.range.to_point(origin.buffer.read(cx));
                    let range = map_origin_to_revision(range, live_row, point.row)?;
                    Some(Location {
                        buffer: revision_buffer.clone(),
                        range: revision.anchor_before(range.start)..revision.anchor_after(range.end),
                    })
                });
                LocationLink { origin, target: link.target }
            })
            .collect::<Vec<LocationLink>>()
    });
    Ok((!links.is_empty()).then_some(links))
}

/// References at `point` of the current file, when the revision's line there
/// is unchanged.
async fn references_at_live_line(
    project: WeakEntity<Project>,
    live_buffer: Task<Result<Entity<Buffer>>>,
    revision: BufferSnapshot,
    point: Point,
    cx: &AsyncApp,
) -> Result<Option<Vec<Location>>> {
    let Some((live_buffer, live_anchor, _)) = live_position(live_buffer, &revision, point, cx).await? else {
        return Ok(None);
    };
    let Some(task) = cx.update(|cx| project.references(&live_buffer, live_anchor, cx)) else {
        return Ok(None);
    };
    Ok(task.await?.filter(|locations| !locations.is_empty()))
}

/// References of the one project symbol named like the word at `point`. With
/// several, the name alone can't tell which one the revision meant.
async fn references_by_name(
    project: WeakEntity<Project>,
    live_path: Option<ProjectPath>,
    revision_buffer: Entity<Buffer>,
    revision: BufferSnapshot,
    point: Point,
    cx: &mut AsyncApp,
) -> Result<Option<Vec<Location>>> {
    let links = definitions_by_name(project.clone(), live_path, revision_buffer, revision, point, cx).await?;
    let Some([link]) = links.as_deref() else {
        return Ok(None);
    };
    let target = link.target.clone();
    let project = project.upgrade().ok_or_else(|| anyhow::anyhow!("project dropped"))?;
    // The symbol's file may not be open, and language servers only answer for
    // files they were told about.
    let _lsp_handle =
        project.update(cx, |project, cx| project.register_buffer_with_language_servers(&target.buffer, cx));
    let references = project.update(cx, |project, cx| project.references(&target.buffer, target.range.start, cx));
    references.await
}

/// Definitions of the project symbols named like the word at `point`.
async fn definitions_by_name(
    project: WeakEntity<Project>,
    live_path: Option<ProjectPath>,
    revision_buffer: Entity<Buffer>,
    revision: BufferSnapshot,
    point: Point,
    cx: &mut AsyncApp,
) -> Result<Option<Vec<LocationLink>>> {
    let (word_range, word_kind) = revision.surrounding_word(point.to_offset(&revision), None);
    if word_kind != Some(CharKind::Word) {
        return Ok(None);
    }
    let name: String = revision.text_for_range(word_range.clone()).collect();
    let project = project.upgrade().ok_or_else(|| anyhow::anyhow!("project dropped"))?;
    let symbols = project.update(cx, |project, cx| project.symbols(&name, cx)).await?;
    let mut matches = exact_symbol_matches(symbols, &name, live_path.as_ref());
    if matches.is_empty() {
        // rust-analyzer only searches types unless the query ends with `#`.
        let symbols = project.update(cx, |project, cx| project.symbols(&format!("{name}#"), cx)).await?;
        matches = exact_symbol_matches(symbols, &name, live_path.as_ref());
    }
    let origin = Location {
        buffer: revision_buffer,
        range: revision.anchor_before(word_range.start)..revision.anchor_after(word_range.end),
    };
    let mut links = Vec::new();
    for symbol in matches {
        let open = project.update(cx, |project, cx| project.open_buffer_for_symbol(&symbol, cx));
        let Some(buffer) = open.await.log_err() else {
            continue;
        };
        let range = buffer.read_with(cx, |buffer, _| {
            let start = buffer.clip_point_utf16(symbol.range.start, Bias::Left);
            let end = buffer.clip_point_utf16(symbol.range.end, Bias::Left);
            buffer.anchor_before(start)..buffer.anchor_after(end)
        });
        links.push(LocationLink { origin: Some(origin.clone()), target: Location { buffer, range } });
    }
    Ok((!links.is_empty()).then_some(links))
}

impl SemanticsProvider for RevisionSemanticsProvider {
    fn hover(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        cx: &mut App,
    ) -> Option<Task<Option<Vec<project::Hover>>>> {
        self.project.hover(buffer, position, cx)
    }

    fn applicable_inlay_chunks(
        &self,
        buffer: &Entity<Buffer>,
        ranges: &[Range<Anchor>],
        cx: &mut App,
    ) -> Vec<Range<BufferRow>> {
        self.project.applicable_inlay_chunks(buffer, ranges, cx)
    }

    fn invalidate_inlay_hints(&self, for_buffers: &HashSet<BufferId>, cx: &mut App) {
        self.project.invalidate_inlay_hints(for_buffers, cx)
    }

    fn inlay_hints(
        &self,
        invalidate: InvalidationStrategy,
        buffer: Entity<Buffer>,
        ranges: Vec<Range<Anchor>>,
        known_chunks: Option<(clock::Global, HashSet<Range<BufferRow>>)>,
        cx: &mut App,
    ) -> Option<HashMap<Range<BufferRow>, Task<Result<CacheInlayHints>>>> {
        self.project.inlay_hints(invalidate, buffer, ranges, known_chunks, cx)
    }

    fn semantic_tokens(
        &self,
        buffer: Entity<Buffer>,
        cx: &mut App,
    ) -> Option<Shared<Task<result::Result<BufferSemanticTokens, Arc<anyhow::Error>>>>> {
        self.project.semantic_tokens(buffer, cx)
    }

    fn supports_inlay_hints(&self, buffer: &Entity<Buffer>, cx: &mut App) -> bool {
        self.project.supports_inlay_hints(buffer, cx)
    }

    fn supports_semantic_tokens(&self, buffer: &Entity<Buffer>, cx: &mut App) -> bool {
        self.project.supports_semantic_tokens(buffer, cx)
    }

    fn supports_document_highlights(&self, buffer: &Entity<Buffer>, cx: &mut App) -> bool {
        self.project.supports_document_highlights(buffer, cx)
    }

    fn document_highlights(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        cx: &mut App,
    ) -> Option<Task<Result<Vec<DocumentHighlight>>>> {
        self.project.document_highlights(buffer, position, cx)
    }

    fn definitions(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        kind: GotoDefinitionKind,
        cx: &mut App,
    ) -> Option<Task<Result<Option<Vec<LocationLink>>>>> {
        let revision = buffer.read(cx).snapshot();
        let point = position.to_point(&revision);
        let live_path = self.live_path(buffer, cx);
        let live_buffer = self.open_live_buffer(live_path.clone(), cx);
        let project = self.project.clone();
        let revision_buffer = buffer.clone();
        Some(cx.spawn(async move |cx| {
            if let Some(live_buffer) = live_buffer {
                let links = definitions_at_live_line(
                    project.clone(),
                    live_buffer,
                    revision_buffer.clone(),
                    revision.clone(),
                    point,
                    kind,
                    cx,
                )
                .await
                .log_err()
                .flatten();
                if links.is_some() {
                    return Ok(links);
                }
            }
            if kind != GotoDefinitionKind::Symbol {
                return Ok(None);
            }
            definitions_by_name(project, live_path, revision_buffer, revision, point, cx).await
        }))
    }

    fn references(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        cx: &mut App,
    ) -> Option<Task<Result<Option<Vec<Location>>>>> {
        let revision = buffer.read(cx).snapshot();
        let point = position.to_point(&revision);
        let live_path = self.live_path(buffer, cx);
        let live_buffer = self.open_live_buffer(live_path.clone(), cx);
        let project = self.project.clone();
        let revision_buffer = buffer.clone();
        Some(cx.spawn(async move |cx| {
            if let Some(live_buffer) = live_buffer {
                let locations = references_at_live_line(project.clone(), live_buffer, revision.clone(), point, cx)
                    .await
                    .log_err()
                    .flatten();
                if locations.is_some() {
                    return Ok(locations);
                }
            }
            references_by_name(project, live_path, revision_buffer, revision, point, cx).await
        }))
    }

    fn range_for_rename(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        cx: &mut App,
    ) -> Task<Result<Option<RenameTarget>>> {
        self.project.range_for_rename(buffer, position, cx)
    }

    fn perform_rename(
        &self,
        buffer: &Entity<Buffer>,
        position: Anchor,
        new_name: String,
        language_server_id: Option<LanguageServerId>,
        cx: &mut App,
    ) -> Option<Task<Result<ProjectTransaction>>> {
        self.project.perform_rename(buffer, position, new_name, language_server_id, cx)
    }
}

#[cfg(test)]
mod tests {
    use language::CodeLabel;
    use language::Point;
    use language::PointUtf16;
    use language::SymbolKind;
    use language::Unclipped;
    use lsp::LanguageServerId;
    use lsp::LanguageServerName;
    use project::ProjectPath;
    use project::Symbol;
    use project::WorktreeId;
    use project::lsp_store::SymbolLocation;
    use util::rel_path::RelPath;

    use super::exact_symbol_matches;
    use super::map_origin_to_revision;
    use super::map_unchanged_row;

    #[test]
    fn test_map_unchanged_row() {
        assert_eq!(map_unchanged_row(&[], 7), Some(7));
        // Two lines replaced by three at rows 2..4, one line inserted before row 9.
        let hunks = [(2..4, 2..5), (9..9, 10..11)];
        assert_eq!(map_unchanged_row(&hunks, 1), Some(1));
        assert_eq!(map_unchanged_row(&hunks, 2), None);
        assert_eq!(map_unchanged_row(&hunks, 3), None);
        assert_eq!(map_unchanged_row(&hunks, 4), Some(5));
        assert_eq!(map_unchanged_row(&hunks, 9), Some(11));
        // Lines deleted above shift rows up.
        assert_eq!(map_unchanged_row(&[(0..3, 0..0)], 5), Some(2));
    }

    #[test]
    fn test_map_unchanged_row_through_line_diff() {
        let revision = "use a;\nfn one() {}\nfn two() {}\nfn three() {}\n";
        let live = "use a;\nuse b;\n\nfn one() {}\nfn two() { changed }\nfn three() {}\n";
        let hunks = language::line_diff(revision, live);
        assert_eq!(map_unchanged_row(&hunks, 0), Some(0));
        assert_eq!(map_unchanged_row(&hunks, 1), Some(3));
        assert_eq!(map_unchanged_row(&hunks, 2), None);
        assert_eq!(map_unchanged_row(&hunks, 3), Some(5));
    }

    #[test]
    fn test_map_origin_to_revision() {
        let origin = Point::new(10, 4)..Point::new(10, 9);
        assert_eq!(map_origin_to_revision(origin, 10, 3), Some(Point::new(3, 4)..Point::new(3, 9)));
        assert_eq!(map_origin_to_revision(Point::new(10, 4)..Point::new(11, 0), 10, 3), None);
        assert_eq!(map_origin_to_revision(Point::new(9, 4)..Point::new(9, 6), 10, 3), None);
    }

    fn symbol(name: &str, path: &str) -> Symbol {
        Symbol {
            language_server_name: LanguageServerName::new_static("test"),
            source_worktree_id: WorktreeId::from_usize(1),
            source_language_server_id: LanguageServerId(0),
            path: SymbolLocation::InProject(ProjectPath {
                worktree_id: WorktreeId::from_usize(1),
                path: RelPath::new_test(path).into_arc(),
            }),
            label: CodeLabel::plain(name.to_string(), None),
            name: name.to_string(),
            kind: SymbolKind::Function,
            range: Unclipped(PointUtf16::new(0, 0))..Unclipped(PointUtf16::new(0, 1)),
            container_name: None,
        }
    }

    fn paths(symbols: &[Symbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|symbol| match &symbol.path {
                SymbolLocation::InProject(path) => path.path.as_unix_str().to_string(),
                SymbolLocation::OutsideProject { .. } => String::new(),
            })
            .collect()
    }

    #[test]
    fn test_exact_symbol_matches() {
        let symbols = || vec![symbol("open", "a.rs"), symbol("open_all", "b.rs"), symbol("open", "c.rs")];
        let live_path =
            ProjectPath { worktree_id: WorktreeId::from_usize(1), path: RelPath::new_test("c.rs").into_arc() };
        // Fuzzy matches are dropped.
        assert_eq!(paths(&exact_symbol_matches(symbols(), "open", None)), ["a.rs", "c.rs"]);
        // The revision's own file wins when it defines the name.
        assert_eq!(paths(&exact_symbol_matches(symbols(), "open", Some(&live_path))), ["c.rs"]);
        let other_path =
            ProjectPath { worktree_id: WorktreeId::from_usize(1), path: RelPath::new_test("d.rs").into_arc() };
        assert_eq!(paths(&exact_symbol_matches(symbols(), "open", Some(&other_path))), ["a.rs", "c.rs"]);
        assert!(exact_symbol_matches(symbols(), "close", None).is_empty());
    }
}
