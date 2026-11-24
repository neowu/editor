use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gpui::App;
use gpui::AsyncApp;
use gpui::Entity;
use language::Buffer;
use language::point_to_lsp;
use lsp::AdapterServerCapabilities;
use lsp::LanguageServer;
use lsp::LanguageServerId;
use lsp_types::notification::Notification;
use lsp_types::request::Request;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use task::ShellKind;
use task::SpawnInTerminal;
use text::BufferId;
use text::PointUtf16;
use text::ToPointUtf16 as _;

use crate::LocationLink;
use crate::lsp_command::LspCommand;
use crate::lsp_command::file_path_to_lsp_url;
use crate::lsp_command::location_link_from_lsp;
use crate::lsp_command::location_links_from_lsp;
use crate::lsp_store::LanguageServerToQuery;
use crate::lsp_store::LspStore;
use crate::make_lsp_text_document_position;
use crate::make_text_document_identifier;

pub struct LspExtExpandMacro(!);

impl Request for LspExtExpandMacro {
    type Params = ExpandMacroParams;
    type Result = Option<ExpandedMacro>;
    const METHOD: &'static str = "rust-analyzer/expandMacro";
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExpandMacroParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub position: lsp_types::Position,
}

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExpandedMacro {
    pub name: String,
    pub expansion: String,
}

impl ExpandedMacro {
    pub const fn is_empty(&self) -> bool {
        self.name.is_empty() && self.expansion.is_empty()
    }
}
#[derive(Debug)]
pub struct ExpandMacro {
    pub position: PointUtf16,
    pub server_id: LanguageServerId,
}

impl LspCommand for ExpandMacro {
    type Response = ExpandedMacro;
    type LspRequest = LspExtExpandMacro;

