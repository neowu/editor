use std::result;

use serde::de;
mod binary;
pub mod blame;
pub mod commit;
mod remote;
mod repo_path;
pub mod repository;
pub mod stash;
pub mod status;

use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Result;
use gpui::Action;
use gpui::actions;
pub use repo_path::RepoPath;
pub use repository::RemoteCommandOutput;
use serde::Deserialize;
use serde::Serialize;

pub use crate::remote::RemoteUrl;

pub const DOT_GIT: &str = ".git";
pub const GITIGNORE: &str = ".gitignore";
pub const FSMONITOR_DAEMON: &str = "fsmonitor--daemon";
pub const LFS_DIR: &str = "lfs";
pub const OBJECTS_DIR: &str = "objects";
pub const REFS_DIR: &str = "refs";
pub const REFTABLE_DIR: &str = "reftable";
pub const HOOKS_DIR: &str = "hooks";
pub const LOGS_DIR: &str = "logs";
pub const LOGS_REF_STASH: &str = "logs/refs/stash";
pub const REBASE_MERGE_DIR: &str = "rebase-merge";
pub const REBASE_APPLY_DIR: &str = "rebase-apply";
pub const SEQUENCER_DIR: &str = "sequencer";
pub const COMMIT_MESSAGE: &str = "COMMIT_EDITMSG";
pub const FETCH_HEAD: &str = "FETCH_HEAD";
pub const ORIG_HEAD: &str = "ORIG_HEAD";
pub const BISECT_LOG: &str = "BISECT_LOG";
pub const GC_PID: &str = "gc.pid";
pub const INFO_DIR: &str = "info";
pub const REPO_EXCLUDE: &str = "info/exclude";

actions!(
    git,
    [
        /// Checks out a different git branch.
        pub CheckoutBranch,
        /// Switches to a different git branch.
        pub Switch,
        /// Selects a different repository.
        pub SelectRepo,
        /// Opens the git branch selector.
        pub Branch,
        /// Shows uncommitted changes across the project.
        pub ViewUncommittedChanges,
        /// Shows unstaged changes across the project.
        pub ViewUnstagedChanges,
        /// Shows staged changes across the project.
        pub ViewStagedChanges,
        /// Opens the git stash selector.
        pub ViewStash,
        /// Opens the git worktree selector.
        pub Worktree,
        // per-hunk
        /// Toggles the staged state of the hunk or status entry at cursor.
        pub ToggleStaged,
        /// Stage status entries between an anchor entry and the cursor.
        pub StageRange,
        /// Stages the current hunk and moves to the next one.
        pub StageAndNext,
        /// Unstages the current hunk and moves to the next one.
        pub UnstageAndNext,
        /// Restores the selected hunks to their original state.
        pub Restore,
        /// Restores the selected hunks to their original state and moves to the
        /// next one.
        pub RestoreAndNext,
        // per-file
        /// Shows git blame information for the current file.
        pub Blame,
        /// Shows the git history for the selected file, folder, or project.
        pub FileHistory,
        /// Opens the selected file in the editor without a diff view.
        pub ViewFile,
        /// Stages the current file.
        pub StageFile,
        /// Unstages the current file.
        pub UnstageFile,
        // per-section
        /// Stages every entry in the section containing the selected entry.
        pub StageSection,
        /// Unstages every entry in the section containing the selected entry.
        pub UnstageSection,
        // repo-wide
        /// Stages all changes in the repository.
        pub StageAll,
        /// Unstages all changes in the repository.
        pub UnstageAll,
        /// Stashes all changes in the repository, including untracked files.
        pub StashAll,
        /// Stashes tracked changes in the repository, leaving untracked files in place.
        pub StashTracked,
        /// Stashes staged changes in the repository, leaving unstaged changes in place.
        pub StashStaged,
        /// Pops the most recent stash.
        pub StashPop,
        /// Apply the most recent stash.
        pub StashApply,
        /// Restores all tracked files to their last committed state.
        pub RestoreTrackedFiles,
        /// Deletes all untracked files (moves them to the system trash).
        pub DeleteUntrackedFiles,
        /// Undoes the last commit, keeping changes in the working directory.
        pub Uncommit,
        /// Pushes commits to the remote repository.
        pub Push,
        /// Pushes commits to a specific remote branch.
        pub PushTo,
        /// Force pushes commits to the remote repository.
        pub ForcePush,
        /// Pulls with rebase, carrying uncommitted changes across via autostash.
        pub Pull,
        /// Continues a rebase that stopped on conflicts.
        pub RebaseContinue,
        /// Aborts a rebase that stopped on conflicts, restoring the state before it.
        pub RebaseAbort,
        /// Fetches changes from the remote repository.
        pub Fetch,
        /// Fetches changes from a specific remote.
        pub FetchFrom,
        /// Creates a new commit with staged changes.
        pub Commit,
        /// Runs the next commit with `git commit --no-verify`.
        pub SkipHooks,
        /// Amends the last commit with staged changes.
        pub Amend,
        /// Enable the --signoff option.
        pub Signoff,
        /// Cancels the current git operation.
        pub Cancel,
        /// Toggles whether the commit message editor fills all the available
        /// vertical space within the git panel.
        pub ToggleFillCommitEditor,
        /// Toggles whether the commit message editor is shown in the git panel.
        pub ToggleCommitEditor,
        /// Initializes a new git repository.
        pub Init,
        /// Opens all modified files in the editor.
        pub OpenModifiedFiles,
        /// Opens the current file in a solo diff view.
        pub OpenFileDiff,
        /// Clones a repository.
        pub Clone,
        pub ViewCommit,
        /// Adds a file to .gitignore.
        pub AddToGitignore,
        /// Adds a file to the repository's .git/info/exclude.
        pub AddToGitInfoExclude,
        /// Copies the current branch name to the clipboard.
        pub CopyBranchName,
        /// Creates a tag at HEAD.
        pub CreateTagAtHead,
    ]
);

