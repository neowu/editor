//! Relative path types for deltadb.
//!
//! Provides [`RelPath`] and [`RelPathBuf`] — path types that are guaranteed to be
//! relative, normalized, and valid unicode. Internally stored in POSIX (`/`-delimited)
//! format.
//!
//! Only Unix paths are supported (local and remote): `/`-separated, with no
//! drive or UNC prefixes, so `std::path::Path` semantics apply everywhere.
//!
//! Adapted from Zed's `util::rel_path` module.

use std::ffi::OsStr;
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;

use crate::rel_path::RelPath;

pub mod abs_path;
pub mod rel_path;

pub fn try_from_bytes<'a, T>(bytes: &'a [u8]) -> anyhow::Result<T>
where
    T: From<&'a Path>,
{
    Ok(T::from(Path::new(OsStr::from_bytes(bytes))))
}

pub trait PathExt {
    fn to_rel_path_buf(&self) -> anyhow::Result<rel_path::RelPathBuf>;
}

impl<T: AsRef<Path> + ?Sized> PathExt for T {
    fn to_rel_path_buf(&self) -> anyhow::Result<rel_path::RelPathBuf> {
        Ok(RelPath::new(self.as_ref())?.into_owned())
    }
}

/// Joins `right` onto `left` with a `/`, without normalizing. Returns `None`
/// when `right` is absolute or either side isn't valid UTF-8.
pub fn join(left: impl AsRef<Path>, right: impl AsRef<Path>) -> Option<String> {
    let right = right.as_ref();
    if right.has_root() {
        return None;
    }
    let right = right.to_str()?;
    let left = left.as_ref().to_str()?;
    if left.is_empty() {
        Some(right.into())
    } else {
        Some(format!("{left}{}{right}", if left.ends_with('/') { "" } else { "/" }))
    }
}

/// Like [`join`], then lexically [`normalize`]s the result.
pub fn join_path(left: impl AsRef<Path>, right: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(normalize(&join_checked(left.as_ref(), right.as_ref())?)))
}

/// Like [`join`], but errors instead of returning `None`, and keeps `.`/`..`.
pub fn join_path_preserving_components(left: impl AsRef<Path>, right: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(join_checked(left.as_ref(), right.as_ref())?))
}

fn join_checked(left: &Path, right: &Path) -> anyhow::Result<String> {
    left.to_str().context("Path contains invalid UTF-8")?;
    right.to_str().context("Path contains invalid UTF-8")?;
    join(left, right).with_context(|| format!("Path must be relative: {right:?}"))
}

/// Lexically resolves `.`, `..` and repeated separators in a `/`-separated
/// string. Leading `..` is kept for relative paths and dropped for absolute ones.
pub fn normalize(path_like: &str) -> String {
    let is_absolute = path_like.starts_with('/');
    let remainder = if is_absolute { path_like.trim_start_matches('/') } else { path_like };

    let mut components = Vec::new();
    for component in remainder.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|component| *component != "..") {
                    components.pop();
                } else if !is_absolute {
                    components.push(component);
                }
            }
            component => components.push(component),
        }
    }

    let normalized = components.join("/");
    if is_absolute { format!("/{normalized}") } else { normalized }
}

/// Splits a `/`-separated string at its last separator into the directory
/// (including the trailing `/`) and the file name.
pub fn split(path_like: &str) -> (Option<&str>, &str) {
    match path_like.rfind('/') {
        Some(pos) => (Some(&path_like[..=pos]), &path_like[pos + 1..]),
        None => (None, path_like),
    }
}

/// Normalizes a path by resolving `.` and `..` components without
/// requiring the path to exist on disk (unlike `canonicalize`).
pub fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut ret = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(..) => unreachable!("no path prefixes on unix"),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join() {
        assert_eq!(join("a", "b").as_deref(), Some("a/b"));
        assert_eq!(join("a/", "b").as_deref(), Some("a/b"));
        assert_eq!(join("", "b").as_deref(), Some("b"));
        assert_eq!(join("/a", "../b").as_deref(), Some("/a/../b"));
        assert_eq!(join("a", "/b"), None);
    }

    #[test]
    fn test_join_path() {
        assert_eq!(join_path("/a/b", "../c").unwrap(), PathBuf::from("/a/c"));
        assert_eq!(join_path("a", "./b/").unwrap(), PathBuf::from("a/b"));
        join_path("a", "/b").unwrap_err();
        assert_eq!(join_path_preserving_components("/a/b", "../c").unwrap(), PathBuf::from("/a/b/../c"));
        join_path_preserving_components("a", "/b").unwrap_err();
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("//a//./b/"), "/a/b");
        assert_eq!(normalize("/../a/../../b"), "/b");
        assert_eq!(normalize("../a/./../../b"), "../../b");
        assert_eq!(normalize("a/b/.."), "a");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn test_split() {
        assert_eq!(split("a/b/c"), (Some("a/b/"), "c"));
        assert_eq!(split("/c"), (Some("/"), "c"));
        assert_eq!(split("c"), (None, "c"));
        assert_eq!(split("a/"), (Some("a/"), ""));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize_path(Path::new("a/../../b")), PathBuf::from("b"));
    }
}
