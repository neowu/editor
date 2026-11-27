use std::borrow::Borrow;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;

/// A file system path that is guaranteed to be relative and normalized.
///
/// Paths are stored in POSIX ('/'-delimited) format.
///
/// Relative paths are also guaranteed to be valid unicode.
#[repr(transparent)]
#[derive(PartialEq, Eq, Hash)]
pub struct RelPath(str);

/// An owned representation of a file system path that is guaranteed to be
/// relative and normalized.
///
/// This type is to [`RelPath`] as [`std::path::PathBuf`] is to [`std::path::Path`].
#[derive(PartialEq, Eq, Clone, Hash)]
pub struct RelPathBuf(String);

impl RelPath {
    /// Creates an empty [`RelPath`].
    pub const fn empty() -> &'static Self {
        Self::from_str("")
    }

    /// Creates an empty [`RelPath`].
    pub fn empty_arc() -> Arc<Self> {
        static EMPTY: OnceLock<Arc<RelPath>> = OnceLock::new();
        Arc::clone(EMPTY.get_or_init(|| Arc::from(Self::empty())))
    }

    /// Converts a path into a [`RelPath`].
    ///
    /// Returns an error if the path is absolute, or is not valid unicode.
    ///
    /// This method will normalize the path by removing `.` components,
    /// processing `..` components, and removing trailing separators. It does
    /// not allocate unless it's necessary to reformat the path.
    #[track_caller]
    pub fn new(path: &Path) -> Result<Cow<'_, Self>> {
        let mut path = path.to_str().context("non utf-8 path")?;

        while let Some(rest) = path.strip_prefix("./") {
            path = rest;
        }
        while let Some(prefix) = path.strip_suffix('/')
            && !prefix.is_empty()
        {
            path = prefix;
        }

        if path.starts_with('/') {
            return Err(anyhow!("absolute path not allowed: {path:?}"));
        }

        let mut result = Cow::Borrowed(Self::from_str(path));

        if result.components().any(|component| component.is_empty() || component == "." || component == "..") {
            let mut normalized = RelPathBuf::new();
            for component in result.components() {
                match component {
                    "" => {}
                    "." => {}
                    ".." => {
                        if !normalized.pop() {
                            return Err(anyhow!("path is not relative: {result:?}"));
                        }
                    }
                    other => normalized.push(RelPath::from_str(other)),
                }
            }
            result = Cow::Owned(normalized);
        }

        Ok(result)
    }

    #[track_caller]
    pub fn new_test(path: &str) -> Cow<'_, Self> {
        Self::new(Path::new(path)).expect("invalid test path")
    }

    /// Converts a path that is already normalized and uses '/' separators
    /// into a [`RelPath`] .
    ///
    /// Returns an error if the path is not already in the correct format.
    #[track_caller]
    pub fn from_unix_str<S: AsRef<Path> + ?Sized>(path: &S) -> Result<&Self> {
        let path = path.as_ref();
        match Self::new(path)? {
            Cow::Borrowed(path) => Ok(path),
            Cow::Owned(_) => Err(anyhow!("invalid relative path {path:?}")),
        }
    }

    const fn from_str(s: &str) -> &Self {
        // Safety: `RelPath` is a transparent wrapper around `str`.
        unsafe { &*(std::ptr::from_ref::<str>(s) as *const Self) }
    }

    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub const fn components(&self) -> RelPathComponents<'_> {
        RelPathComponents(&self.0)
    }

    pub const fn ancestors(&self) -> RelPathAncestors<'_> {
        RelPathAncestors { full: &self.0, front: self.0.len(), back: 0, done: false }
    }

    pub fn file_name(&self) -> Option<&str> {
        self.components().next_back()
    }

    pub fn file_stem(&self) -> Option<&str> {
        Some(self.as_std_path().file_stem()?.to_str().expect("rel paths are utf-8"))
    }

    pub fn extension(&self) -> Option<&str> {
        Some(self.as_std_path().extension()?.to_str().expect("rel paths are utf-8"))
    }

    pub fn parent(&self) -> Option<&Self> {
        let mut components = self.components();
        components.next_back()?;
        Some(components.rest())
    }

    pub fn starts_with(&self, other: &Self) -> bool {
        self.strip_prefix(other).is_ok()
    }

    /// Returns true if this path is a strict descendant of `ancestor`.
    ///
    /// Unlike `starts_with`, this returns false when `self == ancestor`
    /// and false when `ancestor` is empty (since every path trivially
    /// starts with the empty prefix).
    pub fn is_descendant_of(&self, ancestor: &Self) -> bool {
        if ancestor.is_empty() || self == ancestor {
            return false;
        }
        self.starts_with(ancestor)
    }

    pub fn ends_with(&self, other: &Self) -> bool {
        if other.is_empty() {
            return true;
        }
        if let Some(suffix) = self.0.strip_suffix(&other.0)
            && (suffix.ends_with('/') || suffix.is_empty())
        {
            return true;
        }
        false
    }

    pub fn strip_prefix<'a>(&'a self, other: &Self) -> Result<&'a Self, StripPrefixError> {
        if other.is_empty() {
            return Ok(self);
        }
        if let Some(suffix) = self.0.strip_prefix(&other.0) {
            if let Some(suffix) = suffix.strip_prefix('/') {
                return Ok(Self::from_str(suffix));
            } else if suffix.is_empty() {
                return Ok(Self::empty());
            }
        }
        Err(StripPrefixError)
    }

    pub fn len(&self) -> usize {
        if self.0.is_empty() { 0 } else { self.0.matches('/').count() + 1 }
    }

    pub fn last_n_components(&self, count: usize) -> Option<&Self> {
        let len = self.len();
        if len >= count {
            let mut components = self.components();
            for _ in 0..(len - count) {
                components.next()?;
            }
            Some(components.rest())
        } else {
            None
        }
    }

    pub fn join(&self, other: &Self) -> RelPathBuf {
        if self.0.is_empty() {
            other.to_rel_path_buf()
        } else if other.0.is_empty() {
            self.to_rel_path_buf()
        } else {
            RelPathBuf(format!("{}/{}", &self.0, &other.0))
        }
    }

    pub fn to_rel_path_buf(&self) -> RelPathBuf {
        RelPathBuf(self.0.to_string())
    }

    pub fn into_arc(&self) -> Arc<Self> {
        Arc::from(self)
    }

    /// Convert the path into a string for presenting to the user.
    pub const fn display(&self) -> &str {
        &self.0
    }

    /// Get the internal unix-style representation of the path.
    ///
    /// This should not be shown to the user.
    pub const fn as_unix_str(&self) -> &str {
        &self.0
    }

    /// Interprets the path as a [`std::path::Path`], suitable for file system calls.
    ///
    /// This should not be shown to the user.
    pub fn as_std_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[derive(Debug)]