/// Describes which ref to base a new git worktree on. The worktree is
/// always created in a detached HEAD state; users can opt into creating
/// a branch afterwards from the worktree itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NewWorktreeBranchTarget {
    /// Create a detached worktree from the current HEAD.
    #[default]
    CurrentBranch,
    /// Create a detached worktree at the tip of an existing branch.
    ExistingBranch { name: String },
    /// Create a detached worktree at the tip of a remote-tracking branch.
    RemoteBranch { remote_name: String, branch_name: String },
}

/// Creates a new git worktree and switches the workspace to it.
/// Dispatched by the unified worktree picker when the user selects a "Create new worktree" entry.
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = git, no_build)]
pub struct CreateWorktree {
    /// When this is None, ed will randomly generate a worktree name.
    pub worktree_name: Option<String>,
    pub branch_target: NewWorktreeBranchTarget,
}

/// Switches the workspace to an existing linked worktree.
/// Dispatched by the unified worktree picker when the user selects an existing worktree.
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = git, no_build)]
pub struct SwitchWorktree {
    pub path: PathBuf,
    pub display_name: String,
}

/// Opens an existing worktree in a new window.
/// Dispatched by the worktree picker's "Open in New Window" button.
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = git, no_build)]
pub struct OpenWorktreeInNewWindow {
    pub path: PathBuf,
}

/// Renames a git branch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
#[action(namespace = git)]
pub struct RenameBranch {
    /// The branch to rename.
    ///
    /// Default: the current branch.
    branch: Option<String>,
}

/// Restores a file to its last committed state, discarding local changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Action)]
#[action(namespace = git)]
pub struct RestoreFile {
    pub skip_prompt: bool,
}

/// The length of a Git short SHA.
pub const SHORT_SHA_LENGTH: usize = 7;

