use std::borrow::Cow;
use std::cmp::Reverse;
use std::num::NonZeroU32;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::str;
use std::sync::Arc;
use std::sync::LazyLock;

use anyhow::Result;
use async_macos::command::Stdio;
use async_macos::command::new_command;
use async_macos::fs;
use async_trait::async_trait;
use gpui::App;
use gpui::AppContext as _;
use gpui::AsyncApp;
use gpui::SharedString;
use language::ClientCommand;
use language::CodeLabel;
use language::ContextProvider;
use language::HighlightId;
use language::Language;
use language::LanguageServerId;
use language::LanguageServerName;
use language::LibraryProvider;
use language::LibraryRoot;
use language::LspAdapter;
use language::LspAdapterDelegate;
use language::ManifestName;
use language::ManifestProvider;
use language::ManifestQuery;
use language::Rope;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SemanticTokenRules;
use language::language_settings::WordsCompletionMode;
use lsp::LanguageServerBinary;
use lsp_types::InitializeParams;
use regex::Regex;
use rustc_hash::FxHashMap;
use serde_json::json;
use smallvec::SmallVec;
use util::merge_json_value_into;
use util::rel_path::RelPath;

use crate::LanguageDef;

fn semantic_token_rules() -> SemanticTokenRules {
    serde_json::from_str(include_str!("rust/semantic_token_rules.json"))
        .expect("failed to parse rust semantic_token_rules.json")
}

struct RustLspAdapter;

const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("rust-analyzer");

