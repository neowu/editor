use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::FutureExt;
use futures::future::Shared;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Task;
use gpui::WeakEntity;
use rustc_hash::FxHashMap;
use task::Shell;
use task::ShellBuilder;
use task::SpawnInTerminal;
use terminal::Terminal;
use terminal::TerminalBuilder;
use terminal::TerminalMode;
use terminal::terminal_settings::TerminalSettings;
use util::get_system_shell;
use util::rel_path::RelPath;

use crate::Project;

pub struct Terminals {
    pub(crate) local_handles: Vec<WeakEntity<terminal::Terminal>>,
}

impl Project {
    pub fn active_entry_directory(&self, cx: &App) -> Option<PathBuf> {
        let entry_id = self.active_entry()?;
        let worktree = self.worktree_for_entry(entry_id, cx)?;
        let worktree = worktree.read(cx);
        let entry = worktree.entry_for_id(entry_id)?;

        let absolute_path = worktree.absolutize(entry.path.as_ref());
        if entry.is_dir() { Some(absolute_path) } else { absolute_path.parent().map(|p| p.to_path_buf()) }
    }

    pub fn active_project_directory(&self, cx: &App) -> Option<Arc<Path>> {
        self.active_entry()
            .and_then(|entry_id| self.worktree_for_entry(entry_id, cx))
            .into_iter()
            .chain(self.worktrees(cx))
            .find_map(|tree| tree.read(cx).root_dir())
    }

    pub fn first_project_directory(&self, cx: &App) -> Option<PathBuf> {
        let worktree = self.worktrees(cx).next()?;
        let worktree = worktree.read(cx);
        if worktree.root_entry()?.is_dir() { Some(worktree.abs_path().to_path_buf()) } else { None }
    }

    /// The `.venv` directory at the root of the worktree containing `path`, when
    /// `terminal.detect_venv` is on.
    fn project_venv(&self, path: Option<&Path>, settings: &TerminalSettings, cx: &App) -> Option<PathBuf> {
        if !settings.detect_venv {
            return None;
        }
        let (worktree, _) = self.find_worktree(path?, cx)?;
        let worktree = worktree.read(cx);
        let venv = RelPath::from_unix_str(".venv").ok()?;
        worktree.entry_for_path(venv).filter(|entry| entry.is_dir()).map(|_| worktree.absolutize(venv))
    }