const SHA1_BYTE_LENGTH: usize = 20;
const SHA256_BYTE_LENGTH: usize = 32;
const SHA1_HEX_LENGTH: usize = SHA1_BYTE_LENGTH * 2;
const SHA256_HEX_LENGTH: usize = SHA256_BYTE_LENGTH * 2;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Oid {
    bytes: [u8; SHA256_BYTE_LENGTH],
    format: OidFormat,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum OidFormat {
    Sha1,
    Sha256,
}

impl OidFormat {
    const fn byte_len(self) -> usize {
        match self {
            Self::Sha1 => SHA1_BYTE_LENGTH,
            Self::Sha256 => SHA256_BYTE_LENGTH,
        }
    }

    const fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => SHA1_HEX_LENGTH,
            Self::Sha256 => SHA256_HEX_LENGTH,
        }
    }
}

impl Oid {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let format = match bytes.len() {
            SHA1_BYTE_LENGTH => OidFormat::Sha1,
            SHA256_BYTE_LENGTH => OidFormat::Sha256,
            len => {
                anyhow::bail!(
                    "invalid git oid byte length: expected {SHA1_BYTE_LENGTH} for SHA-1 or {SHA256_BYTE_LENGTH} for SHA-256, got {len}"
                );
            }
        };

        let mut oid_bytes = [0_u8; SHA256_BYTE_LENGTH];
        oid_bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(Self { bytes: oid_bytes, format })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.format.byte_len()]
    }

    fn is_zero(&self) -> bool {
        self.as_bytes().iter().all(|byte| *byte == 0)
    }

    /// Returns this [`Oid`] as a short SHA.
    pub fn display_short(&self) -> String {
        self.hex_string(SHORT_SHA_LENGTH)
    }

    fn hex_string(&self, len: usize) -> String {
        let mut string = String::with_capacity(len);
        for index in 0..len {
            string.push(self.hex_digit(index));
        }
        string
    }

    fn write_hex(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for index in 0..self.format.hex_len() {
            f.write_char(self.hex_digit(index))?;
        }
        Ok(())
    }

    #[inline(always)]
    fn hex_digit(&self, index: usize) -> char {
        let byte = self.as_bytes()[index / 2];
        let nibble = if index & 1 == 0 { byte >> 4 } else { byte & 0x0f };
        char::from(HEX_DIGITS[nibble as usize])
    }
}

impl TryFrom<&str> for Oid {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> result::Result<Self, Self::Error> {
        Oid::from_str(value)
    }
}

impl FromStr for Oid {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> result::Result<Self, Self::Err> {
        let format = match s.len() {
            1..=SHA1_HEX_LENGTH => OidFormat::Sha1,
            SHA256_HEX_LENGTH => OidFormat::Sha256,
            len => {
                anyhow::bail!(
                    "invalid git oid hex length: expected 1..={SHA1_HEX_LENGTH} for SHA-1 or {SHA256_HEX_LENGTH} for SHA-256, got {len}"
                );
            }
        };

        let mut bytes = [0_u8; SHA256_BYTE_LENGTH];
        for (index, byte) in s.bytes().enumerate() {
            let digit = decode_hex_digit(byte)
                .ok_or_else(|| anyhow::anyhow!("invalid hex digit at byte {index} for git oid"))?;
            if index % 2 == 0 {
                bytes[index / 2] = digit << 4;
            } else {
                bytes[index / 2] |= digit;
            }
        }

        Ok(Self { bytes, format })
    }
}

const fn decode_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_hex(f)
    }
}

impl Serialize for Oid {
    fn serialize<S>(&self, serializer: S) -> result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.hex_string(self.format.hex_len()))
    }
}

impl<'de> Deserialize<'de> for Oid {
    fn deserialize<D>(deserializer: D) -> result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<Oid>().map_err(de::Error::custom)
    }
}

impl From<Oid> for u32 {
    fn from(oid: Oid) -> Self {
        let mut u32_bytes = [0_u8; 4];
        u32_bytes.copy_from_slice(&oid.as_bytes()[..4]);
        u32::from_le_bytes(u32_bytes)
    }
}

impl From<Oid> for usize {
    fn from(oid: Oid) -> Self {
        let mut u64_bytes = [0_u8; 8];
        u64_bytes.copy_from_slice(&oid.as_bytes()[..8]);
        u64::from_le_bytes(u64_bytes) as usize
    }
}