    fn display_name(&self) -> &'static str {
        "Expand macro"
    }

    fn check_capabilities(&self, _: AdapterServerCapabilities<'_>) -> bool {
        true
    }

    fn language_server_to_query(&self) -> LanguageServerToQuery {
        LanguageServerToQuery::Other(self.server_id)
    }

    fn to_lsp(&self, path: &Path, _: &Buffer, _: &Arc<LanguageServer>, _: &App) -> Result<ExpandMacroParams> {
        Ok(ExpandMacroParams {
            text_document: make_text_document_identifier(path)?,
            position: point_to_lsp(self.position),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<ExpandedMacro>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> anyhow::Result<ExpandedMacro> {
        Ok(message.unwrap_or_default())
    }
}

pub struct LspOpenDocs(!);

impl Request for LspOpenDocs {
    type Params = OpenDocsParams;
    type Result = Option<DocsUrls>;
    const METHOD: &'static str = "experimental/externalDocs";
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocsParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub position: lsp_types::Position,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocsUrls {
    pub web: Option<String>,
    pub local: Option<String>,
}

impl DocsUrls {
    pub const fn is_empty(&self) -> bool {
        self.web.is_none() && self.local.is_none()
    }
}

#[derive(Debug)]
pub struct OpenDocs {
    pub position: PointUtf16,
    pub server_id: LanguageServerId,
}

impl LspCommand for OpenDocs {
    type Response = DocsUrls;
    type LspRequest = LspOpenDocs;

    fn display_name(&self) -> &'static str {
        "Open docs"
    }

    fn check_capabilities(&self, _: AdapterServerCapabilities<'_>) -> bool {
        true
    }

    fn language_server_to_query(&self) -> LanguageServerToQuery {
        LanguageServerToQuery::Other(self.server_id)
    }

    fn to_lsp(&self, path: &Path, _: &Buffer, _: &Arc<LanguageServer>, _: &App) -> Result<OpenDocsParams> {
        let uri = lsp_types::Uri::from_file_path(path).map_err(|()| anyhow::anyhow!("{path:?} is not a valid URI"))?;
        Ok(OpenDocsParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
            position: point_to_lsp(self.position),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<DocsUrls>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> anyhow::Result<DocsUrls> {
        Ok(message.unwrap_or_default())
    }
}

#[derive(Debug)]
pub struct GoToParentModule {
    pub position: PointUtf16,
    pub server_id: LanguageServerId,
}

pub struct LspGoToParentModule;

impl Request for LspGoToParentModule {
    type Params = lsp_types::TextDocumentPositionParams;
    type Result = Option<Vec<lsp_types::LocationLink>>;
    const METHOD: &'static str = "experimental/parentModule";
}

impl LspCommand for GoToParentModule {
    type Response = Vec<LocationLink>;
    type LspRequest = LspGoToParentModule;

    fn display_name(&self) -> &'static str {
        "Go to parent module"
    }

    fn check_capabilities(&self, _: AdapterServerCapabilities<'_>) -> bool {
        true
    }

    fn language_server_to_query(&self) -> LanguageServerToQuery {
        LanguageServerToQuery::Other(self.server_id)
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp_types::TextDocumentPositionParams> {
        make_lsp_text_document_position(path, self.position)
    }

    async fn response_from_lsp(
        self,
        links: Option<Vec<lsp_types::LocationLink>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> anyhow::Result<Vec<LocationLink>> {
        location_links_from_lsp(links.map(lsp_types::GotoDefinitionResponse::Link), lsp_store, buffer, server_id, cx)
            .await
    }
}

// https://rust-analyzer.github.io/book/contributing/lsp-extensions.html#runnables
// Taken from https://github.com/rust-lang/rust-analyzer/blob/3aaa35b49ef27e15144952aa4f7ba3eecd36fbb4/crates/rust-analyzer/src/lsp/ext.rs#L425-L489
//
// Note that in rust-analyzer, `Runnable` is defined as:
//
// ```
// #[derive(Deserialize, Serialize, Debug, Clone)]
// #[serde(rename_all = "camelCase")]
// pub struct Runnable {
//     pub label: String,
//     #[serde(skip_serializing_if = "Option::is_none")]
//     pub location: Option<lsp_types::LocationLink>,
//     pub kind: RunnableKind,
//     pub args: RunnableArgs,
// }
//
// #[derive(Deserialize, Serialize, Debug, Clone)]
// #[serde(rename_all = "camelCase")]
// #[serde(untagged)]
// pub enum RunnableArgs {
//     Cargo(CargoRunnableArgs),
//     Shell(ShellRunnableArgs),
// }
// ```
//
// i.e., RunnableArgs uses serde(untagged) and is not associated with
// RunnableKind. But rust-analyzer always syncs RunnableKind with RunnableArgs:
//
// * https://github.com/rust-lang/rust-analyzer/blob/3aaa35b49ef27e15144952aa4f7ba3eecd36fbb4/crates/rust-analyzer/src/lsp/to_proto.rs#L1608-L1633
// * https://github.com/rust-lang/rust-analyzer/blob/3aaa35b49ef27e15144952aa4f7ba3eecd36fbb4/crates/rust-analyzer/src/lsp/to_proto.rs#L1648-L1653
// * https://github.com/rust-lang/rust-analyzer/blob/3aaa35b49ef27e15144952aa4f7ba3eecd36fbb4/crates/rust-analyzer/src/handlers/request.rs#L1052-L1066
//
// And it really doesn't make any sense for it to be any other way. On top of
// that, the Shell and Cargo variants are similar enough that serde(untagged)
// deserialization has been observed to confuse one for the other. So we rely on
// RunnableKind to determine which variant to deserialize.
pub struct Runnables(!);

impl Request for Runnables {
    type Params = RunnablesParams;
    type Result = Vec<Runnable>;
    const METHOD: &'static str = "experimental/runnables";
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunnablesParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    #[serde(default)]
    pub position: Option<lsp_types::Position>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Runnable {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<lsp_types::LocationLink>,
    #[serde(flatten)]
    pub args: RunnableArgs,
}

/// The `kind` field in the JSON determines which variant is deserialized; see
/// comment on `Runnables` above for more discussion.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "kind", content = "args")]
#[serde(rename_all = "lowercase")]
pub enum RunnableArgs {
    Cargo(CargoRunnableArgs),
    Shell(ShellRunnableArgs),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CargoRunnableArgs {
    #[serde(default, skip_serializing_if = "FxHashMap::is_empty")]
    pub environment: FxHashMap<String, String>,
    pub cwd: PathBuf,
    /// Command to be executed instead of cargo.
    #[serde(default)]
    pub override_cargo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    // command, --package and --lib stuff
    #[serde(default)]
    pub cargo_args: Vec<String>,
    // stuff after --
    #[serde(default)]
    pub executable_args: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ShellRunnableArgs {
    #[serde(default, skip_serializing_if = "FxHashMap::is_empty")]
    pub environment: FxHashMap<String, String>,
    pub cwd: PathBuf,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug)]
pub struct GetLspRunnables {
    pub buffer_id: BufferId,
    pub position: Option<text::Anchor>,
    pub server_id: LanguageServerId,
}

#[derive(Debug, Default)]
pub struct LspRunnables {
    pub runnables: Vec<(LocationLink, SpawnInTerminal)>,
}

pub fn runnable_to_spawn(label: String, args: RunnableArgs) -> SpawnInTerminal {
    let (command, args, env, cwd) = match args {
        RunnableArgs::Cargo(cargo) => {
            let mut task_args = Vec::new();
            let command = match cargo.override_cargo {
                Some(override_cargo) => {
                    let mut override_parts = override_cargo.split(' ').map(str::to_owned);
                    let command = override_parts.next().unwrap_or_else(|| override_cargo.clone());
                    task_args.extend(override_parts);
                    command
                }
                None => "cargo".to_owned(),
            };
            task_args.extend(cargo.cargo_args);
            if !cargo.executable_args.is_empty() {
                let shell_kind = ShellKind::system();
                task_args.push("--".to_owned());
                task_args.extend(
                    cargo
                        .executable_args
                        .into_iter()
                        // rust-analyzer's doctest data may contain things like `X<T>::new`
                        // which cause shell issues when run as `$SHELL -i -c "cargo test ..."`.
                        // Escape extra cargo args unconditionally as those are unlikely to contain `~`.
                        .filter_map(|extra_arg| shell_kind.try_quote(&extra_arg).map(|s| s.to_string())),
                );
            }
            (command, task_args, cargo.environment, cargo.workspace_root.unwrap_or(cargo.cwd))
        }
        RunnableArgs::Shell(shell) => (shell.program, shell.args, shell.environment, shell.cwd),
    };
    SpawnInTerminal { env, ..SpawnInTerminal::new(label, command, args, Some(cwd)) }
}

impl LspCommand for GetLspRunnables {
    type Response = LspRunnables;
    type LspRequest = Runnables;

    fn display_name(&self) -> &'static str {
        "LSP Runnables"
    }

    fn check_capabilities(&self, _: AdapterServerCapabilities<'_>) -> bool {
        true
    }

    fn language_server_to_query(&self) -> LanguageServerToQuery {
        LanguageServerToQuery::Other(self.server_id)
    }

    fn to_lsp(&self, path: &Path, buffer: &Buffer, _: &Arc<LanguageServer>, _: &App) -> Result<RunnablesParams> {
        let url = file_path_to_lsp_url(path)?;
        Ok(RunnablesParams {
            text_document: lsp_types::TextDocumentIdentifier::new(url),
            position: self.position.map(|anchor| point_to_lsp(anchor.to_point_utf16(&buffer.snapshot()))),
        })
    }

    async fn response_from_lsp(
        self,
        lsp_runnables: Vec<Runnable>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<LspRunnables> {
        let mut runnables = Vec::with_capacity(lsp_runnables.len());

        for runnable in lsp_runnables {
            // Without a location there is no row for a run icon.
            let Some(location) = runnable.location else {
                continue;
            };
            let location = location_link_from_lsp(location, &lsp_store, &buffer, server_id, &mut cx).await?;
            runnables.push((location, runnable_to_spawn(runnable.label, runnable.args)));
        }

        Ok(LspRunnables { runnables })
    }
}

#[derive(Debug)]
pub struct LspExtRunFlycheck;

impl Notification for LspExtRunFlycheck {
    type Params = RunFlycheckParams;
    const METHOD: &'static str = "rust-analyzer/runFlycheck";
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunFlycheckParams {
    pub text_document: Option<lsp_types::TextDocumentIdentifier>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustc_hash::FxHashMap;

    use super::CargoRunnableArgs;
    use super::RunnableArgs;
    use super::ShellRunnableArgs;
    use super::runnable_to_spawn;

    #[test]
    fn runnable_to_spawn_builds_cargo_and_shell_tasks() {
        let cargo = RunnableArgs::Cargo(CargoRunnableArgs {
            environment: FxHashMap::from_iter([("RUST_BACKTRACE".to_owned(), "1".to_owned())]),
            cwd: PathBuf::from("/ws/crate"),
            override_cargo: Some("cross +nightly".to_owned()),
            workspace_root: Some(PathBuf::from("/ws")),
            cargo_args: vec!["test".to_owned(), "-p".to_owned(), "a".to_owned()],
            executable_args: vec!["tests::it".to_owned()],
        });
        let task = runnable_to_spawn("test it".to_owned(), cargo);
        assert_eq!(task.full_label, "test it");
        assert_eq!(task.command.as_deref(), Some("cross"));
        assert_eq!(task.args, ["+nightly", "test", "-p", "a", "--", "tests::it"]);
        assert_eq!(task.cwd, Some(PathBuf::from("/ws")));
        assert_eq!(task.env["RUST_BACKTRACE"], "1");

        let cargo = RunnableArgs::Cargo(CargoRunnableArgs {
            environment: FxHashMap::default(),
            cwd: PathBuf::from("/ws/crate"),
            override_cargo: None,
            workspace_root: None,
            cargo_args: vec!["run".to_owned()],
            executable_args: Vec::new(),
        });
        let task = runnable_to_spawn("run".to_owned(), cargo);
        assert_eq!(task.command.as_deref(), Some("cargo"));
        assert_eq!(task.args, ["run"]);
        assert_eq!(task.cwd, Some(PathBuf::from("/ws/crate")));

        let shell = RunnableArgs::Shell(ShellRunnableArgs {
            environment: FxHashMap::default(),
            cwd: PathBuf::from("/x"),
            program: "make".to_owned(),
            args: vec!["all".to_owned()],
        });
        let task = runnable_to_spawn("make".to_owned(), shell);
        assert_eq!(task.command_label, "make all");
        assert_eq!(task.cwd, Some(PathBuf::from("/x")));
    }
}