    pub fn create_terminal_task(
        &mut self,
        spawn_task: SpawnInTerminal,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Terminal>>> {
        let original_task = spawn_task.clone();

        let path: Option<Arc<Path>> = if let Some(cwd) = &spawn_task.cwd {
            let cwd = cwd.to_string_lossy();
            let tilde_substituted = shellexpand::tilde(&cwd);
            Some(Arc::from(Path::new(tilde_substituted.as_ref())))
        } else {
            self.active_project_directory(cx)
        };

        let settings = TerminalSettings::get().clone();
        let venv = self.project_venv(path.as_deref(), &settings, cx);

        // The terminal keeps the task as requested so a rerun wraps it in the shell afresh.
        let spawn_task = wrap_task_in_shell(&spawn_task, &Shell::System);
        let terminal_mode =
            TerminalMode::task(SpawnInTerminal { command_label: spawn_task.command_label.clone(), ..original_task });

        let env_task = self.resolve_directory_environment(&get_system_shell(), path.clone(), cx);

        cx.spawn(async move |project, cx| {
            let mut env = env_task.await.unwrap_or_default();
            env.extend(settings.env);

            if let Some(venv) = &venv {
                add_venv_to_env(&mut env, venv);
            }

            let builder = project
                .update(cx, move |_, cx| {
                    env.extend(spawn_task.env);
                    let shell = if let Some(program) = spawn_task.command {
                        Shell::WithArguments { program, args: spawn_task.args, title_override: None }
                    } else {
                        Shell::System
                    };
                    anyhow::Ok(TerminalBuilder::new(
                        path.map(|path| path.to_path_buf()),
                        terminal_mode,
                        shell,
                        env,
                        settings.cursor_shape,
                        settings.alternate_scroll,
                        settings.max_scroll_history_lines,
                        settings.path_hyperlink_regexes,
                        Duration::from_millis(settings.path_hyperlink_timeout_ms),
                        cx.entity_id().as_u64(),
                        cx,
                    ))
                })??
                .await?;
            project.update(cx, move |this, cx| {
                let terminal_handle = cx.new(|cx| builder.subscribe(cx));

                this.terminals.local_handles.push(terminal_handle.downgrade());

                let id = terminal_handle.entity_id();
                cx.observe_release(&terminal_handle, move |project, _terminal, cx| {
                    let handles = &mut project.terminals.local_handles;

                    if let Some(index) = handles.iter().position(|terminal| terminal.entity_id() == id) {
                        handles.remove(index);
                        cx.notify();
                    }
                })
                .detach();

                terminal_handle
            })
        })
    }

    pub fn create_terminal_shell(
        &mut self,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Terminal>>> {
        let path: Option<Arc<Path>> = cwd.map(|p| Arc::from(&*p));

        let settings = TerminalSettings::get().clone();
        let venv = self.project_venv(path.as_deref(), &settings, cx);
        let env_task = self.resolve_directory_environment(&get_system_shell(), path.clone(), cx);

        cx.spawn(async move |project, cx| {
            let mut env = env_task.await.unwrap_or_default();
            env.extend(settings.env);

            if let Some(venv) = &venv {
                add_venv_to_env(&mut env, venv);
            }

            let builder = project
                .update(cx, move |_, cx| {
                    let (shell, env) = (settings.shell, env);
                    anyhow::Ok(TerminalBuilder::new(
                        path.map(|path| path.to_path_buf()),
                        TerminalMode::interactive(),
                        shell,
                        env,
                        settings.cursor_shape,
                        settings.alternate_scroll,
                        settings.max_scroll_history_lines,
                        settings.path_hyperlink_regexes,
                        Duration::from_millis(settings.path_hyperlink_timeout_ms),
                        cx.entity_id().as_u64(),
                        cx,
                    ))
                })??
                .await?;
            project.update(cx, move |this, cx| {
                let terminal_handle = cx.new(|cx| builder.subscribe(cx));

                this.terminals.local_handles.push(terminal_handle.downgrade());

                let id = terminal_handle.entity_id();
                cx.observe_release(&terminal_handle, move |project, _terminal, cx| {
                    let handles = &mut project.terminals.local_handles;

                    if let Some(index) = handles.iter().position(|terminal| terminal.entity_id() == id) {
                        handles.remove(index);
                        cx.notify();
                    }
                })
                .detach();

                terminal_handle
            })
        })
    }

    pub fn clone_terminal(
        &mut self,
        terminal: &Entity<Terminal>,
        cx: &mut Context<'_, Project>,
        cwd: Option<PathBuf>,
    ) -> Task<Result<Entity<Terminal>>> {
        // We cannot clone the task's terminal, as it will effectively re-spawn the task, which might not be desirable.
        // For now, create a new shell instead.
        if terminal.read(cx).task().is_some() {
            return self.create_terminal_shell(cwd, cx);
        }
        let builder = terminal.read(cx).clone_builder(cx, cwd);
        cx.spawn(async |project, cx| {
            let terminal = builder.await?;
            project.update(cx, |project, cx| {
                let terminal_handle = cx.new(|cx| terminal.subscribe(cx));

                project.terminals.local_handles.push(terminal_handle.downgrade());

                let id = terminal_handle.entity_id();
                cx.observe_release(&terminal_handle, move |project, _terminal, cx| {
                    let handles = &mut project.terminals.local_handles;

                    if let Some(index) = handles.iter().position(|terminal| terminal.entity_id() == id) {
                        handles.remove(index);
                        cx.notify();
                    }
                })
                .detach();

                terminal_handle
            })
        })
    }

    fn resolve_directory_environment(
        &self,
        shell: &str,
        path: Option<Arc<Path>>,
        cx: &mut App,
    ) -> Shared<Task<Option<FxHashMap<String, String>>>> {
        if let Some(path) = path {
            let shell = Shell::Program(shell.to_string());
            self.environment.update(cx, |project_env, cx| project_env.local_directory_environment(&shell, path, cx))
        } else {
            Task::ready(None).shared()
        }
    }
}

/// Puts the project `.venv` in front of `PATH` and sets `VIRTUAL_ENV`, so its
/// executables win in any shell without sourcing an activation script.
fn add_venv_to_env(env: &mut FxHashMap<String, String>, venv: &Path) {
    let (bin_dir, separator) = ("bin", ":");
    let bin = venv.join(bin_dir).to_string_lossy().into_owned();
    let path = match env.get("PATH") {
        Some(path) if !path.is_empty() => format!("{bin}{separator}{path}"),
        _ => bin,
    };
    env.insert("PATH".to_owned(), path);
    env.insert("VIRTUAL_ENV".to_owned(), venv.to_string_lossy().into_owned());
}

/// Wraps the task's command and args into an invocation of `shell`.
fn wrap_task_in_shell(task: &SpawnInTerminal, shell: &Shell) -> SpawnInTerminal {
    let builder = ShellBuilder::new(shell);
    let command_label = builder.command_label(task.command.as_deref().unwrap_or(""));
    let (command, args) = builder.build_no_quote(task.command.clone(), &task.args);

    SpawnInTerminal { command_label, command: Some(command), args, ..task.clone() }
}
