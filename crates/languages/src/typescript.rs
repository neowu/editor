use std::num::NonZeroU32;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;

use anyhow::Context as _;
use async_lock::RwLock;
use futures::future::join_all;
use gpui::App;
use gpui::AppContext as _;
use gpui::Task;
use itertools::Itertools as _;
use language::ContextProvider;
use language::LspAdapter;
use language::RunContext;
use language::language_settings::Formatter;
use language::language_settings::LanguageSettings;
use language::language_settings::RewrapBehavior;
use language::language_settings::SemanticTokenRules;
use language::language_settings::WordsCompletionMode;
use project::Fs;
use rustc_hash::FxHashMap;
use task::SpawnInTerminal;

use crate::LanguageDef;
use crate::PackageJson;
use crate::PackageJsonData;
use crate::tsgo::TsgoLspAdapter;

pub(crate) struct TypeScriptContextProvider {
    fs: Arc<Fs>,
    last_package_json: PackageJsonContents,
}

#[derive(Clone, Debug, Default)]
struct PackageJsonContents(Arc<RwLock<FxHashMap<PathBuf, PackageJson>>>);

/// `js-test`: one task per test runner the nearest `package.json` files depend on.
pub fn js_test_tasks(cx: &RunContext, data: &PackageJsonData, runner: &str) -> Vec<SpawnInTerminal> {
    let file = cx.file.to_string_lossy().into_owned();
    let cwd = |path: &Option<Arc<Path>>| path.as_ref().and_then(|path| path.parent()).map(Path::to_path_buf);
    let exec = |args: &[&str], extra: Vec<String>| {
        ["exec", "--"].iter().chain(args).map(|arg| (*arg).to_owned()).chain(extra).collect::<Vec<_>>()
    };
    let mut tasks = Vec::new();
    if let Some(symbol) = cx.symbol.as_deref() {
        let pattern = format!("\"{}\"", replace_test_name_parameters(symbol));
        if let Some(cwd) = cwd(&data.jest_package_path) {
            let args = exec(&["jest", "--runInBand", "--testNamePattern"], vec![pattern.clone(), file.clone()]);
            tasks.push(SpawnInTerminal::new(format!("jest test {symbol}"), runner, args, Some(cwd)));
        }
        if let Some(cwd) = cwd(&data.vitest_package_path) {
            let args = exec(
                &["vitest", "run", "--no-file-parallelism", "--testNamePattern"],
                vec![pattern.clone(), file.clone()],
            );
            tasks.push(SpawnInTerminal::new(format!("vitest test {symbol}"), runner, args, Some(cwd)));
        }
        if let Some(cwd) = cwd(&data.mocha_package_path) {
            let args = exec(&["mocha", "--grep"], vec![format!("\"{symbol}\""), file.clone()]);
            tasks.push(SpawnInTerminal::new(format!("mocha test {symbol}"), runner, args, Some(cwd)));
        }
        if let Some(cwd) = cwd(&data.jasmine_package_path) {
            let args = exec(&["jasmine"], vec![format!("--filter={symbol}"), file.clone()]);
            tasks.push(SpawnInTerminal::new(format!("jasmine test {symbol}"), runner, args, Some(cwd)));
        }
        if let Some(cwd) = cwd(&data.bun_package_path) {
            let args = vec!["test".to_owned(), "--test-name-pattern".to_owned(), pattern, file.clone()];
            tasks.push(SpawnInTerminal::new(format!("bun test {symbol}"), "bun", args, Some(cwd)));
        }
    }
    if let Some(cwd) = cwd(&data.node_package_path) {
        let args = vec!["--test".to_owned(), file.clone()];
        tasks.push(SpawnInTerminal::new("node file test", "node", args, Some(cwd.clone())));
        if let Some(symbol) = cx.symbol.as_deref() {
            let args = vec!["--test".to_owned(), "--test-name-pattern".to_owned(), format!("\"{symbol}\""), file];
            tasks.push(SpawnInTerminal::new(format!("node test {symbol}"), "node", args, Some(cwd)));
        }
    }
    tasks
}

impl TypeScriptContextProvider {
    pub(crate) fn new(fs: Arc<Fs>) -> Self {
        Self { fs, last_package_json: PackageJsonContents::default() }
    }

