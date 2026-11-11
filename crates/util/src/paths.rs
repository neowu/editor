use std::cmp::Ordering;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::iter::Peekable;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::path::StripPrefixError;
use std::ptr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::OnceLock;

use globset::Glob;
use globset::GlobBuilder;
use globset::GlobSet;
use globset::GlobSetBuilder;
use itertools::Itertools;
use path::rel_path::RelPath;
use path::rel_path::RelPathBuf;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;

use crate::shell::ShellKind;

/// Returns the path to the user's home directory.
pub fn home_dir() -> &'static PathBuf {
    static HOME_DIR: OnceLock<PathBuf> = OnceLock::new();
    HOME_DIR.get_or_init(|| dirs::home_dir().expect("failed to determine home directory"))
}

pub trait PathExt {
    /// Compacts a given file path by replacing the user's home directory
    /// prefix with a tilde (`~`).
    ///
    /// # Returns
    ///
    /// * A `PathBuf` containing the compacted file path. If the input path
    ///   does not have the user's home directory prefix, the original path is
    ///   returned unchanged.
    fn compact(&self) -> PathBuf;

    /// Returns a file's extension or, if the file is hidden, its name without the leading dot
    fn extension_or_hidden_file_name(&self) -> Option<&str>;

    fn try_from_bytes<'a>(bytes: &'a [u8]) -> anyhow::Result<Self>
    where
        Self: From<&'a Path>,
    {
        path::try_from_bytes::<Self>(bytes)
    }

    /// Returns a file's "full" joined collection of extensions, in the case where a file does not
    /// just have a singular extension but instead has multiple (e.g File.tar.gz, Component.stories.tsx)
    ///
    /// Will provide back the extensions joined together such as tar.gz or stories.tsx
    fn multiple_extensions(&self) -> Option<String>;

    /// Try to make a shell-safe representation of the path.
    fn try_shell_safe(&self, shell_kind: ShellKind) -> anyhow::Result<String>;
}

impl<T: AsRef<Path>> PathExt for T {
    fn compact(&self) -> PathBuf {
        if false || true {
            match self.as_ref().strip_prefix(home_dir().as_path()) {
                Ok(relative_path) => {
                    let mut shortened_path = PathBuf::new();
                    shortened_path.push("~");
                    shortened_path.push(relative_path);
                    shortened_path
                }
                Err(_) => self.as_ref().to_path_buf(),
            }
        } else {
            self.as_ref().to_path_buf()
        }
    }

    fn extension_or_hidden_file_name(&self) -> Option<&str> {
        let path = self.as_ref();
        let file_name = path.file_name()?.to_str()?;
        if file_name.starts_with('.') {
            return file_name.strip_prefix('.');
        }

        path.extension().and_then(|e| e.to_str()).or_else(|| path.file_stem()?.to_str())
    }

    fn multiple_extensions(&self) -> Option<String> {
        let path = self.as_ref();
        let file_name = path.file_name()?.to_str()?;

        let parts: Vec<&str> = file_name
            .split('.')
            // Skip the part with the file name extension
            .skip(1)
            .collect();

        if parts.len() < 2 {
            return None;
        }

        Some(parts.into_iter().join("."))
    }

    fn try_shell_safe(&self, shell_kind: ShellKind) -> anyhow::Result<String> {
        use anyhow::Context;
        let path_str = self.as_ref().to_str().with_context(|| "Path contains invalid UTF-8")?;
        shell_kind.try_quote(path_str).as_deref().map(ToOwned::to_owned).context("Failed to quote path")
    }
}

/// In memory, this is identical to `Path`. On non-Windows conversions to this type are no-ops. On
/// windows, these conversions sanitize UNC paths by removing the `\\\\?\\` prefix.
#[derive(Eq, PartialEq, Hash, Ord, PartialOrd)]
#[repr(transparent)]
pub struct SanitizedPath(Path);

impl SanitizedPath {
    pub fn new<T: AsRef<Path> + ?Sized>(path: &T) -> &Self {
        Self::unchecked_new(path.as_ref())
    }

