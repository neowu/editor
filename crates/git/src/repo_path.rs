use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use util::rel_path::RelPath;

#[derive(Clone, Ord, Hash, PartialOrd, Eq, PartialEq)]
pub struct RepoPath(Arc<RelPath>);

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl RepoPath {
    pub fn new<S: AsRef<str> + ?Sized>(s: &S) -> Result<Self> {
        let rel_path = RelPath::from_unix_str(s.as_ref())?;
        Ok(Self::from_rel_path(rel_path))
    }

    pub fn from_rel_path(path: &RelPath) -> RepoPath {
        Self(Arc::from(path))
    }

    pub fn as_std_path(&self) -> &Path {
        if self.is_empty() { Path::new(".") } else { self.0.as_std_path() }
    }
}

impl AsRef<Arc<RelPath>> for RepoPath {
    fn as_ref(&self) -> &Arc<RelPath> {
        &self.0
    }
}

impl Deref for RepoPath {
    type Target = RelPath;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
