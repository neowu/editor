use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_macos::command;
use fs::Fs;
use futures::future::Shared;
use gpui::EntityId;
use gpui::Task;
use lsp::LanguageServerBinary;
use lsp::LanguageServerName;
use rustc_hash::FxHashMap;

use crate::BinaryStatus;
use crate::LanguageRegistry;
use crate::LspAdapter;

/// Lets [`LspAdapter`] implementations interface with the worktree a server runs for.
pub struct LspAdapterDelegate {
    worktree_root: Arc<Path>,
    /// Entity binary statuses are reported under.
    status_source_id: EntityId,
    pub fs: Arc<Fs>,
    language_registry: Arc<LanguageRegistry>,
    load_shell_env_task: Shared<Task<Option<FxHashMap<String, String>>>>,
}

impl LspAdapterDelegate {
    pub fn new(
        worktree_root: Arc<Path>,
        status_source_id: EntityId,
        fs: Arc<Fs>,
        language_registry: Arc<LanguageRegistry>,
        load_shell_env_task: Shared<Task<Option<FxHashMap<String, String>>>>,
    ) -> Arc<Self> {
        Arc::new(Self { worktree_root, status_source_id, fs, language_registry, load_shell_env_task })
    }

    pub fn worktree_root_path(&self) -> &Path {
        &self.worktree_root
    }

    pub fn update_status(&self, server_name: LanguageServerName, status: BinaryStatus) {
        self.language_registry.update_lsp_binary_status_for_entity(self.status_source_id, server_name, status);
    }

    pub fn registered_lsp_adapters(&self) -> Vec<Arc<dyn LspAdapter>> {
        let mut adapters = self
            .language_registry
            .all_lsp_adapters()
            .into_iter()
            .map(|adapter| Arc::clone(&adapter.adapter) as Arc<dyn LspAdapter>)
            .collect::<Vec<_>>();
        adapters.sort_by_key(|adapter| adapter.name());
        adapters
    }

    pub async fn shell_env(&self) -> FxHashMap<String, String> {
        self.load_shell_env_task.clone().await.unwrap_or_default()
    }

    /// Directory to resolve commands from: the worktree root, or its parent for a single-file worktree.
    async fn working_dir(&self) -> PathBuf {
        let mut dir = self.worktree_root.to_path_buf();
        if self.fs.is_file(&dir).await {
            dir.pop();
        }
        dir
    }

    pub async fn which(&self, command: &OsStr) -> Option<PathBuf> {
        let dir = self.working_dir().await;
        let env = self.shell_env().await;
        which::which_in(command, env.get("PATH"), dir).ok()
    }

    pub async fn try_exec(&self, command: LanguageServerBinary) -> Result<()> {
        let output = command::new_command(&command.path)
            .args(command.arguments)
            .envs(command.env.unwrap_or_default())
            .current_dir(self.working_dir().await)
            .output()
            .await?;

        anyhow::ensure!(
            output.status.success(),
            "{}, stdout: {:?}, stderr: {:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::thread;
    use std::time::Duration;

    use futures::FutureExt as _;
    use gpui::BackgroundExecutor;
    use gpui::PlatformDispatcher;
    use gpui::Priority;
    use gpui::RunnableVariant;
    use gpui::block_on;

    use super::*;

    /// Runs every runnable on a fresh thread; enough for fs and process work.
    struct ThreadDispatcher;

    impl PlatformDispatcher for ThreadDispatcher {
        fn is_main_thread(&self) -> bool {
            false
        }

        fn dispatch(&self, runnable: RunnableVariant, _: Priority) {
            thread::spawn(move || runnable.run());
        }

        fn dispatch_on_main_thread(&self, runnable: RunnableVariant) {
            thread::spawn(move || runnable.run());
        }

        fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
            thread::spawn(move || {
                thread::sleep(duration);
                runnable.run();
            });
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("lsp_adapter_delegate_{name}_{}", std::process::id()));
            _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            _ = fs::remove_dir_all(&self.0);
        }
    }

    fn delegate(root: &Path, env: Option<FxHashMap<String, String>>) -> Arc<LspAdapterDelegate> {
        let executor = BackgroundExecutor::new(Arc::new(ThreadDispatcher));
        LspAdapterDelegate::new(
            root.into(),
            EntityId::from(1),
            Fs::new(executor.clone()),
            Arc::new(LanguageRegistry::new(executor)),
            Task::ready(env).shared(),
        )
    }

    #[test]
    fn worktree_root_path_is_the_given_root() {
        let delegate = delegate(Path::new("/some/root"), None);
        assert_eq!(delegate.worktree_root_path(), Path::new("/some/root"));
    }

    #[test]
    fn shell_env_defaults_to_empty_when_unavailable() {
        let delegate = delegate(Path::new("/"), None);
        assert!(block_on(delegate.shell_env()).is_empty());

        let env = FxHashMap::from_iter([("A".to_owned(), "1".to_owned())]);
        let delegate = self::delegate(Path::new("/"), Some(env.clone()));
        assert_eq!(block_on(delegate.shell_env()), env);
    }

    #[test]
    fn working_dir_is_parent_of_single_file_worktree() {
        let dir = TempDir::new("working_dir");
        let file = dir.0.join("main.rs");
        fs::write(&file, "").unwrap();

        let delegate = delegate(&dir.0, None);
        assert_eq!(block_on(delegate.working_dir()), dir.0);
        let delegate = self::delegate(&file, None);
        assert_eq!(block_on(delegate.working_dir()), dir.0);
    }

    #[test]
    fn which_searches_shell_env_path() {
        let dir = TempDir::new("which");
        let bin = dir.0.join("fake-server");
        fs::write(&bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

        let env = FxHashMap::from_iter([("PATH".to_owned(), dir.0.display().to_string())]);
        let delegate = delegate(&dir.0, Some(env));
        assert_eq!(block_on(delegate.which("fake-server".as_ref())), Some(bin));
        assert_eq!(block_on(delegate.which("missing-server".as_ref())), None);
    }

    #[test]
    fn try_exec_reports_exit_status() {
        let delegate = delegate(Path::new("/"), None);
        let binary = |path: &str| LanguageServerBinary { path: path.into(), arguments: Vec::new(), env: None };
        block_on(delegate.try_exec(binary("/usr/bin/true"))).unwrap();
        block_on(delegate.try_exec(binary("/usr/bin/false"))).unwrap_err();
    }

    #[test]
    fn registered_lsp_adapters_is_empty_for_empty_registry() {
        let delegate = delegate(Path::new("/"), None);
        assert!(delegate.registered_lsp_adapters().is_empty());
    }
}
