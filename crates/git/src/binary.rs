use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitStatus;

use anyhow::Result;
use async_macos::command::Command;
use async_macos::command::new_command;
use rustc_hash::FxHashMap;
use thiserror::Error;

pub(crate) struct GitBinary {
    git_binary_path: PathBuf,
    working_directory: PathBuf,
    envs: FxHashMap<String, String>,
}

impl GitBinary {
    pub(crate) fn new(git_binary_path: PathBuf, working_directory: PathBuf) -> Self {
        Self { git_binary_path, working_directory, envs: FxHashMap::default() }
    }

    pub(crate) fn envs(mut self, envs: FxHashMap<String, String>) -> Self {
        self.envs = envs;
        self
    }

    pub(crate) async fn run<S>(&self, args: &[S]) -> Result<String>
    where
        S: AsRef<OsStr>,
    {
        let mut stdout = self.run_raw(args).await?;
        if stdout.ends_with('\n') {
            stdout.pop();
        }
        Ok(stdout)
    }

    /// Returns the result of the command without trimming the trailing newline.
    async fn run_raw<S>(&self, args: &[S]) -> Result<String>
    where
        S: AsRef<OsStr>,
    {
        let mut command = self.build_command(args);
        let output = command.output().await?;
        anyhow::ensure!(
            output.status.success(),
            GitBinaryCommandError {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                status: output.status,
            }
        );
        Ok(String::from_utf8(output.stdout)?)
    }

    pub(crate) fn build_command<S>(&self, args: &[S]) -> Command
    where
        S: AsRef<OsStr>,
    {
        let mut command = new_command(&self.git_binary_path);
        command.current_dir(&self.working_directory);
        // Disabled to stop malicious actors from running arbitrary commands via fsmonitor hooks
        command.args(["-c", "core.fsmonitor=false"]);
        // Prepended signature lines would corrupt our --format parsers.
        command.args(["-c", "log.showSignature=false"]);
        command.arg("--no-optional-locks");
        // Internal commands must be non-interactive so background tasks never block on user input.
        command.arg("--no-pager");

        command.args(args);

        command.envs(&self.envs);
        command
    }
}

#[derive(Error, Debug)]
#[error("Git command failed:\n{stdout}{stderr}\n")]
pub(crate) struct GitBinaryCommandError {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) status: ExitStatus,
}