impl RustLspAdapter {
    async fn rustup_rust_analyzer_for_worktree(delegate: &LspAdapterDelegate) -> Option<PathBuf> {
        if !Self::workspace_has_rust_toolchain_override(delegate).await {
            return None;
        }

        let rustup = delegate.which("rustup".as_ref()).await?;
        let env = delegate.shell_env().await;
        let worktree_root = delegate.worktree_root_path();
        let output = new_command(rustup)
            .args(["which", "rust-analyzer"])
            .envs(env.iter())
            .current_dir(worktree_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        let output = match output {
            Ok(output) if output.status.success() => output,
            Ok(output) => {
                log::debug!(
                    "failed to locate rust-analyzer through rustup in {worktree_root:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return None;
            }
            Err(err) => {
                log::debug!("failed to run `rustup which rust-analyzer` in {worktree_root:?}: {err:#}");
                return None;
            }
        };

        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        Some(path).filter(|p| !p.as_os_str().is_empty())
    }

    async fn workspace_has_rust_toolchain_override(delegate: &LspAdapterDelegate) -> bool {
        for file_name in ["rust-toolchain.toml", "rust-toolchain"] {
            if fs::metadata(delegate.worktree_root_path().join(file_name)).await.is_ok() {
                return true;
            }
        }

        false
    }
}

pub(crate) struct CargoManifestProvider;

impl ManifestProvider for CargoManifestProvider {
    fn name(&self) -> ManifestName {
        SharedString::new_static("Cargo.toml").into()
    }

    fn search(&self, ManifestQuery { path, depth, delegate }: ManifestQuery) -> Option<Arc<RelPath>> {
        let mut outermost_cargo_toml = None;
        for path in path.ancestors().take(depth) {
            let p = path.join(RelPath::from_unix_str("Cargo.toml").expect("valid relative path"));
            if delegate.exists(&p, Some(false)) {
                outermost_cargo_toml = Some(Arc::from(path));
            }
        }

        outermost_cargo_toml
    }
}

pub(crate) struct CargoLibraryProvider;

/// Recognizes library sources by their directory layout, so this works for any
/// `CARGO_HOME` / `RUSTUP_HOME` and on remote hosts:
/// - `<cargo home>/registry/src/<index>/<name>-<version>/`
/// - `<cargo home>/git/checkouts/<repo>-<hash>/<rev>/`
/// - `<toolchain>/lib/rustlib/src/rust/library/<crate>/`
impl LibraryProvider for CargoLibraryProvider {
    fn library_root(&self, abs_path: &Path) -> Option<LibraryRoot> {
        // The outermost match wins, so a crate that happens to contain a
        // matching layout still maps to its own root.
        abs_path.ancestors().filter_map(cargo_library_root_at).last()
    }
}

fn cargo_library_root_at(dir: &Path) -> Option<LibraryRoot> {
    let names: Vec<&str> =
        dir.ancestors().take(6).map(|path| path.file_name()?.to_str()).map_while(|name| name).collect();
    let name = |index: usize| names.get(index).copied();
    let library_name = match (name(0)?, name(1), name(2), name(3), name(4), name(5)) {
        (krate, Some(_index), Some("src"), Some("registry"), _, _) => krate.to_owned(),
        (rev, Some(repo), Some("checkouts"), Some("git"), _, _) => {
            let repo = repo.rsplit_once('-').map_or(repo, |(repo, _hash)| repo);
            format!("{repo}@{rev}")
        }
        (krate, Some("library"), Some("rust"), Some("src"), Some("rustlib"), Some("lib")) => krate.to_owned(),
        _ => return None,
    };
    Some(LibraryRoot { path: dir.to_path_buf(), name: library_name })
}

#[async_trait(?Send)]
impl LspAdapter for RustLspAdapter {
    fn name(&self) -> LanguageServerName {
        SERVER_NAME
    }

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<LspAdapterDelegate>,
        cx: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        let delegate = Arc::clone(delegate);
        cx.background_spawn(async move {
            let env = delegate.shell_env().await;
            if let Some(path) = Self::rustup_rust_analyzer_for_worktree(delegate.as_ref()).await {
                let result = delegate
                    .try_exec(LanguageServerBinary {
                        path: path.clone(),
                        arguments: vec!["--help".into()],
                        env: Some(env.clone()),
                    })
                    .await;
                if result.is_ok() {
                    log::debug!("found rust-analyzer in rustup toolchain override");
                    return Some(LanguageServerBinary { path, env: Some(env), arguments: vec![] });
                }
            }

            let path = delegate.which("rust-analyzer".as_ref()).await?;

            // It is surprisingly common for ~/.cargo/bin/rust-analyzer to be a symlink to
            // /usr/bin/rust-analyzer that fails when you run it; so we need to test it.
            log::debug!("found rust-analyzer in PATH. trying to run `rust-analyzer --help`");
            let result = delegate
                .try_exec(LanguageServerBinary {
                    path: path.clone(),
                    arguments: vec!["--help".into()],
                    env: Some(env.clone()),
                })
                .await;
            if let Err(err) = result {
                log::debug!("failed to run rust-analyzer after detecting it in PATH: binary: {path:?}: {err}");
                return None;
            }

            Some(LanguageServerBinary { path, env: Some(env), arguments: vec![] })
        })
        .await
    }

    fn disk_based_diagnostic_sources(&self) -> Vec<String> {
        vec!["rustc".to_owned()]
    }

    fn disk_based_diagnostics_progress_token(&self) -> Option<String> {
        Some("rust-analyzer/flycheck".into())
    }

    async fn initialization_options(
        self: Arc<Self>,
        delegate: &Arc<LspAdapterDelegate>,
        _: &mut AsyncApp,
    ) -> Result<Option<serde_json::Value>> {
        let clippy_configs = existing_clippy_configs(delegate.worktree_root_path()).await;
        Ok(Some(initialization_options(&clippy_configs)))
    }

    fn process_diagnostics(&self, params: &mut lsp_types::PublishDiagnosticsParams, _: LanguageServerId) {
        static REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?m)`([^`]+)\n`$").expect("Failed to create REGEX"));

        dedup_diagnostics(&mut params.diagnostics);
        for diagnostic in &mut params.diagnostics {
            for message in diagnostic.related_information.iter_mut().flatten().map(|info| &mut info.message).chain(
                match &mut diagnostic.message {
                    lsp_types::DiagnosticMessage::String(message) => Some(message),
                    lsp_types::DiagnosticMessage::MarkupContent(_) => None,
                },
            ) {
                if let Cow::Owned(sanitized) = REGEX.replace_all(message, "`$1`") {
                    *message = sanitized;
                }
            }
        }
    }

    fn diagnostic_message_to_markdown(&self, message: &str) -> Option<String> {
        static REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)\n *").expect("Failed to create REGEX"));
        Some(REGEX.replace_all(message, "\n\n").to_string())
    }

    async fn label_for_completion(
        &self,
        completion: &lsp_types::CompletionItem,
        language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        // rust-analyzer calls these detail left and detail right in terms of where it expects things to be rendered
        // this usually contains signatures of the thing to be completed
        let detail_right = completion
            .label_details
            .as_ref()
            .and_then(|detail| detail.description.as_ref())
            .or(completion.detail.as_ref())
            .map(|detail| detail.trim());
        // this tends to contain alias and import information
        let mut detail_left = completion.label_details.as_ref().and_then(|detail| detail.detail.as_deref());
        let mk_label = |text: String, filter_range: &dyn Fn() -> Range<usize>, runs| {
            let filter_range = completion
                .filter_text
                .as_deref()
                .and_then(|filter| text.find(filter).map(|ix| ix..ix + filter.len()))
                .or_else(|| text.find(&completion.label).map(|ix| ix..ix + completion.label.len()))
                .unwrap_or_else(filter_range);

            CodeLabel::new(text, filter_range, runs)
        };
        let mut label = match (detail_right, completion.kind) {
            (Some(signature), Some(lsp_types::CompletionItemKind::FIELD)) => {
                let name = &completion.label;
                let text = format!("{name}: {signature}");
                let prefix = "struct S { ";
                let source = Rope::from_iter([prefix, &text, " }"]);
                let runs = language.highlight_text(&source, prefix.len()..prefix.len() + text.len()).collect();
                mk_label(text, &|| 0..completion.label.len(), runs)
            }
            (
                Some(signature),
                Some(lsp_types::CompletionItemKind::CONSTANT | lsp_types::CompletionItemKind::VARIABLE),
            ) if completion.insert_text_format != Some(lsp_types::InsertTextFormat::SNIPPET) => {
                let name = &completion.label;
                let text = format!("{name}: {signature}");
                let prefix = "let ";
                let source = Rope::from_iter([prefix, &text, " = ();"]);
                let runs = language.highlight_text(&source, prefix.len()..prefix.len() + text.len()).collect();
                mk_label(text, &|| 0..completion.label.len(), runs)
            }
            (
                function_signature,
                Some(lsp_types::CompletionItemKind::FUNCTION | lsp_types::CompletionItemKind::METHOD),
            ) => {
                const FUNCTION_PREFIXES: [&str; 6] =
                    ["async fn", "async unsafe fn", "const fn", "const unsafe fn", "unsafe fn", "fn"];
                let fn_prefixed = FUNCTION_PREFIXES
                    .iter()
                    .find_map(|&prefix| function_signature?.strip_prefix(prefix).map(|suffix| (prefix, suffix)));
                let label = if let Some(label) =
                    completion.label.strip_suffix("(…)").or_else(|| completion.label.strip_suffix("()"))
                {
                    label
                } else {
                    &completion.label
                };

                static FULL_SIGNATURE_REGEX: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(r"fn (.+?)\(").expect("Failed to create REGEX"));
                if let Some((function_signature, match_)) = function_signature
                    .filter(|it| it.contains(label))
                    .and_then(|it| Some((it, FULL_SIGNATURE_REGEX.find(it)?)))
                {
                    let runs = language.highlight_text(function_signature, 0..function_signature.len()).collect();
                    mk_label(function_signature.to_owned(), &|| match_.range().start + 3..match_.range().end - 1, runs)
                } else if let Some((prefix, suffix)) = fn_prefixed {
                    let text = format!("{label}{suffix}");
                    let source = Rope::from_iter([prefix, " ", &text, " {}"]);
                    let run_start = prefix.len() + 1;
                    let runs = language.highlight_text(&source, run_start..run_start + text.len()).collect();
                    mk_label(text, &|| 0..label.len(), runs)
                } else if detail_right.is_some_and(|detail| detail.starts_with("macro_rules! ")) {
                    let text = completion.label.clone();
                    let len = text.len();
                    let runs = language.highlight_text(text.as_str(), 0..len).collect();
                    mk_label(text, &|| 0..completion.label.len(), runs)
                } else if detail_left.is_none() {
                    return None;
                } else {
                    mk_label(completion.label.clone(), &|| 0..completion.label.len(), vec![])
                }
            }
            (_, kind) => {
                let mut label;
                let mut runs = vec![];

                if completion.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET)
                    && let Some(
                        lsp_types::CompletionTextEdit::InsertAndReplace(lsp_types::InsertReplaceEdit {
                            new_text, ..
                        })
                        | lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit { new_text, .. }),
                    ) = completion.text_edit.as_ref()
                    && let Ok(mut snippet) = snippet::Snippet::parse(new_text)
                    && snippet.tabstops.len() > 1
                {
                    label = String::new();

                    // we never display the final tabstop
                    snippet.tabstops.remove(snippet.tabstops.len() - 1);

                    let mut text_pos = 0;

                    let mut all_stop_ranges =
                        snippet.tabstops.into_iter().flat_map(|stop| stop.ranges).collect::<SmallVec<[_; 8]>>();
                    all_stop_ranges.sort_unstable_by_key(|a| (a.start, Reverse(a.end)));

                    // Placeholders may nest, e.g. `$2` inside `${1:"$2"}`
                    struct OpenPlaceholder {
                        snippet_text_end: usize,
                        label_run_start: usize,
                    }
                    let mut open_placeholders = SmallVec::<[OpenPlaceholder; 4]>::new();

                    for range in &all_stop_ranges {
                        let start_pos = range.start as usize;
                        let end_pos = range.end as usize;

                        while let Some(placeholder) = open_placeholders.last() {
                            if placeholder.snippet_text_end > start_pos {
                                break;
                            }
                            label.push_str(&snippet.text[text_pos..placeholder.snippet_text_end]);
                            text_pos = placeholder.snippet_text_end;
                            runs.push((placeholder.label_run_start..label.len(), HighlightId::TABSTOP_REPLACE_ID));
                            open_placeholders.pop();
                        }

                        label.push_str(&snippet.text[text_pos..start_pos]);
                        text_pos = start_pos;

                        if start_pos == end_pos {
                            let caret_start = label.len();
                            label.push('…');
                            runs.push((caret_start..label.len(), HighlightId::TABSTOP_INSERT_ID));
                        } else {
                            open_placeholders
                                .push(OpenPlaceholder { snippet_text_end: end_pos, label_run_start: label.len() });
                        }
                    }

                    while let Some(placeholder) = open_placeholders.pop() {
                        label.push_str(&snippet.text[text_pos..placeholder.snippet_text_end]);
                        text_pos = placeholder.snippet_text_end;
                        runs.push((placeholder.label_run_start..label.len(), HighlightId::TABSTOP_REPLACE_ID));
                    }

                    label.push_str(&snippet.text[text_pos..]);
                    runs.sort_unstable_by_key(|(range, _)| (range.start, Reverse(range.end)));

                    if detail_left.is_some_and(|detail_left| detail_left == new_text) {
                        // We only include the left detail if it isn't the snippet again
                        detail_left.take();
                    }

                    runs.extend(language.highlight_text(label.as_str(), 0..label.len()));
                } else {
                    let highlight_name = kind.and_then(|kind| match kind {
                        lsp_types::CompletionItemKind::STRUCT
                        | lsp_types::CompletionItemKind::INTERFACE
                        | lsp_types::CompletionItemKind::ENUM => Some("type"),
                        lsp_types::CompletionItemKind::ENUM_MEMBER => Some("variant"),
                        lsp_types::CompletionItemKind::KEYWORD => Some("keyword"),
                        lsp_types::CompletionItemKind::VALUE | lsp_types::CompletionItemKind::CONSTANT => {
                            Some("constant")
                        }
                        _ => None,
                    });

                    label = completion.label.clone();

                    if let Some(highlight_name) = highlight_name {
                        let highlight_id = language.grammar()?.highlight_id_for_name(highlight_name)?;
                        runs.push((0..label.rfind('(').unwrap_or(completion.label.len()), highlight_id));
                    } else if detail_left.is_none() && kind != Some(lsp_types::CompletionItemKind::SNIPPET) {
                        return None;
                    }
                }

                let label_len = label.len();

                mk_label(label, &|| 0..label_len, runs)
            }
        };

        if let Some(detail_left) = detail_left {
            label.text.push(' ');
            if !detail_left.starts_with('(') {
                label.text.push('(');
            }
            label.text.push_str(detail_left);
            if !detail_left.ends_with(')') {
                label.text.push(')');
            }
        }

        Some(label)
    }

    async fn label_for_symbol(&self, symbol: &language::Symbol, language: &Arc<Language>) -> Option<CodeLabel> {
        let name = &symbol.name;
        let (prefix, suffix) = match symbol.kind {
            language::SymbolKind::Method | language::SymbolKind::Function => ("fn ", "();"),
            language::SymbolKind::Struct => ("struct ", ";"),
            language::SymbolKind::Enum => ("enum ", "{}"),
            language::SymbolKind::Interface => ("trait ", "{}"),
            language::SymbolKind::Constant => ("const ", ":()=();"),
            language::SymbolKind::Module => ("mod ", ";"),
            language::SymbolKind::Package => ("extern crate ", ";"),
            language::SymbolKind::TypeParameter => ("type ", "=();"),
            language::SymbolKind::EnumMember => {
                let prefix = "enum E {";
                return Some(CodeLabel::new(
                    name.clone(),
                    0..name.len(),
                    language
                        .highlight_text(&Rope::from_iter([prefix, name, "}"]), prefix.len()..prefix.len() + name.len())
                        .collect(),
                ));
            }
            // lint: expect(wildcard_match) only some variants matter here
            _ => return None,
        };

        let filter_range = prefix.len()..prefix.len() + name.len();
        let display_range = 0..filter_range.end;
        Some(CodeLabel::new(
            format!("{prefix}{name}"),
            filter_range,
            language.highlight_text(&Rope::from_iter([prefix, name, suffix]), display_range).collect(),
        ))
    }

    fn prepare_initialize_params(&self, mut original: InitializeParams, _: &App) -> Result<InitializeParams> {
        // `rust-analyzer.runSingle` is deliberately not advertised, so the server sends no
        // "Run" / "Run Test" code lenses: gutter runnables already cover them.
        let commands = [
            "rust-analyzer.showReferences",
            "rust-analyzer.gotoLocation",
            "rust-analyzer.triggerParameterHints",
            "rust-analyzer.rename",
        ];

        let experimental = json!({
            "commands": {
                "commands": commands,
            },
            "runnables": { "kinds": ["cargo", "shell"] },
        });

        if let Some(original_experimental) = &mut original.capabilities.experimental {
            merge_json_value_into(experimental, original_experimental);
        } else {
            original.capabilities.experimental = Some(experimental);
        }

        Ok(original)
    }

    fn client_command(&self, command_name: &str, _arguments: &[serde_json::Value]) -> Option<ClientCommand> {
        match command_name {
            "rust-analyzer.showReferences" => Some(ClientCommand::ShowLocations),
            _ => None,
        }
    }
}

struct RustContextProvider;

impl ContextProvider for RustContextProvider {
    fn lsp_task_source(&self) -> Option<LanguageServerName> {
        Some(SERVER_NAME)
    }
}

const CLIPPY_CONFIG_FILE_NAMES: [&str; 2] = ["clippy.toml", ".clippy.toml"];

/// Clippy reports its Cargo lints (e.g. `wildcard_dependencies`) at line 1 of the clippy config.
/// rust-analyzer drops diagnostics for files outside its VFS, and a virtual workspace root isn't
/// in it, so those configs must be included explicitly.
async fn existing_clippy_configs(worktree_root: &Path) -> Vec<PathBuf> {
    let mut configs = Vec::new();
    for file_name in CLIPPY_CONFIG_FILE_NAMES {
        let path = worktree_root.join(file_name);
        if fs::metadata(&path).await.is_ok_and(|metadata| metadata.is_file()) {
            configs.push(path);
        }
    }
    configs
}

/// rust-analyzer options, plus the clippy configs at `paths` as `vfs.extraIncludes`.
fn initialization_options(paths: &[PathBuf]) -> serde_json::Value {
    let mut options = json!({
        "rustfmt": { "extraArgs": ["+nightly"] },
        "check": { "command": "clippy" },
        "cargo": { "features": "all" },
    });
    if !paths.is_empty() {
        options["vfs"] = json!({ "extraIncludes": paths });
    }
    options
}

/// Removes exact duplicates, keeping the first of each. Clippy repeats spanless lints once per
/// crate and target, all at the same place.
fn dedup_diagnostics(diagnostics: &mut Vec<lsp_types::Diagnostic>) {
    let mut kept: Vec<lsp_types::Diagnostic> = Vec::with_capacity(diagnostics.len());
    let mut kept_by_range: FxHashMap<lsp_types::Range, SmallVec<[usize; 1]>> = FxHashMap::default();
    for diagnostic in diagnostics.drain(..) {
        let same_range = kept_by_range.entry(diagnostic.range).or_default();
        if same_range.iter().any(|&ix| kept[ix] == diagnostic) {
            continue;
        }
        same_range.push(kept.len());
        kept.push(diagnostic);
    }
    *diagnostics = kept;
}

/// Settings for Rust.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(4).expect("non-zero"),
        hard_tabs: false,
        soft_wrap: false,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        formatter: language::language_settings::Formatter::Auto,
        code_actions_on_format: FxHashMap::default(),
        remove_trailing_whitespace_on_save: true,
        ensure_final_newline_on_save: true,
        lsp_document_symbols: true,
        allow_rewrap: RewrapBehavior::InComments,
        use_on_type_format: true,
        auto_indent_on_paste: true,
        word_completions: WordsCompletionMode::Fallback,
    }
}

pub(crate) fn languages() -> Vec<LanguageDef> {
    vec![LanguageDef {
        grammar: Some(tree_sitter_rust::LANGUAGE.into()),
        adapters: vec![Arc::new(RustLspAdapter)],
        context: Some(Arc::new(RustContextProvider)),
        manifest_name: Some(SharedString::new_static("Cargo.toml").into()),
        semantic_token_rules: Some(semantic_token_rules()),
        ..LanguageDef::new("rust", settings())
    }]
}

#[cfg(test)]
mod tests {
    #[test]
    fn semantic_token_rules_map_to_theme_styles() {
        let rules = super::semantic_token_rules().rules;
        let first_style = |token_type: &str| {
            rules
                .iter()
                .find(|rule| rule.token_type.as_deref() == Some(token_type))
                .and_then(|rule| rule.style.first().cloned())
        };
        for (token_type, style) in [
            ("enum", "type"),
            ("enumMember", "variant"),
            ("namespace", "variable"),
            ("lifetime", "lifetime"),
            ("label", "lifetime"),
            ("formatSpecifier", "string.escape"),
        ] {
            assert_eq!(first_style(token_type).as_deref(), Some(style), "{token_type}");
        }

        // Only bindings are underlined when mutable, not `&mut self` methods or compound assignments.
        // These come first so the underline inherits the final colour, including the unsafe one.
        let underlined: Vec<_> = rules
            .iter()
            .filter(|rule| rule.underline.is_some())
            .map(|rule| (rule.token_type.as_deref().unwrap(), rule.token_modifiers.as_slice()))
            .collect();
        let mutable = ["mutable".to_owned()];
        assert_eq!(
            underlined,
            ["variable", "parameter", "selfKeyword", "static"].map(|token_type| (token_type, mutable.as_slice()))
        );
        assert!(rules[..underlined.len()].iter().all(|rule| rule.underline.is_some()));

        // The modifier-only rule applies to every token type and must stay ahead of the type mappings.
        let unsafe_rule = &rules[underlined.len()];
        assert_eq!(
            (unsafe_rule.token_type.as_deref(), unsafe_rule.token_modifiers.as_slice()),
            (None, ["unsafe".to_owned()].as_slice())
        );
        assert!(unsafe_rule.foreground_color.is_some());

        let control_flow = rules
            .iter()
            .position(|rule| rule.token_type.as_deref() == Some("operator") && rule.token_modifiers == ["controlFlow"])
            .unwrap();
        assert_eq!(rules[control_flow].style, ["keyword"]);
        assert!(rules[control_flow + 1..].iter().any(|rule| rule.token_type.as_deref() == Some("operator")));
    }

    use std::fs;

    use serde_json::json;

    use super::CargoLibraryProvider;
    use super::LanguageServerId;
    use super::LibraryProvider as _;
    use super::LibraryRoot;
    use super::LspAdapter as _;
    use super::Path;
    use super::PathBuf;
    use super::RustLspAdapter;
    use super::dedup_diagnostics;
    use super::existing_clippy_configs;
    use super::initialization_options;
    use super::str;

    fn diagnostic(line: u32, message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: lsp_types::Range::new(lsp_types::Position::new(line, 0), lsp_types::Position::new(line, 0)),
            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
            message: lsp_types::DiagnosticMessage::String(message.to_owned()),
            ..lsp_types::Diagnostic::default()
        }
    }

    #[test]
    fn dedup_diagnostics_keeps_first_of_exact_duplicates() {
        let mut diagnostics = vec![
            diagnostic(0, "wildcard dependency for `anyhow`"),
            diagnostic(0, "wildcard dependency for `smol`"),
            diagnostic(0, "wildcard dependency for `anyhow`"),
            diagnostic(1, "wildcard dependency for `anyhow`"),
            diagnostic(0, "wildcard dependency for `anyhow`"),
        ];
        dedup_diagnostics(&mut diagnostics);
        assert_eq!(
            diagnostics,
            vec![
                diagnostic(0, "wildcard dependency for `anyhow`"),
                diagnostic(0, "wildcard dependency for `smol`"),
                diagnostic(1, "wildcard dependency for `anyhow`"),
            ]
        );
    }

    #[test]
    fn dedup_diagnostics_keeps_diagnostics_differing_in_any_field() {
        let error =
            lsp_types::Diagnostic { severity: Some(lsp_types::DiagnosticSeverity::ERROR), ..diagnostic(0, "same") };
        let with_code = lsp_types::Diagnostic {
            code: Some(lsp_types::NumberOrString::String("code".to_owned())),
            ..diagnostic(0, "same")
        };
        let mut diagnostics = vec![diagnostic(0, "same"), error.clone(), with_code.clone()];
        dedup_diagnostics(&mut diagnostics);
        assert_eq!(diagnostics, vec![diagnostic(0, "same"), error, with_code]);
    }

    #[test]
    fn process_diagnostics_removes_duplicates() {
        let mut params = lsp_types::PublishDiagnosticsParams {
            uri: lsp_types::Uri::from_file_path("/p/clippy.toml").unwrap(),
            diagnostics: vec![diagnostic(0, "dup"), diagnostic(0, "dup")],
            version: None,
        };
        RustLspAdapter.process_diagnostics(&mut params, LanguageServerId(0));
        assert_eq!(params.diagnostics, vec![diagnostic(0, "dup")]);
    }

    #[test]
    fn initialization_options_adds_clippy_configs() {
        let base = json!({
            "rustfmt": { "extraArgs": ["+nightly"] },
            "check": { "command": "clippy" },
            "cargo": { "features": "all" },
        });
        assert_eq!(initialization_options(&[]), base);

        let mut expected = base;
        expected["vfs"] = json!({ "extraIncludes": ["/p/clippy.toml", "/p/.clippy.toml"] });
        assert_eq!(
            initialization_options(&[PathBuf::from("/p/clippy.toml"), PathBuf::from("/p/.clippy.toml")]),
            expected
        );
    }

    #[test]
    fn existing_clippy_configs_finds_files_at_root() {
        let dir = test_dir("clippy_finds_files");
        let root = dir.as_path();
        assert_eq!(futures::executor::block_on(existing_clippy_configs(root)), Vec::<PathBuf>::new());

        fs::write(root.join("clippy.toml"), "").unwrap();
        assert_eq!(futures::executor::block_on(existing_clippy_configs(root)), vec![root.join("clippy.toml")]);

        fs::write(root.join(".clippy.toml"), "").unwrap();
        assert_eq!(
            futures::executor::block_on(existing_clippy_configs(root)),
            vec![root.join("clippy.toml"), root.join(".clippy.toml")]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn existing_clippy_configs_ignores_directories() {
        let dir = test_dir("clippy_ignores_dirs");
        fs::create_dir_all(dir.join("clippy.toml")).unwrap();
        assert_eq!(futures::executor::block_on(existing_clippy_configs(&dir)), Vec::<PathBuf>::new());
        fs::remove_dir_all(dir).unwrap();
    }

    /// Fresh scratch dir, unique per test name; tests remove it on success.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("languages-test-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn library_root(abs_path: &str) -> Option<LibraryRoot> {
        CargoLibraryProvider.library_root(Path::new(abs_path))
    }

    #[test]
    fn detects_registry_crate() {
        assert_eq!(
            library_root(
                "/Users/me/.local/share/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/vte-0.15.0/src/ansi.rs"
            ),
            Some(LibraryRoot {
                path: PathBuf::from(
                    "/Users/me/.local/share/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/vte-0.15.0"
                ),
                name: "vte-0.15.0".into(),
            })
        );
    }

    #[test]
    fn detects_git_checkout() {
        assert_eq!(
            library_root("/home/me/.cargo/git/checkouts/zed-a1b2c3d4e5f60718/9f8e7d6/crates/gpui/src/lib.rs"),
            Some(LibraryRoot {
                path: PathBuf::from("/home/me/.cargo/git/checkouts/zed-a1b2c3d4e5f60718/9f8e7d6"),
                name: "zed@9f8e7d6".into(),
            })
        );
    }

    #[test]
    fn detects_std_sources() {
        assert_eq!(
            library_root(
                "/Users/me/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/src/rust/library/core/src/option.rs"
            ),
            Some(LibraryRoot {
                path: PathBuf::from(
                    "/Users/me/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/src/rust/library/core"
                ),
                name: "core".into(),
            })
        );
    }

    #[test]
    fn outermost_match_wins() {
        let root = library_root("/c/registry/src/idx/foo-1.0.0/fixtures/registry/src/idx/bar-2.0.0/lib.rs");
        assert_eq!(root.map(|root| root.name), Some("foo-1.0.0".into()));
    }

    #[test]
    fn ignores_other_paths() {
        assert_eq!(library_root("/Users/me/projects/editor/crates/project/src/project.rs"), None);
        assert_eq!(library_root("/c/registry/src/idx"), None);
    }
}
