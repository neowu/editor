use std::ops::Range;

use anyhow::Result;
use gpui::App;
use gpui::AsyncApp;
use gpui::Entity;
use gpui::WeakEntity;
use language::Bias;
use language::Buffer;
use language::CharKind;
use language::Location;
use language::PointUtf16;
use language::ToOffset as _;
use lsp::LanguageServerName;
use text::Anchor;

use crate::LocationLink;
use crate::ProjectPath;
use crate::Symbol;
use crate::lsp_store::LspStore;
use crate::lsp_store::SymbolLocation;

/// What to look up when a server opting into
/// [`language::LspAdapter::definition_symbol_label`] finds no definition.
pub(super) struct DefinitionSymbolQuery {
    server: LanguageServerName,
    word: String,
    label: String,
    origin: Range<Anchor>,
    file: Option<ProjectPath>,
}

impl LspStore {
    pub(super) fn definition_symbol_query(
        &self,
        buffer: &Entity<Buffer>,
        position: PointUtf16,
        cx: &mut App,
    ) -> Option<DefinitionSymbolQuery> {
        let (word, origin, file) = {
            let buffer = buffer.read(cx);
            let snapshot = buffer.text_snapshot();
            let (range, kind) = buffer.snapshot().surrounding_word(position.to_offset(&snapshot), None);
            if kind != Some(CharKind::Word) {
                return None;
            }
            let word = snapshot.text_for_range(range.clone()).collect::<String>();
            let origin = snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end);
            (word, origin, buffer.file().map(|file| ProjectPath::from_file(file.as_ref(), cx)))
        };
        let (server, label) = buffer.update(cx, |buffer, cx| {
            self.local
                .language_servers_for_buffer(buffer, cx)
                .find_map(|(adapter, _)| Some((adapter.name.clone(), adapter.adapter.definition_symbol_label(&word)?)))
        })?;
        Some(DefinitionSymbolQuery { server, word, label, origin, file })
    }
}

/// Resolves `query` through workspace symbols into a single definition.
pub(super) async fn definition_from_symbols(
    this: WeakEntity<LspStore>,
    buffer: Entity<Buffer>,
    query: DefinitionSymbolQuery,
    cx: &mut AsyncApp,
) -> Result<Option<LocationLink>> {
    let symbols = this.update(cx, |this, cx| this.symbols(&query.word, cx))?.await?;
    let Some(symbol) = definition_symbol(symbols, &query.server, &query.label, query.file.as_ref()) else {
        return Ok(None);
    };
    let target = this.update(cx, |this, cx| this.open_buffer_for_symbol(&symbol, cx))?.await?;
    let range = target.read_with(cx, |target, _| {
        let start = target.clip_point_utf16(symbol.range.start, Bias::Left);
        let end = target.clip_point_utf16(symbol.range.end, Bias::Left);
        target.anchor_before(start)..target.anchor_after(end)
    });
    Ok(Some(LocationLink {
        origin: Some(Location { buffer, range: query.origin }),
        target: Location { buffer: target, range },
    }))
}

/// The one symbol from `server` whose name ends with `label`, looking in the
/// directory of `file` first and then everywhere. Ambiguity resolves to nothing.
fn definition_symbol(
    symbols: Vec<Symbol>,
    server: &LanguageServerName,
    label: &str,
    file: Option<&ProjectPath>,
) -> Option<Symbol> {
    let matches: Vec<Symbol> = symbols
        .into_iter()
        .filter(|symbol| &symbol.language_server_name == server && symbol.name.split(' ').next_back() == Some(label))
        .collect();
    let in_dir = |symbol: &Symbol| {
        file.is_some_and(|file| {
            matches!(&symbol.path, SymbolLocation::InProject(path)
                if path.worktree_id == file.worktree_id && path.path.parent() == file.path.parent())
        })
    };
    let local: Vec<&Symbol> = matches.iter().filter(|symbol| in_dir(symbol)).collect();
    match (local.as_slice(), matches.as_slice()) {
        ([symbol], _) => Some((*symbol).clone()),
        ([], [symbol]) => Some(symbol.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use language::CodeLabel;
    use language::SymbolKind;
    use language::Unclipped;
    use lsp::LanguageServerId;
    use util::rel_path::RelPath;
    use worktree::WorktreeId;

    use super::*;

    fn path(path: &str) -> ProjectPath {
        ProjectPath { worktree_id: WorktreeId::from_usize(1), path: RelPath::new_test(path).into_arc() }
    }

    fn symbol(server: &'static str, name: &str, file: &str) -> Symbol {
        Symbol {
            language_server_name: LanguageServerName::new_static(server),
            source_worktree_id: WorktreeId::from_usize(1),
            source_language_server_id: LanguageServerId(0),
            path: SymbolLocation::InProject(path(file)),
            label: CodeLabel::plain(name.to_owned(), None),
            name: name.to_owned(),
            kind: SymbolKind::Class,
            range: Unclipped(PointUtf16::new(0, 0))..Unclipped(PointUtf16::new(0, 1)),
            container_name: None,
        }
    }

    fn found(symbols: Vec<Symbol>, label: &str, file: &str) -> Option<(String, String)> {
        let file = path(file);
        definition_symbol(symbols, &LanguageServerName::new_static("tf"), label, Some(&file)).map(|symbol| {
            let SymbolLocation::InProject(path) = &symbol.path else { unreachable!() };
            (symbol.name, path.path.as_unix_str().to_owned())
        })
    }

    #[test]
    fn definition_symbol_matches_label_exactly() {
        let symbols = || {
            vec![
                symbol("tf", "module \"cert\"", "a/1.tf"),
                symbol("tf", "module \"cert_3\"", "a/2.tf"),
                symbol("other", "\"web\"", "a/3.tf"),
            ]
        };
        assert_eq!(found(symbols(), "\"cert\"", "a/main.tf"), Some(("module \"cert\"".into(), "a/1.tf".into())));
        assert_eq!(found(symbols(), "\"cer\"", "a/main.tf"), None);
        // Other servers' symbols are ignored.
        assert_eq!(found(symbols(), "\"web\"", "a/main.tf"), None);
    }

    #[test]
    fn definition_symbol_prefers_own_directory_and_rejects_ambiguity() {
        let symbols = || {
            vec![
                symbol("tf", "module \"cert\"", "a/1.tf"),
                symbol("tf", "module \"cert\"", "b/1.tf"),
                symbol("tf", "resource \"subnet\" \"gw\"", "a/1.tf"),
                symbol("tf", "resource \"nsg\" \"gw\"", "a/1.tf"),
            ]
        };
        assert_eq!(found(symbols(), "\"cert\"", "b/main.tf"), Some(("module \"cert\"".into(), "b/1.tf".into())));
        // Two outside matches and none local, or two local ones: no guess.
        assert_eq!(found(symbols(), "\"cert\"", "c/main.tf"), None);
        assert_eq!(found(symbols(), "\"gw\"", "a/main.tf"), None);
        assert_eq!(
            found(vec![symbol("tf", "module \"cert\"", "a/1.tf")], "\"cert\"", "c/main.tf"),
            Some(("module \"cert\"".into(), "a/1.tf".into()))
        );
    }
}