    pub fn unchecked_new<T: AsRef<Path> + ?Sized>(path: &T) -> &Self {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<&Path, &Self>(path.as_ref()) }
    }

    pub fn from_arc(path: Arc<Path>) -> Arc<Self> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl

        unsafe { mem::transmute::<Arc<Path>, Arc<Self>>(path) }
    }

    pub fn new_arc<T: AsRef<Path> + ?Sized>(path: &T) -> Arc<Self> {
        Self::new(path).into()
    }

    pub fn cast_arc(path: Arc<Self>) -> Arc<Path> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<Arc<Self>, Arc<Path>>(path) }
    }

    pub fn cast_arc_ref(path: &Arc<Self>) -> &Arc<Path> {
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute::<&Arc<Self>, &Arc<Path>>(path) }
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.0.starts_with(&prefix.0)
    }

    pub const fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn file_name(&self) -> Option<&OsStr> {
        self.0.file_name()
    }

    pub fn extension(&self) -> Option<&OsStr> {
        self.0.extension()
    }

    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.0.join(path)
    }

    pub fn parent(&self) -> Option<&Self> {
        self.0.parent().map(Self::unchecked_new)
    }

    pub fn strip_prefix(&self, base: &Self) -> Result<&Path, StripPrefixError> {
        self.0.strip_prefix(base.as_path())
    }

    pub fn to_str(&self) -> Option<&str> {
        self.0.to_str()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.to_path_buf()
    }
}

impl Debug for SanitizedPath {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, formatter)
    }
}

impl Display for SanitizedPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl From<&SanitizedPath> for Arc<SanitizedPath> {
    fn from(sanitized_path: &SanitizedPath) -> Self {
        let path: Arc<Path> = sanitized_path.0.into();
        // safe because `Path` and `SanitizedPath` have the same repr and Drop impl
        unsafe { mem::transmute(path) }
    }
}

impl From<&SanitizedPath> for PathBuf {
    fn from(sanitized_path: &SanitizedPath) -> Self {
        sanitized_path.as_path().into()
    }
}

impl AsRef<Path> for SanitizedPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct NormalizeError;

impl Error for NormalizeError {}

impl Display for NormalizeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("parent reference `..` points outside of base directory")
    }
}

/// Copied from stdlib where it's unstable.
///
/// Normalize a path, including `..` without traversing the filesystem.
///
/// Returns an error if normalization would leave leading `..` components.
///
/// <div class="warning">
///
/// This function always resolves `..` to the "lexical" parent.
/// That is "a/b/../c" will always resolve to `a/c` which can change the meaning of the path.
/// In particular, `a/c` and `a/b/../c` are distinct on many systems because `b` may be a symbolic link, so its parent isn't `a`.
///
/// </div>
///
/// [`path::absolute`](absolute) is an alternative that preserves `..`.
/// Or [`Path::canonicalize`] can be used to resolve any `..` by querying the filesystem.
pub fn normalize_lexically(path: &Path) -> Result<PathBuf, NormalizeError> {
    use std::path::Component;

    let mut lexical = PathBuf::new();
    let mut iter = path.components().peekable();

    // Find the root, if any, and add it to the lexical path.
    let root = match iter.peek() {
        Some(Component::ParentDir) => return Err(NormalizeError),
        Some(p @ Component::RootDir) | Some(p @ Component::CurDir) => {
            lexical.push(p);
            iter.next();
            lexical.as_os_str().len()
        }
        Some(Component::Prefix(_)) => unreachable!("no path prefixes on unix"),
        None => return Ok(PathBuf::new()),
        Some(Component::Normal(_)) => 0,
    };

    for component in iter {
        match component {
            Component::RootDir => unreachable!(),
            Component::Prefix(_) => unreachable!("no path prefixes on unix"),
            Component::CurDir => continue,
            Component::ParentDir => {
                // It's an error if ParentDir causes us to go above the "root".
                if lexical.as_os_str().len() == root {
                    return Err(NormalizeError);
                } else {
                    lexical.pop();
                }
            }
            Component::Normal(path) => lexical.push(path),
        }
    }
    Ok(lexical)
}

/// A delimiter to use in `path_query:row_number:column_number` strings parsing.
pub const FILE_ROW_COLUMN_DELIMITER: char = ':';