    /// `package.json` data merged from the file's folder up to the worktree root; the nearest wins.
    fn combined_package_json_data(
        &self,
        fs: Arc<Fs>,
        file: &Path,
        worktree_root: &Path,
        cx: &App,
    ) -> Task<PackageJsonData> {
        let new_json_data = file
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(worktree_root))
            .map(|dir| self.package_json_data(dir, self.last_package_json.clone(), Arc::clone(&fs), cx))
            .collect::<Vec<_>>();

        cx.background_spawn(async move {
            let mut package_json_data = PackageJsonData::default();
            for new_data in join_all(new_json_data).await.into_iter().flatten() {
                package_json_data.merge(new_data);
            }
            package_json_data
        })
    }

    fn package_json_data(
        &self,
        directory_path: &Path,
        existing_package_json: PackageJsonContents,
        fs: Arc<Fs>,
        cx: &App,
    ) -> Task<anyhow::Result<PackageJsonData>> {
        let package_json_path = directory_path.join("package.json");
        let metadata_check_fs = Arc::clone(&fs);
        cx.background_spawn(async move {
            let metadata = metadata_check_fs
                .metadata(&package_json_path)
                .await
                .with_context(|| format!("getting metadata for {package_json_path:?}"))?
                .with_context(|| format!("missing FS metadata for {package_json_path:?}"))?;
            let mtime = metadata.mtime.timestamp_for_user();
            let existing_data = {
                let contents = existing_package_json.0.read().await;
                contents
                    .get(&package_json_path)
                    .filter(|package_json| package_json.mtime == mtime)
                    .map(|package_json| package_json.data.clone())
            };
            if let Some(existing_data) = existing_data {
                Ok(existing_data)
            } else {
                let package_json_string = fs
                    .load(&package_json_path)
                    .await
                    .with_context(|| format!("loading package.json from {package_json_path:?}"))?;
                let package_json: FxHashMap<String, serde_json::Value> = serde_json::from_str(&package_json_string)
                    .with_context(|| format!("parsing package.json from {package_json_path:?}"))?;
                let new_data = PackageJsonData::new(package_json_path.as_path().into(), package_json);
                {
                    let mut contents = existing_package_json.0.write().await;
                    contents.insert(package_json_path, PackageJson { mtime, data: new_data.clone() });
                }
                Ok(new_data)
            }
        })
    }
}

async fn detect_package_manager(
    worktree_root: PathBuf,
    fs: Arc<Fs>,
    package_json_data: Option<PackageJsonData>,
) -> &'static str {
    if let Some(package_json_data) = package_json_data
        && let Some(package_manager) = package_json_data.package_manager
    {
        return package_manager;
    }
    if fs.is_file(&worktree_root.join("pnpm-lock.yaml")).await {
        return "pnpm";
    }
    if fs.is_file(&worktree_root.join("yarn.lock")).await {
        return "yarn";
    }
    "npm"
}

impl ContextProvider for TypeScriptContextProvider {
    fn runnables(&self, cx: RunContext, app: &App) -> Task<Vec<SpawnInTerminal>> {
        if cx.tag.as_ref() != "js-test" {
            return Task::ready(Vec::new());
        }
        let Some(worktree_root) = cx.worktree_root.clone() else {
            return Task::ready(Vec::new());
        };
        let fs = Arc::clone(&self.fs);
        let data = self.combined_package_json_data(Arc::clone(&fs), &cx.file, &worktree_root, app);
        app.background_spawn(async move {
            let data = data.await;
            let runner = detect_package_manager(worktree_root, fs, Some(data.clone())).await;
            js_test_tasks(&cx, &data, runner)
        })
    }
}