pub struct StripPrefixError;

impl ToOwned for RelPath {
    type Owned = RelPathBuf;

    fn to_owned(&self) -> Self::Owned {
        self.to_rel_path_buf()
    }
}

impl Borrow<RelPath> for RelPathBuf {
    fn borrow(&self) -> &RelPath {
        self.as_rel_path()
    }
}

impl PartialOrd for RelPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RelPath {
    fn cmp(&self, other: &Self) -> Ordering {
        // Ranking `/` below every other byte orders paths by component.
        const CHUNK: usize = size_of::<u128>();
        let rank = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
        // Read as little-endian, the XOR's lowest set bit is in the first difference.
        let first_difference = |a: &[u8; CHUNK], b: &[u8; CHUNK]| {
            let difference = u128::from_le_bytes(*a) ^ u128::from_le_bytes(*b);
            (difference != 0).then(|| difference.trailing_zeros() as usize / 8)
        };
        let (a, b) = (self.0.as_bytes(), other.0.as_bytes());
        let len = a.len().min(b.len());
        let (a_prefix, b_prefix) = (&a[..len], &b[..len]);
        let mut difference = None;
        for (chunk_ix, (a_chunk, b_chunk)) in
            a_prefix.as_chunks::<CHUNK>().0.iter().zip(b_prefix.as_chunks::<CHUNK>().0).enumerate()
        {
            if let Some(ix) = first_difference(a_chunk, b_chunk) {
                difference = Some(chunk_ix * CHUNK + ix);
                break;
            }
        }
        if difference.is_none() && len % CHUNK != 0 {
            // A window ending at `len` overlaps only matched bytes.
            difference = match (a_prefix.last_chunk::<CHUNK>(), b_prefix.last_chunk::<CHUNK>()) {
                (Some(a_chunk), Some(b_chunk)) => first_difference(a_chunk, b_chunk).map(|ix| len - CHUNK + ix),
                _ => a_prefix.iter().zip(b_prefix).position(|(a, b)| a != b),
            };
        }
        match difference {
            Some(ix) => rank(a[ix]).cmp(&rank(b[ix])),
            None => a.len().cmp(&b.len()),
        }
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Debug for RelPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl RelPathBuf {
    pub const fn new() -> Self {
        Self(String::new())
    }

    pub fn pop(&mut self) -> bool {
        if let Some(ix) = self.0.rfind('/') {
            self.0.truncate(ix);
            true
        } else if !self.is_empty() {
            self.0.clear();
            true
        } else {
            false
        }
    }

    pub fn push(&mut self, path: &RelPath) {
        if path.is_empty() {
            return;
        }
        if !self.is_empty() {
            self.0.push('/');
        }
        self.0.push_str(&path.0);
    }

    pub fn push_component(&mut self, component: &str) -> Result<()> {
        anyhow::ensure!(
            !component.is_empty() && !component.contains('/') && component != "." && component != "..",
            "invalid relative path component: {component:?}"
        );

        if !self.is_empty() {
            self.0.push('/');
        }
        self.0.push_str(component);
        Ok(())
    }

    pub const fn as_rel_path(&self) -> &RelPath {
        RelPath::from_str(self.0.as_str())
    }
}

impl Default for RelPathBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialOrd for RelPathBuf {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RelPathBuf {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_rel_path().cmp(other.as_rel_path())
    }
}

impl From<RelPathBuf> for Arc<RelPath> {
    fn from(value: RelPathBuf) -> Self {
        Arc::from(value.as_rel_path())
    }
}

impl AsRef<RelPath> for RelPathBuf {
    fn as_ref(&self) -> &RelPath {
        self.as_rel_path()
    }
}

impl AsRef<Path> for RelPathBuf {
    fn as_ref(&self) -> &Path {
        self.as_std_path()
    }
}

impl AsRef<RelPath> for RelPath {
    fn as_ref(&self) -> &RelPath {
        self
    }
}

impl AsRef<Path> for RelPath {
    fn as_ref(&self) -> &Path {
        self.as_std_path()
    }
}

impl Deref for RelPathBuf {
    type Target = RelPath;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<'a> From<&'a RelPath> for Cow<'a, RelPath> {
    fn from(value: &'a RelPath) -> Self {
        Self::Borrowed(value)
    }
}

impl From<&RelPath> for Arc<RelPath> {
    fn from(rel_path: &RelPath) -> Self {
        let bytes: Arc<str> = Arc::from(&rel_path.0);
        // SAFETY: `RelPath` is a `repr(transparent)` wrapper around `str`.
        unsafe { Arc::from_raw(Arc::into_raw(bytes) as *const RelPath) }
    }
}

impl<'a> TryFrom<&'a str> for &'a RelPath {
    type Error = anyhow::Error;

    fn try_from(s: &'a str) -> Result<Self> {
        RelPath::from_unix_str(s)
    }
}

impl TryFrom<&str> for RelPathBuf {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> Result<Self> {
        RelPath::new(Path::new(s)).map(|x| x.into_owned())
    }
}

impl PartialEq<str> for RelPath {
    fn eq(&self, other: &str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<RelPath> for RelPathBuf {
    fn eq(&self, other: &RelPath) -> bool {
        self.as_rel_path() == other
    }
}

impl PartialEq<RelPathBuf> for RelPath {
    fn eq(&self, other: &RelPathBuf) -> bool {
        other.as_rel_path() == self
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for RelPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Default)]
pub struct RelPathComponents<'a>(&'a str);

pub struct RelPathAncestors<'a> {
    full: &'a str,
    front: usize,
    back: usize,
    done: bool,
}

const SEPARATOR: char = '/';

impl<'a> RelPathComponents<'a> {
    pub const fn rest(&self) -> &'a RelPath {
        RelPath::from_str(self.0)
    }
}

