use std::ops::Add;
use std::sync::Arc;

use git::repository::RepoPath;
use sum_tree::ContextLessSummary;
use sum_tree::Item;
use sum_tree::KeyedItem;
use worktree::PathKey;
use worktree::PathSummary;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitStatus {
    Staged,
    Unstaged,
    Reverted,
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobStatus {
    Running,
    Finished,
    Skipped,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingOps {
    pub repo_path: RepoPath,
    pub ops: Vec<PendingOp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingOp {
    pub(crate) id: PendingOpId,
    pub git_status: GitStatus,
    pub(crate) job_status: JobStatus,
}

#[derive(Clone, Debug)]
pub struct PendingOpsSummary {
    staged_count: usize,
    pub staging_count: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PendingOpId(u16);

impl Item for PendingOps {
    type Summary = PathSummary<PendingOpsSummary>;

    fn summary(&self, _cx: ()) -> Self::Summary {
        PathSummary {
            max_path: Arc::clone(self.repo_path.as_ref()),
            item_summary: PendingOpsSummary {
                staged_count: self.staged() as usize,
                staging_count: self.staging() as usize,
            },
        }
    }
}

impl ContextLessSummary for PendingOpsSummary {
    fn zero() -> Self {
        Self { staged_count: 0, staging_count: 0 }
    }

    fn add_summary(&mut self, summary: &Self) {
        self.staged_count += summary.staged_count;
        self.staging_count += summary.staging_count;
    }
}

impl KeyedItem for PendingOps {
    type Key = PathKey;

    fn key(&self) -> Self::Key {
        PathKey(Arc::clone(self.repo_path.as_ref()))
    }
}

impl Add<u16> for PendingOpId {
    type Output = PendingOpId;

    fn add(self, rhs: u16) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl From<u16> for PendingOpId {
    fn from(id: u16) -> Self {
        Self(id)
    }
}

impl PendingOps {
    pub(crate) fn new(path: &RepoPath) -> Self {
        Self { repo_path: path.clone(), ops: Vec::new() }
    }

    pub(crate) fn max_id(&self) -> PendingOpId {
        self.ops.last().map_or_default(|op| op.id)
    }

    pub(crate) fn op_by_id_mut(&mut self, id: PendingOpId) -> Option<&mut PendingOp> {
        self.ops.iter_mut().find(|op| op.id == id)
    }

    /// File is staged if the last job is finished and has status Staged.
    pub fn staged(&self) -> bool {
        if let Some(last) = self.ops.last()
            && last.git_status == GitStatus::Staged
            && last.job_status == JobStatus::Finished
        {
            return true;
        }
        false
    }

    /// File is staged if the last job is not finished and has status Staged.
    pub fn staging(&self) -> bool {
        if let Some(last) = self.ops.last()
            && last.git_status == GitStatus::Staged
            && last.job_status != JobStatus::Finished
        {
            return true;
        }
        false
    }

    /// Checks whether the last operation in the pending operations resulted in
    /// an error.
    pub fn last_op_errored(&self) -> bool {
        self.ops.last().is_some_and(|op| op.error())
    }
}

impl PendingOp {
    pub(crate) fn running(self) -> bool {
        self.job_status == JobStatus::Running
    }

    pub const fn finished(&self) -> bool {
        matches!(self.job_status, JobStatus::Finished | JobStatus::Skipped)
    }

    fn error(self) -> bool {
        self.job_status == JobStatus::Error
    }
}