fn replace_test_name_parameters(test_name: &str) -> String {
    static PATTERN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(\$([A-Za-z0-9_\.]+|[\#])|%[psdifjo#\$%])").expect("valid regex"));
    PATTERN.split(test_name).map(regex::escape).join("(.+?)")
}

/// Settings for TypeScript, TSX and JavaScript.
pub(crate) fn settings() -> LanguageSettings {
    LanguageSettings {
        tab_size: NonZeroU32::new(2).expect("non-zero"),
        hard_tabs: false,
        soft_wrap: false,
        preferred_line_length: 120,
        wrap_guides: vec![120],
        formatter: Formatter::Auto,
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

/// Language directories and their grammars; JavaScript is parsed with the TSX grammar.
pub(crate) fn grammars() -> [(&'static str, tree_sitter::Language); 3] {
    [
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        ("javascript", tree_sitter_typescript::LANGUAGE_TSX.into()),
    ]
}

/// Shared by TypeScript, TSX and JavaScript. Covers only the tokens where tsgo beats
/// tree-sitter and the theme has a distinct colour; the rest stay tree-sitter only.
fn semantic_token_rules() -> SemanticTokenRules {
    serde_json::from_str(include_str!("typescript/semantic_token_rules.json"))
        .expect("failed to parse typescript semantic_token_rules.json")
}

pub(crate) fn languages(fs: Arc<Fs>, biome: Arc<dyn LspAdapter>) -> Vec<LanguageDef> {
    let rules = semantic_token_rules();
    let tsgo: Arc<dyn LspAdapter> = Arc::new(TsgoLspAdapter::new(Arc::clone(&fs)));
    let context: Arc<dyn ContextProvider> = Arc::new(TypeScriptContextProvider::new(fs));
    grammars()
        .into_iter()
        .map(|(dir, grammar)| LanguageDef {
            grammar: Some(grammar),
            adapters: vec![Arc::clone(&tsgo), Arc::clone(&biome)],
            context: Some(Arc::clone(&context)),
            semantic_token_rules: Some(rules.clone()),
            ..LanguageDef::new(dir, settings())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;

    use language::RunContext;

    use super::js_test_tasks;
    use super::replace_test_name_parameters;
    use crate::PackageJsonData;

    #[test]
    fn replace_test_name_parameters_turns_placeholders_into_wildcards() {
        assert_eq!(replace_test_name_parameters("adds $a and %d"), "adds (.+?) and (.+?)");
        assert_eq!(replace_test_name_parameters("a.b(c)"), "a\\.b\\(c\\)");
    }

    #[test]
    fn js_test_tasks_follow_detected_runners() {
        let cx = RunContext {
            tag: "js-test".into(),
            file: PathBuf::from("/w/app/a.test.ts"),
            worktree_root: Some(PathBuf::from("/w")),
            symbol: Some("adds".to_owned()),
            ..RunContext::default()
        };
        let package_json: Arc<Path> = Path::new("/w/app/package.json").into();
        let data = PackageJsonData {
            vitest_package_path: Some(Arc::clone(&package_json)),
            node_package_path: Some(package_json),
            ..PackageJsonData::default()
        };
        let tasks = js_test_tasks(&cx, &data, "pnpm");
        let labels = tasks.iter().map(|task| task.full_label.as_str()).collect::<Vec<_>>();
        assert_eq!(labels, ["vitest test adds", "node file test", "node test adds"]);
        assert_eq!(
            tasks[0].command_label,
            "pnpm exec -- vitest run --no-file-parallelism --testNamePattern \"adds\" /w/app/a.test.ts"
        );
        assert_eq!(tasks[0].cwd, Some(PathBuf::from("/w/app")));

        // Without a symbol only the whole-file run is left.
        let tasks = js_test_tasks(&RunContext { symbol: None, ..cx.clone() }, &data, "pnpm");
        assert_eq!(tasks.len(), 1);
        assert!(js_test_tasks(&cx, &PackageJsonData::default(), "npm").is_empty());
    }

    #[test]
    fn semantic_token_rules_cover_only_what_beats_tree_sitter() {
        let rules = super::semantic_token_rules().rules;
        let style = |token_type: &str| {
            rules.iter().find(|rule| rule.token_type.as_deref() == Some(token_type)).map(|rule| rule.style.clone())
        };
        assert_eq!(style("class"), Some(vec!["type".to_owned()]));
        assert_eq!(style("enum"), Some(vec!["enum".to_owned(), "type".to_owned()]));
        assert_eq!(style("enumMember"), Some(vec!["type".to_owned()]));
        // Same colour as tree-sitter in the built-in theme, so left without rules.
        for token_type in ["variable", "parameter", "property", "function", "method", "namespace"] {
            assert_eq!(style(token_type), None, "{token_type}");
        }
        assert!(rules.iter().all(|rule| rule.token_type.is_some() && rule.token_modifiers.is_empty()));
    }
}