const ROW_COL_CAPTURE_REGEX: &str = r"(?xs)
    ([^\(]+)\:(?:
        \((\d+)[,:](\d+)\) # filename:(row,column), filename:(row:column)
        |
        \((\d+)\)()     # filename:(row)
    )
    |
    ([^\(]+)(?:
        \((\d+)[,:](\d+)\) # filename(row,column), filename(row:column)
        |
        \((\d+)\)()     # filename(row)
    )
    \:*$
    |
    (.+?)(?:
        \:+(\d+)\:(\d+)\:*$  # filename:row:column
        |
        \:+(\d+)\:*()$       # filename:row
        |
        \:+()()$
    )";

/// A representation of a path-like string with optional row and column numbers.
/// Matching values example: `te`, `test.rs:22`, `te:22:5`, `test.c(22)`, `test.c(22,5)`etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathWithPosition {
    pub path: PathBuf,
    pub row: Option<u32>,
    // Absent if row is absent.
    pub column: Option<u32>,
}

impl PathWithPosition {
    /// Returns a PathWithPosition from a path.
    pub const fn from_path(path: PathBuf) -> Self {
        Self { path, row: None, column: None }
    }

    /// Parses a string that possibly has `:row:column` or `(row, column)` suffix.
    /// Parenthesis format is used by [MSBuild](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-diagnostic-format-for-tasks) compatible tools
    /// Ignores trailing `:`s, so `test.rs:22:` is parsed as `test.rs:22`.
    /// If the suffix parsing fails, the whole string is parsed as a path.
    ///
    /// Be mindful that `test_file:10:1:` is a valid posix filename.
    /// `PathWithPosition` class assumes that the ending position-like suffix is **not** part of the filename.
    pub fn parse_str(s: &str) -> Self {
        let trimmed = s.trim();
        let path = Path::new(trimmed);
        let Some(maybe_file_name_with_row_col) = path.file_name().unwrap_or_default().to_str() else {
            return Self { path: Path::new(s).to_path_buf(), row: None, column: None };
        };
        if maybe_file_name_with_row_col.is_empty() {
            return Self { path: Path::new(s).to_path_buf(), row: None, column: None };
        }

        // Let's avoid repeated init cost on this. It is subject to thread contention, but
        // so far this code isn't called from multiple hot paths. Getting contention here
        // in the future seems unlikely.
        static SUFFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ROW_COL_CAPTURE_REGEX).unwrap());
        match SUFFIX_RE.captures(maybe_file_name_with_row_col).and_then(|captures| {
            let file_name_end = captures.iter().skip(1).flatten().next()?.end();
            let (_, [_, row, column]) = captures.extract();
            Some((file_name_end, row, column))
        }) {
            Some((file_name_end, maybe_row, maybe_column)) => {
                let row = maybe_row.parse::<u32>().ok();
                let column = maybe_column.parse::<u32>().ok();

                Self { path: path.with_file_name(&maybe_file_name_with_row_col[..file_name_end]), row, column }
            }
            None => {
                // The `ROW_COL_CAPTURE_REGEX` deals with separated digits only,
                // but in reality there could be `foo/bar.py:22:in` inputs which we want to match too.
                // The regex mentioned is not very extendable with "digit or random string" checks, so do this here instead.
                let path = Path::new(s);
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    return Self::from_path(path.to_path_buf());
                };
                let delimiter = ':';
                let mut path_parts = file_name.rsplitn(3, delimiter).collect::<Vec<_>>().into_iter().rev().fuse();
                let mut path_string = path_parts
                    .next()
                    .expect("rsplitn should have the rest of the string as its last parameter that we reversed")
                    .to_owned();
                let mut row = None;
                let mut column = None;
                if let Some(maybe_row) = path_parts.next() {
                    if let Ok(parsed_row) = maybe_row.parse::<u32>() {
                        row = Some(parsed_row);
                        if let Some(parsed_column) =
                            path_parts.next().and_then(|maybe_col| maybe_col.parse::<u32>().ok())
                        {
                            column = Some(parsed_column);
                        }
                    } else {
                        path_string.push(delimiter);
                        path_string.push_str(maybe_row);
                    }
                }
                for split in path_parts {
                    path_string.push(delimiter);
                    path_string.push_str(split);
                }

                Self {
                    path: if row.is_some() { path.with_file_name(path_string) } else { path.to_path_buf() },
                    row,
                    column,
                }
            }
        }
    }

    pub fn to_string(&self, path_to_string: &dyn Fn(&PathBuf) -> String) -> String {
        let path_string = path_to_string(&self.path);
        if let Some(row) = self.row {
            if let Some(column) = self.column {
                format!("{path_string}:{row}:{column}")
            } else {
                format!("{path_string}:{row}")
            }
        } else {
            path_string
        }
    }
}