impl<'a> Iterator for RelPathComponents<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(sep_ix) = self.0.find(SEPARATOR) {
            let (head, tail) = self.0.split_at(sep_ix);
            self.0 = &tail[1..];
            Some(head)
        } else if self.0.is_empty() {
            None
        } else {
            let result = self.0;
            self.0 = "";
            Some(result)
        }
    }
}

impl<'a> Iterator for RelPathAncestors<'a> {
    type Item = &'a RelPath;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let result = &self.full[..self.front];
        if self.front == self.back {
            self.done = true;
        } else {
            self.front = result.rfind(SEPARATOR).unwrap_or_default();
        }
        Some(RelPath::from_str(result))
    }
}

impl DoubleEndedIterator for RelPathAncestors<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let result = &self.full[..self.back];
        if self.front == self.back {
            self.done = true;
        } else {
            let search_start = if self.back == 0 { 0 } else { self.back + 1 };
            self.back = match self.full[search_start..].find(SEPARATOR) {
                Some(sep_ix) => search_start + sep_ix,
                None => self.full.len(),
            };
        }
        Some(RelPath::from_str(result))
    }
}

impl DoubleEndedIterator for RelPathComponents<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if let Some(sep_ix) = self.0.rfind(SEPARATOR) {
            let (head, tail) = self.0.split_at(sep_ix);
            self.0 = head;
            Some(&tail[1..])
        } else if self.0.is_empty() {
            None
        } else {
            let result = self.0;
            self.0 = "";
            Some(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RelPath;

    #[test]
    fn cmp_matches_component_order() {
        let paths = [
            "",
            "a",
            "A",
            "ab",
            "a b",
            "a-b",
            "a.b",
            "a!",
            "a~",
            "a\0",
            "a\0b",
            "ä",
            "a/b",
            "a/b/c",
            "a/b.c",
            "a/bc",
            "a.b/c",
            "a-b/c",
            "a b/c",
            "ab/c",
            "a/b/c/d",
            "a/ä",
            "a/\0b",
            "\0a/b",
            "\0a/\0b/c",
            "日本/語",
            "x/a.b/c",
            "x/a/b",
            // Differ inside a full 16-byte chunk.
            "a/cdefghijklmnop",
            "A/cdefghijklmnop",
            "a.bdefghijklmnop",
        ];
        // Long prefixes make `cmp` compare 16-byte chunks.
        for prefix in ["", "abcdefghijklmn/", "abcdefghijklmno/", "abcdefghijklmnopqrstuvwxyz01234/"] {
            for a in paths {
                for b in paths {
                    let (a, b) = (format!("{prefix}{a}"), format!("{prefix}{b}"));
                    let (a, b) = (RelPath::from_str(&a), RelPath::from_str(&b));
                    assert_eq!(a.cmp(b), a.components().cmp(b.components()), "{a:?} vs {b:?}");
                }
            }
        }
    }
}