#[derive(Clone)]
pub struct PathMatcher {
    sources: Vec<(String, RelPathBuf, /*trailing separator*/ bool)>,
    glob: GlobSet,
}

impl Debug for PathMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathMatcher").field("sources", &self.sources).finish()
    }
}

impl PartialEq for PathMatcher {
    fn eq(&self, other: &Self) -> bool {
        self.sources.eq(&other.sources)
    }
}

impl Eq for PathMatcher {}

impl PathMatcher {
    pub fn new(globs: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, globset::Error> {
        let globs = globs
            .into_iter()
            .map(|as_str| GlobBuilder::new(as_str.as_ref()).backslash_escape(true).build())
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_globs(globs)
    }

    /// Skips invalid globs, reporting each error to `on_error`.
    /// If the combined set cannot be built, reports the error and matches nothing.
    pub fn new_lenient(
        globs: impl IntoIterator<Item = impl AsRef<str>>,
        mut on_error: impl FnMut(globset::Error),
    ) -> Self {
        let globs = globs
            .into_iter()
            .filter_map(|pattern| match GlobBuilder::new(pattern.as_ref()).backslash_escape(true).build() {
                Ok(glob) => Some(glob),
                Err(error) => {
                    on_error(error);
                    None
                }
            })
            .collect();
        match Self::from_globs(globs) {
            Ok(matcher) => matcher,
            Err(error) => {
                on_error(error);
                Self::default()
            }
        }
    }

    fn from_globs(globs: Vec<Glob>) -> Result<Self, globset::Error> {
        let sources = globs
            .iter()
            .filter_map(|glob| {
                let glob = glob.glob();
                Some((
                    glob.to_string(),
                    RelPath::new(glob.as_ref()).ok().map(std::borrow::Cow::into_owned)?,
                    glob.ends_with('/'),
                ))
            })
            .collect();
        let mut glob_builder = GlobSetBuilder::new();
        for single_glob in globs {
            glob_builder.add(single_glob);
        }
        let glob = glob_builder.build()?;
        Ok(PathMatcher { glob, sources })
    }

    pub fn sources(&self) -> impl Iterator<Item = &str> + Clone {
        self.sources.iter().map(|(source, ..)| source.as_str())
    }

    pub fn is_match<P: AsRef<RelPath>>(&self, other: P) -> bool {
        let other = other.as_ref();
        if self.sources.iter().any(|(_, source, _)| other.starts_with(source) || other.ends_with(source)) {
            return true;
        }
        let other_path = other.display();

        if self.glob.is_match(other_path) {
            return true;
        }

        self.glob.is_match(format!("{other_path}/"))
    }
}

impl Default for PathMatcher {
    fn default() -> Self {
        Self { glob: GlobSet::empty(), sources: vec![] }
    }
}

/// Compares two sequences of consecutive digits for natural sorting.
///
/// This function is a core component of natural sorting that handles numeric comparison
/// in a way that feels natural to humans. It extracts and compares consecutive digit
/// sequences from two iterators, handling various cases like leading zeros and very large numbers.
///
/// # Behavior
///
/// The function implements the following comparison rules:
/// 1. Different numeric values: Compares by actual numeric value (e.g., "2" < "10")
/// 2. Leading zeros: When values are equal, longer sequence wins (e.g., "002" > "2")
/// 3. Large numbers: Falls back to string comparison for numbers that would overflow u128
///
/// # Examples
///
/// ```text
/// "1" vs "2"      -> Less       (different values)
/// "2" vs "10"     -> Less       (numeric comparison)
/// "002" vs "2"    -> Greater    (leading zeros)
/// "10" vs "010"   -> Less       (leading zeros)
/// "999..." vs "1000..." -> Less (large number comparison)
/// ```
///
/// # Implementation Details
///
/// 1. Extracts consecutive digits into strings
/// 2. Compares sequence lengths for leading zero handling
/// 3. For equal lengths, compares digit by digit
/// 4. For different lengths:
///    - Attempts numeric comparison first (for numbers up to 2^128 - 1)
///    - Falls back to string comparison if numbers would overflow
///
/// The function advances both iterators past their respective numeric sequences,
/// regardless of the comparison result.
fn compare_numeric_segments<I>(a_iter: &mut Peekable<I>, b_iter: &mut Peekable<I>) -> Ordering
where
    I: Iterator<Item = char>,
{
    // Collect all consecutive digits into strings
    let mut a_num_str = String::new();
    let mut b_num_str = String::new();

    while let Some(&c) = a_iter.peek() {
        if !c.is_ascii_digit() {
            break;
        }

        a_num_str.push(c);
        a_iter.next();
    }

    while let Some(&c) = b_iter.peek() {
        if !c.is_ascii_digit() {
            break;
        }

        b_num_str.push(c);
        b_iter.next();
    }

    // First compare lengths (handle leading zeros)
    match a_num_str.len().cmp(&b_num_str.len()) {
        Ordering::Equal => {
            // Same length, compare digit by digit
            match a_num_str.cmp(&b_num_str) {
                Ordering::Equal => Ordering::Equal,
                ordering => ordering,
            }
        }

        // Different lengths but same value means leading zeros
        ordering => {
            // Try parsing as numbers first
            if let (Ok(a_val), Ok(b_val)) = (a_num_str.parse::<u128>(), b_num_str.parse::<u128>()) {
                match a_val.cmp(&b_val) {
                    Ordering::Equal => ordering, // Same value, longer one is greater (leading zeros)
                    ord => ord,
                }
            } else {
                // If parsing fails (overflow), compare as strings
                a_num_str.cmp(&b_num_str)
            }
        }
    }
}

/// Performs natural sorting comparison between two strings.
///
/// Natural sorting is an ordering that handles numeric sequences in a way that matches human expectations.
/// For example, "file2" comes before "file10" (unlike standard lexicographic sorting).
///
/// # Characteristics
///
/// * Case-sensitive with lowercase priority: When comparing same letters, lowercase comes before uppercase
/// * Numbers are compared by numeric value, not character by character
/// * Leading zeros affect ordering when numeric values are equal
/// * Can handle numbers larger than u128::MAX (falls back to string comparison)
/// * When strings are equal case-insensitively, lowercase is prioritized (lowercase < uppercase)
///
/// # Algorithm
///
/// The function works by:
/// 1. Processing strings character by character in a case-insensitive manner
/// 2. When encountering digits, treating consecutive digits as a single number
/// 3. Comparing numbers by their numeric value rather than lexicographically
/// 4. For non-numeric characters, using case-insensitive comparison
/// 5. If everything is equal case-insensitively, using case-sensitive comparison as final tie-breaker
pub fn natural_sort(a: &str, b: &str) -> Ordering {
    let mut a_iter = a.chars().peekable();
    let mut b_iter = b.chars().peekable();

    loop {
        match (a_iter.peek(), b_iter.peek()) {
            (None, None) => {
                return b.cmp(a);
            }
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(&a_char), Some(&b_char)) => {
                if a_char.is_ascii_digit() && b_char.is_ascii_digit() {
                    match compare_numeric_segments(&mut a_iter, &mut b_iter) {
                        Ordering::Equal => continue,
                        ordering => return ordering,
                    }
                } else {
                    match a_char.to_ascii_lowercase().cmp(&b_char.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            a_iter.next();
                            b_iter.next();
                        }
                        ordering => return ordering,
                    }
                }
            }
        }
    }
}

/// Case-insensitive natural sort without applying the final lowercase/uppercase tie-breaker.
/// This is useful when comparing individual path components where we want to keep walking
/// deeper components before deciding on casing.
fn natural_sort_no_tiebreak(a: &str, b: &str) -> Ordering {
    if a.eq_ignore_ascii_case(b) { Ordering::Equal } else { natural_sort(a, b) }
}

fn stem_and_extension(filename: &str) -> (Option<&str>, Option<&str>) {
    if filename.is_empty() {
        return (None, None);
    }

    match filename.rsplit_once('.') {
        // Case 1: No dot was found. The entire name is the stem.
        None => (Some(filename), None),

        // Case 2: A dot was found.
        Some((before, after)) => {
            // This is the crucial check for dotfiles like ".bashrc".
            // If `before` is empty, the dot was the first character.
            // In that case, we revert to the "whole name is the stem" logic.
            if before.is_empty() {
                (Some(filename), None)
            } else {
                // Otherwise, we have a standard stem and extension.
                (Some(before), Some(after))
            }
        }
    }
}

/// Controls the lexicographic sorting of file and folder names.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// Case-insensitive natural sort with lowercase preferred in ties.
    /// Numbers in file names are compared by value (e.g., `file2` before `file10`).
    #[default]
    Default,
    /// Uppercase names are grouped before lowercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Upper,
    /// Lowercase names are grouped before uppercase names, with case-insensitive
    /// natural sort within each group. Dot-prefixed names sort before both groups.
    Lower,
    /// Pure Unicode codepoint comparison. No case folding, no natural number sorting.
    /// Uppercase ASCII sorts before lowercase. Accented characters sort after ASCII.
    Unicode,
}

/// Controls how files and directories are ordered relative to each other.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// Directories are listed before files at each level.
    #[default]
    DirectoriesFirst,
    /// Files and directories are interleaved alphabetically.
    Mixed,
    /// Files are listed before directories at each level.
    FilesFirst,
}

fn case_group_key(name: &str, order: SortOrder) -> u8 {
    let first = match name.chars().next() {
        Some(c) => c,
        None => return 0,
    };
    match order {
        SortOrder::Upper => {
            if first.is_lowercase() {
                1
            } else {
                0
            }
        }
        SortOrder::Lower if first.is_uppercase() => 1,
        _ => 0,
    }
}

fn compare_strings(a: &str, b: &str, order: SortOrder) -> Ordering {
    match order {
        SortOrder::Unicode => a.cmp(b),
        _ => natural_sort(a, b),
    }
}

fn compare_strings_no_tiebreak(a: &str, b: &str, order: SortOrder) -> Ordering {
    match order {
        SortOrder::Unicode => a.cmp(b),
        _ => natural_sort_no_tiebreak(a, b),
    }
}

pub fn compare_rel_paths_by(
    (path_a, a_is_file): (&RelPath, bool),
    (path_b, b_is_file): (&RelPath, bool),
    mode: SortMode,
    order: SortOrder,
) -> Ordering {
    let needs_final_tiebreak = mode != SortMode::DirectoriesFirst && !(ptr::eq(path_a, path_b) || path_a == path_b);

    let mut components_a = path_a.components();
    let mut components_b = path_b.components();

    loop {
        match (components_a.next(), components_b.next()) {
            (Some(component_a), Some(component_b)) => {
                let a_leaf_file = a_is_file && components_a.rest().is_empty();
                let b_leaf_file = b_is_file && components_b.rest().is_empty();

                let file_dir_ordering = match mode {
                    SortMode::DirectoriesFirst => a_leaf_file.cmp(&b_leaf_file),
                    SortMode::FilesFirst => b_leaf_file.cmp(&a_leaf_file),
                    SortMode::Mixed => Ordering::Equal,
                };

                if !file_dir_ordering.is_eq() {
                    return file_dir_ordering;
                }

                let (a_stem, a_ext) = if a_leaf_file { stem_and_extension(component_a) } else { Default::default() };
                let (b_stem, b_ext) = if b_leaf_file { stem_and_extension(component_b) } else { Default::default() };
                let a_key = if a_leaf_file { a_stem } else { Some(component_a) };
                let b_key = if b_leaf_file { b_stem } else { Some(component_b) };

                let ordering = match (a_key, b_key) {
                    (Some(a), Some(b)) => {
                        let name_cmp =
                            case_group_key(a, order).cmp(&case_group_key(b, order)).then_with(|| match mode {
                                SortMode::DirectoriesFirst => compare_strings(a, b, order),
                                _ => compare_strings_no_tiebreak(a, b, order),
                            });

                        let name_cmp = if mode == SortMode::Mixed {
                            name_cmp.then_with(|| match (a_leaf_file, b_leaf_file) {
                                (true, false) if a.eq_ignore_ascii_case(b) => Ordering::Greater,
                                (false, true) if a.eq_ignore_ascii_case(b) => Ordering::Less,
                                _ => Ordering::Equal,
                            })
                        } else {
                            name_cmp
                        };

                        name_cmp.then_with(|| {
                            if a_leaf_file && b_leaf_file {
                                match order {
                                    SortOrder::Unicode => a_ext.unwrap_or_default().cmp(b_ext.unwrap_or_default()),
                                    _ => {
                                        let a_ext_str = a_ext.unwrap_or_default().to_lowercase();
                                        let b_ext_str = b_ext.unwrap_or_default().to_lowercase();
                                        a_ext_str.cmp(&b_ext_str)
                                    }
                                }
                            } else {
                                Ordering::Equal
                            }
                        })
                    }
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                    (None, None) => Ordering::Equal,
                };

                if !ordering.is_eq() {
                    return ordering;
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => {
                if needs_final_tiebreak {
                    return compare_strings(path_a.as_unix_str(), path_b.as_unix_str(), order);
                }
                return Ordering::Equal;
            }
        }
    }
}

pub fn compare_paths((path_a, a_is_file): (&Path, bool), (path_b, b_is_file): (&Path, bool)) -> Ordering {
    let mut components_a = path_a.components().peekable();
    let mut components_b = path_b.components().peekable();

    loop {
        match (components_a.next(), components_b.next()) {
            (Some(component_a), Some(component_b)) => {
                let a_is_file = components_a.peek().is_none() && a_is_file;
                let b_is_file = components_b.peek().is_none() && b_is_file;

                let ordering = a_is_file.cmp(&b_is_file).then_with(|| {
                    let path_a = Path::new(component_a.as_os_str());
                    let path_string_a =
                        if a_is_file { path_a.file_stem() } else { path_a.file_name() }.map(|s| s.to_string_lossy());

                    let path_b = Path::new(component_b.as_os_str());
                    let path_string_b =
                        if b_is_file { path_b.file_stem() } else { path_b.file_name() }.map(|s| s.to_string_lossy());

                    let compare_components = match (path_string_a, path_string_b) {
                        (Some(a), Some(b)) => natural_sort(&a, &b),
                        (Some(_), None) => Ordering::Greater,
                        (None, Some(_)) => Ordering::Less,
                        (None, None) => Ordering::Equal,
                    };

                    compare_components.then_with(|| {
                        if a_is_file && b_is_file {
                            let ext_a = path_a.extension().unwrap_or_default();
                            let ext_b = path_b.extension().unwrap_or_default();
                            ext_a.cmp(ext_b)
                        } else {
                            Ordering::Equal
                        }
                    })
                });

                if !ordering.is_eq() {
                    return ordering;
                }
            }
            (Some(_), None) => break Ordering::Greater,
            (None, Some(_)) => break Ordering::Less,
            (None, None) => break Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::PathWithPosition;

    #[test]
    fn path_with_position_parse_str() {
        use std::path::PathBuf;
        assert_eq!(
            PathWithPosition::parse_str("test_file"),
            PathWithPosition { path: PathBuf::from("test_file"), row: None, column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file:10"),
            PathWithPosition { path: PathBuf::from("test_file"), row: Some(10), column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: None, column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:1"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: Some(1), column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:1:2"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: Some(1), column: Some(2) }
        );
    }

    #[test]
    fn path_with_position_parse_str_2() {
        use std::path::PathBuf;
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:a"),
            PathWithPosition { path: PathBuf::from("test_file.rs:a"), row: None, column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:a:b"),
            PathWithPosition { path: PathBuf::from("test_file.rs:a:b"), row: None, column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: None, column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs::1"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: Some(1), column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:1::"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: Some(1), column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs::1:2"),
            PathWithPosition { path: PathBuf::from("test_file.rs"), row: Some(1), column: Some(2) }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:1::2"),
            PathWithPosition { path: PathBuf::from("test_file.rs:1"), row: Some(2), column: None }
        );
        assert_eq!(
            PathWithPosition::parse_str("test_file.rs:1:2:3"),
            PathWithPosition { path: PathBuf::from("test_file.rs:1"), row: Some(2), column: Some(3) }
        );
    }
}
