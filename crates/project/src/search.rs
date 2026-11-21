use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io;
use std::io::Read;
use std::ops::Range;
use std::result;
use std::str as std_str;
use std::sync::Arc;
use std::sync::LazyLock;

use aho_corasick::AhoCorasick;
use aho_corasick::AhoCorasickBuilder;
use anyhow::Ok;
use anyhow::Result;
use async_macos::yield_now;
use fancy_regex::Captures;
use fancy_regex::Expander;
use fancy_regex::Regex;
use fancy_regex::RegexBuilder;
use gpui::Entity;
use language::Buffer;
use language::BufferSnapshot;
use language::CharKind;
use text::Anchor;
use util::paths::PathMatcher;
use util::rel_path::RelPath;

#[derive(Debug)]
pub enum SearchResult {
    Buffer { buffer: Entity<Buffer>, ranges: Vec<Range<Anchor>> },
    LimitReached,
    WaitingForScan,
    Searching,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchInputKind {
    Query,
    Include,
    Exclude,
}

#[derive(Clone, Debug)]
pub struct SearchInputs {
    query: Arc<str>,
    files_to_include: PathMatcher,
    files_to_exclude: PathMatcher,
    match_full_paths: bool,
    buffers: Option<Vec<Entity<Buffer>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPositionHint {
    Line(u32),
    ByteOffset(usize),
}

impl Default for MatchPositionHint {
    fn default() -> Self {
        Self::Line(0)
    }
}

impl SearchInputs {
    pub fn as_str(&self) -> &str {
        self.query.as_ref()
    }
    pub const fn files_to_include(&self) -> &PathMatcher {
        &self.files_to_include
    }
    pub const fn files_to_exclude(&self) -> &PathMatcher {
        &self.files_to_exclude
    }
    pub const fn buffers(&self) -> &Option<Vec<Entity<Buffer>>> {
        &self.buffers
    }
}
#[derive(Clone, Debug)]
pub enum SearchQuery {
    Text {
        search: AhoCorasick,
        replacement: Option<String>,
        whole_word: bool,
        case_sensitive: bool,
        include_ignored: bool,
        inner: SearchInputs,
    },
    Regex {
        regex: Regex,
        replacement: Option<String>,
        /// `replacement` with escape sequences resolved and capture references delimited, ready
        /// for `Captures::expand`. Only set for queries that are not `escaped`.
        replacement_template: Option<String>,
        whole_word: bool,
        case_sensitive: bool,
        include_ignored: bool,
        one_match_per_line: bool,
        inner: SearchInputs,
        escaped: bool,
    },
}

static WORD_MATCH_TEST: LazyLock<Regex> =
    LazyLock::new(|| RegexBuilder::new(r"\B").build().expect("Failed to create WORD_MATCH_TEST"));

impl SearchQuery {
    /// Create a text query.
    ///
    /// If `match_full_paths` is true, include/exclude patterns will always be matched against fully qualified project paths beginning with a project root.
    /// If `match_full_paths` is false, patterns will be matched against worktree-relative paths.
    pub fn text(
        query: impl ToString,
        whole_word: bool,
        case_sensitive: bool,
        include_ignored: bool,
        files_to_include: PathMatcher,
        files_to_exclude: PathMatcher,
        match_full_paths: bool,
        buffers: Option<Vec<Entity<Buffer>>>,
    ) -> Result<Self> {
        let mut query = query.to_string();
        text::LineEnding::normalize(&mut query);
        if !case_sensitive && !query.is_ascii() {
            // AhoCorasickBuilder doesn't support case-insensitive search with unicode characters
            // Fallback to regex search as recommended by
            // https://docs.rs/aho-corasick/1.1/aho_corasick/struct.AhoCorasickBuilder.html#method.ascii_case_insensitive
            return Self::escaped_regex(
                query,
                whole_word,
                case_sensitive,
                include_ignored,
                files_to_include,
                files_to_exclude,
                match_full_paths,
                buffers,
            );
        }
        let search = AhoCorasickBuilder::new().ascii_case_insensitive(!case_sensitive).build([&query])?;
        let inner = SearchInputs { query: query.into(), files_to_exclude, files_to_include, match_full_paths, buffers };
        Ok(Self::Text { search, replacement: None, whole_word, case_sensitive, include_ignored, inner })
    }

    /// Create a regex query.
    ///
    /// If `match_full_paths` is true, include/exclude patterns will be matched against fully qualified project paths
    /// beginning with a project root name. If false, they will be matched against project-relative paths (which don't start
    /// with their respective project root).
    pub fn regex(
        query: impl ToString,
        whole_word: bool,
        case_sensitive: bool,
        include_ignored: bool,
        one_match_per_line: bool,
        files_to_include: PathMatcher,
        files_to_exclude: PathMatcher,
        match_full_paths: bool,
        buffers: Option<Vec<Entity<Buffer>>>,
    ) -> Result<Self> {
        let query = query.to_string();
        let inner = SearchInputs {
            query: Arc::from(query.as_str()),
            files_to_include,
            files_to_exclude,
            match_full_paths,
            buffers,
        };
        Self::build_regex(query, whole_word, case_sensitive, include_ignored, one_match_per_line, inner, false)
    }

    /// Create a regex query from a literal string, escaping any regex
    /// metacharacters so that the resulting query matches the literal text.
    ///
    /// Unlike `regex`, the query stored on the resulting `SearchQuery` is the
    /// original unescaped text, so `as_str` returns what the user typed.
    pub fn escaped_regex(
        query: impl ToString,
        whole_word: bool,
        case_sensitive: bool,
        include_ignored: bool,
        files_to_include: PathMatcher,
        files_to_exclude: PathMatcher,
        match_full_paths: bool,
        buffers: Option<Vec<Entity<Buffer>>>,
    ) -> Result<Self> {
        let mut query = query.to_string();
        text::LineEnding::normalize(&mut query);
        let inner = SearchInputs {
            query: Arc::from(query.as_str()),
            files_to_include,
            files_to_exclude,
            match_full_paths,
            buffers,
        };
        Self::build_regex(regex::escape(&query), whole_word, case_sensitive, include_ignored, false, inner, true)
    }

    fn build_regex(
        mut pattern: String,
        whole_word: bool,
        mut case_sensitive: bool,
        include_ignored: bool,
        one_match_per_line: bool,
        inner: SearchInputs,
        escaped: bool,
    ) -> Result<Self> {
        if let Some((case_sensitive_from_pattern, new_pattern)) = Self::case_sensitive_from_pattern(&pattern) {
            case_sensitive = case_sensitive_from_pattern;
            pattern = new_pattern;
        }

        if whole_word {
            let mut word_pattern = String::new();
            if let Some(first) = pattern.get(0..1)
                && WORD_MATCH_TEST.is_match(first).is_ok_and(|x| !x)
            {
                word_pattern.push_str("\\b");
            }
            word_pattern.push_str(&pattern);
            if let Some(last) = pattern.get(pattern.len() - 1..)
                && WORD_MATCH_TEST.is_match(last).is_ok_and(|x| !x)
            {
                word_pattern.push_str("\\b");
            }
            pattern = word_pattern;
        }

        let regex =
            RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).multi_line(true).crlf(true).build()?;
        Ok(Self::Regex {
            regex,
            replacement: None,
            replacement_template: None,
            whole_word,
            case_sensitive,
            include_ignored,
            inner,
            one_match_per_line,
            escaped,
        })
    }

    /// Extracts case sensitivity settings from pattern items in the provided
    /// query and returns the same query, with the pattern items removed.
    ///
    /// The following pattern modifiers are supported:
    ///
    /// - `\c` (case_sensitive: false)
    /// - `\C` (case_sensitive: true)
    ///
    /// If no pattern item were found, `None` will be returned.
    fn case_sensitive_from_pattern(query: &str) -> Option<(bool, String)> {
        if !(query.contains("\\c") || query.contains("\\C")) {
            return None;
        }

        let mut was_escaped = false;
        let mut new_query = String::new();
        let mut is_case_sensitive = None;

        for c in query.chars() {
            if was_escaped {
                if c == 'c' {
                    is_case_sensitive = Some(false);
                } else if c == 'C' {
                    is_case_sensitive = Some(true);
                } else {
                    new_query.push('\\');
                    new_query.push(c);
                }
                was_escaped = false;
            } else if c == '\\' {
                was_escaped = true;
            } else {
                new_query.push(c);
            }
        }

        is_case_sensitive.map(|c| (c, new_query))
    }

    pub fn with_replacement(mut self, new_replacement: String) -> Self {
        match &mut self {
            Self::Text { replacement, .. } => *replacement = Some(new_replacement),
            Self::Regex { regex, replacement, replacement_template, escaped, .. } => {
                // Prepared here rather than in `replacement_for`, which runs for every hit.
                *replacement_template = (!*escaped).then(|| prepare_replacement_template(regex, &new_replacement));
                *replacement = Some(new_replacement);
            }
        }
        self
    }

    pub async fn detect(&self, reader: &mut (dyn Read + Send)) -> Result<Option<MatchPositionHint>> {
        let query_str = self.as_str();
        if query_str.is_empty() {
            return Ok(None);
        }

        match self {
            Self::Text { search, .. } if !query_str.contains('\n') => detect_single_line(search, reader).await,
            Self::Text { search, .. } => {
                let mut text = read_to_string(reader).await?;
                text::LineEnding::normalize(&mut text);
                if search.is_match(&text) { Ok(Some(MatchPositionHint::default())) } else { Ok(None) }
            }
            Self::Regex { regex, .. } => {
                let mut text = read_to_string(reader).await?;
                text::LineEnding::normalize(&mut text);
                if let Some(m) = regex.find(&text)? {
                    Ok(Some(MatchPositionHint::ByteOffset(m.start())))
                } else {
                    Ok(None)
                }
            }
        }
    }
    /// Returns the replacement text for this `SearchQuery`.
    pub fn replacement(&self) -> Option<&str> {
        match self {
            SearchQuery::Text { replacement, .. } | SearchQuery::Regex { replacement, .. } => replacement.as_deref(),
        }
    }
    /// Expands `hit` against its line so lookaround assertions retain context.
    pub fn replacement_for<'a>(&self, line: &'a str, hit: Range<usize>) -> Option<Cow<'a, str>> {
        match self {
            SearchQuery::Text { replacement, .. } | SearchQuery::Regex { replacement, escaped: true, .. } => {
                replacement.clone().map(Cow::from)
            }

            SearchQuery::Regex { regex, replacement_template: Some(replacement), escaped: false, .. } => {
                let captures = regex
                    .captures_from_pos(line, hit.start)
                    .ok()
                    .flatten()
                    .filter(|captures| captures.get(0).is_some_and(|m| m.range() == hit));
                let Some(captures) = captures else {
                    // The pattern is not guaranteed to match the whole line, for instance when
                    // searching within a selection that starts or ends mid-line, so fall back to
                    // matching the hit on its own.
                    return Some(regex.replace(line.get(hit)?, replacement.as_str()));
                };
                let mut replaced = String::new();
                captures.expand(replacement, &mut replaced);
                Some(Cow::Owned(replaced))
            }

            SearchQuery::Regex { replacement_template: None, .. } => None,
        }
    }

    pub async fn search(&self, buffer: &BufferSnapshot, subrange: Option<Range<usize>>) -> Vec<Range<usize>> {
        const YIELD_INTERVAL: usize = 20000;

        if self.as_str().is_empty() {
            return Vec::default();
        }

        let range_offset = subrange.as_ref().map_or(0, |r| r.start);
        let rope = if let Some(range) = subrange { buffer.as_rope().slice(range) } else { buffer.as_rope().clone() };

        let mut matches = Vec::new();
        match self {
            Self::Text { search, whole_word, .. } => {
                for (ix, mat) in search.stream_find_iter(rope.bytes_in_range(0..rope.len())).enumerate() {
                    if (ix + 1) % YIELD_INTERVAL == 0 {
                        yield_now().await;
                    }

                    let mat = mat.expect("reading a rope can't fail");
                    if *whole_word {
                        let classifier = buffer.char_classifier_at(range_offset + mat.start());

                        let prev_kind = rope.reversed_chars_at(mat.start()).next().map(|c| classifier.kind(c));
                        let start_kind =
                            classifier.kind(rope.chars_at(mat.start()).next().expect("matches are non-empty"));
                        let end_kind =
                            classifier.kind(rope.reversed_chars_at(mat.end()).next().expect("matches are non-empty"));
                        let next_kind = rope.chars_at(mat.end()).next().map(|c| classifier.kind(c));
                        if (Some(start_kind) == prev_kind && start_kind == CharKind::Word)
                            || (Some(end_kind) == next_kind && end_kind == CharKind::Word)
                        {
                            continue;
                        }
                    }
                    matches.push(mat.start()..mat.end());
                }
            }

            Self::Regex { regex, one_match_per_line, .. } => {
                let text = rope.to_string();
                let mut seen_lines = BTreeSet::default();
                for (ix, mat) in regex.find_iter(&text).enumerate() {
                    if (ix + 1) % YIELD_INTERVAL == 0 {
                        yield_now().await;
                    }

                    if let result::Result::Ok(mat) = mat {
                        let should_push = if *one_match_per_line {
                            // ensure that only one match per line is returned.
                            let pos = buffer.offset_to_point(mat.start());
                            seen_lines.insert(pos.row)
                        } else {
                            true
                        };
                        if should_push {
                            matches.push(mat.start()..mat.end());
                        }
                    }
                }
            }
        }

        matches
    }

    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    pub fn as_str(&self) -> &str {
        self.as_inner().as_str()
    }

    pub const fn whole_word(&self) -> bool {
        match self {
            Self::Text { whole_word, .. } => *whole_word,
            Self::Regex { whole_word, .. } => *whole_word,
        }
    }

    pub const fn case_sensitive(&self) -> bool {
        match self {
            Self::Text { case_sensitive, .. } => *case_sensitive,
            Self::Regex { case_sensitive, .. } => *case_sensitive,
        }
    }

    pub const fn include_ignored(&self) -> bool {
        match self {
            Self::Text { include_ignored, .. } => *include_ignored,
            Self::Regex { include_ignored, .. } => *include_ignored,
        }
    }

    pub const fn is_regex(&self) -> bool {
        matches!(self, Self::Regex { .. })
    }

    pub const fn replacement_requires_context(&self) -> bool {
        matches!(self, Self::Regex { escaped: false, .. })
    }

    pub const fn files_to_include(&self) -> &PathMatcher {
        self.as_inner().files_to_include()
    }

    pub const fn files_to_exclude(&self) -> &PathMatcher {
        self.as_inner().files_to_exclude()
    }

    pub const fn buffers(&self) -> Option<&Vec<Entity<Buffer>>> {
        self.as_inner().buffers.as_ref()
    }

    pub const fn is_opened_only(&self) -> bool {
        self.as_inner().buffers.is_some()
    }

    pub fn filters_path(&self) -> bool {
        !(self.files_to_exclude().sources().next().is_none() && self.files_to_include().sources().next().is_none())
    }

    pub const fn match_full_paths(&self) -> bool {
        self.as_inner().match_full_paths
    }

    /// Check match full paths to determine whether you're required to pass a fully qualified
    /// project path (starts with a project root).
    pub fn match_path(&self, file_path: &RelPath) -> bool {
        let mut path = file_path.to_rel_path_buf();
        loop {
            if self.files_to_exclude().is_match(&path) {
                return false;
            } else if self.files_to_include().sources().next().is_none() || self.files_to_include().is_match(&path) {
                return true;
            } else if !path.pop() {
                return false;
            }
        }
    }
    pub const fn as_inner(&self) -> &SearchInputs {
        match self {
            Self::Regex { inner, .. } | Self::Text { inner, .. } => inner,
        }
    }

    pub fn search_str(&self, text: &str) -> Vec<Range<usize>> {
        if self.as_str().is_empty() {
            return Vec::new();
        }

        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

        let mut matches = Vec::new();
        match self {
            Self::Text { search, whole_word, .. } => {
                for mat in search.find_iter(text.as_bytes()) {
                    if *whole_word {
                        let prev_char = text[..mat.start()].chars().last();
                        let next_char = text[mat.end()..].chars().next();
                        if prev_char.is_some_and(is_word_char) || next_char.is_some_and(is_word_char) {
                            continue;
                        }
                    }
                    matches.push(mat.start()..mat.end());
                }
            }
            Self::Regex { regex, .. } => {
                for mat in regex.find_iter(text).flatten() {
                    matches.push(mat.start()..mat.end());
                }
            }
        }
        matches
    }
}

async fn detect_single_line(search: &AhoCorasick, reader: &mut (dyn Read + Send)) -> Result<Option<MatchPositionHint>> {
    const BLOCK_BYTES: usize = 64 * 1024;
    let batch_size = search.max_pattern_len().max(BLOCK_BYTES);
    let carry_len = search.max_pattern_len().saturating_sub(1).max(3);
    let mut block = vec![0; BLOCK_BYTES];
    let mut window = TextSearchWindow::default();
    loop {
        let batch_end = window.batch_start + if window.first_match.is_some() { BLOCK_BYTES } else { batch_size };
        let limit = (batch_end - window.bytes.len()).min(BLOCK_BYTES);
        let read_result = match reader.read(&mut block[..limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result,
        };
        if let result::Result::Ok(read) = read_result {
            window.extend(&block[..read], batch_size + carry_len)?;
            if read > 0 && window.bytes.len() < batch_end {
                yield_now().await;
                continue;
            }
        }
        let line_end = window.find_matching_line_end(search);
        window.validate_prefix(line_end, read_result.is_err())?;
        if line_end.is_some() {
            return Ok(window.first_match);
        }
        if read_result? == 0 {
            if window.validated_len < window.bytes.len() {
                return Err(invalid_data());
            }
            return Ok(window.first_match);
        }
        window.retain_suffix(if window.first_match.is_some() { 3 } else { carry_len });
        yield_now().await;
    }
}

#[derive(Default)]
struct TextSearchWindow {
    bytes: Vec<u8>,
    batch_start: usize,
    validated_len: usize,
    lines_before_window: usize,
    first_match: Option<MatchPositionHint>,
}

impl TextSearchWindow {
    fn extend(&mut self, bytes: &[u8], max_capacity: usize) -> Result<()> {
        if self.bytes.len() + bytes.len() > self.bytes.capacity() {
            let capacity =
                self.bytes.capacity().saturating_mul(2).max(self.bytes.len() + bytes.len()).min(max_capacity);
            self.bytes.try_reserve_exact(capacity - self.bytes.len())?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn find_matching_line_end(&mut self, search: &AhoCorasick) -> Option<usize> {
        let start = if self.first_match.is_some() {
            self.batch_start
        } else {
            let found = search.find(&self.bytes)?;
            let line = self.lines_before_window + count_newlines(&self.bytes[..found.start()]);
            self.first_match = Some(MatchPositionHint::Line(u32::try_from(line).unwrap_or(u32::MAX)));
            found.end()
        };
        self.bytes[start..].iter().position(|&byte| byte == b'\n').map(|offset| start + offset + 1)
    }

    fn validate_prefix(&mut self, line_end: Option<usize>, read_failed: bool) -> Result<()> {
        let validation_end = line_end.unwrap_or_else(|| {
            if read_failed {
                self.bytes
                    .iter()
                    .rposition(|&byte| byte == b'\n')
                    .map_or(self.validated_len, |offset| (offset + 1).max(self.validated_len))
            } else {
                self.bytes.len()
            }
        });
        self.validated_len += match std_str::from_utf8(&self.bytes[self.validated_len..validation_end]) {
            Err(error) if error.error_len().is_some() => return Err(invalid_data()),
            Err(error) => error.valid_up_to(),
            _ => validation_end - self.validated_len,
        };
        Ok(())
    }

    fn retain_suffix(&mut self, length: usize) {
        let consumed = self.bytes.len().saturating_sub(length);
        self.lines_before_window += count_newlines(&self.bytes[..consumed]);
        self.bytes.drain(..consumed);
        self.batch_start = self.bytes.len();
        self.validated_len -= consumed;
    }
}

async fn read_to_string(reader: &mut (dyn Read + Send)) -> Result<String> {
    const BLOCK_BYTES: usize = 64 * 1024;
    let mut bytes = Vec::new();
    loop {
        let bytes_read = (&mut *reader).take(BLOCK_BYTES as u64).read_to_end(&mut bytes)?;
        if bytes_read < BLOCK_BYTES {
            return String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err).into());
        }
        yield_now().await;
    }
}

/// Resolves the escape sequences supported in replacements and delimits capture references, so
/// that `Captures::expand` can use the result for every hit as is.
fn prepare_replacement_template(regex: &Regex, replacement: &str) -> String {
    static TEXT_REPLACEMENT_SPECIAL_CHARACTERS_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\\\|\\n|\\t").expect("static regex is valid"));
    let replacement = TEXT_REPLACEMENT_SPECIAL_CHARACTERS_REGEX.replace_all(replacement, |c: &Captures<str>| {
        match c.get(0).expect("capture group 0 always exists").as_str() {
            r"\\" => "\\",
            r"\n" => "\n",
            r"\t" => "\t",
            x => unreachable!("Unexpected escape sequence: {}", x),
        }
    });
    delimit_capture_references(regex, &replacement).into_owned()
}

/// `fancy_regex` reads an unbraced capture reference up to the first character that is neither a
/// Unicode alphanumeric nor `_`, so `$1ème` refers to a group named `1ème` and expands to nothing.
/// Unless such a group exists, end the reference before its first non-ASCII character by rewriting
/// it as `${1}ème`. Every other reference is left for `fancy_regex` to interpret.
fn delimit_capture_references<'a>(regex: &Regex, replacement: &'a str) -> Cow<'a, str> {
    let mut delimited = String::new();
    let mut copied_len = 0;
    let mut search_start = 0;
    while let Some(offset) = replacement[search_start..].find('$') {
        let name_start = search_start + offset + 1;
        let rest = &replacement[name_start..];
        if rest.starts_with('$') {
            search_start = name_start + 1;
            continue;
        }
        let name_len =
            rest.find(|character: char| !(character.is_alphanumeric() || character == '_')).unwrap_or(rest.len());
        let name_end = name_start + name_len;
        search_start = name_end;
        let name = &rest[..name_len];
        let ascii_len = name.find(|character: char| !character.is_ascii()).unwrap_or(name_len);
        if ascii_len == 0 || ascii_len == name_len {
            continue;
        }
        // `Expander::check` looks up a single name, whereas `Regex::capture_names` lists every
        // group. It allocates an error when the name is missing, so this must not run per hit.
        let reference = &replacement[name_start - 1..name_end];
        if Expander::default().check(reference, regex).is_ok() {
            continue;
        }
        let ascii_end = name_start + ascii_len;
        delimited.push_str(&replacement[copied_len..name_start]);
        delimited.push('{');
        delimited.push_str(&replacement[name_start..ascii_end]);
        delimited.push('}');
        copied_len = ascii_end;
    }
    if delimited.is_empty() {
        Cow::Borrowed(replacement)
    } else {
        delimited.push_str(&replacement[copied_len..]);
        Cow::Owned(delimited)
    }
}

fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().map(|&byte| usize::from(byte == b'\n')).sum()
}

fn invalid_data() -> anyhow::Error {
    anyhow::Error::from(io::Error::from(io::ErrorKind::InvalidData))
}

#[cfg(test)]
mod tests {
    use util::paths::PathMatcher;

    use super::SearchQuery;

    fn regex_query(pattern: &str, replacement: &str) -> SearchQuery {
        SearchQuery::regex(
            pattern,
            false,
            true,
            false,
            false,
            PathMatcher::default(),
            PathMatcher::default(),
            false,
            None,
        )
        .expect("valid regex")
        .with_replacement(replacement.to_owned())
    }

    fn replace_first_hit(pattern: &str, replacement: &str, line: &str) -> Option<String> {
        let query = regex_query(pattern, replacement);
        let hit = query.search_str(line).into_iter().next()?;
        query.replacement_for(line, hit).map(|replacement| replacement.into_owned())
    }

    #[test]
    fn replacement_for_capture_reference_followed_by_non_ascii_text() {
        assert_eq!(replace_first_hit(r"(\d+)e", "$1ème", "2e étage").as_deref(), Some("2ème"));
        assert_eq!(replace_first_hit(r"(\d+)e", "${1}ème", "2e étage").as_deref(), Some("2ème"));
        assert_eq!(replace_first_hit(r"(?<number>\d+)e", "$numberème", "2e étage").as_deref(), Some("2ème"));
        assert_eq!(replace_first_hit(r"第(\d+)回", "第$1章", "第12回").as_deref(), Some("第12章"));
        // A group whose name really continues with non-ASCII characters still resolves.
        assert_eq!(replace_first_hit(r"(?<né>\d+)e", "$né!", "2e").as_deref(), Some("2!"));
        // ASCII-only references and escaped dollars are untouched.
        assert_eq!(replace_first_hit(r"(\d+)e", "$1x $$1", "2e").as_deref(), Some(" $1"));
    }

    #[test]
    fn replacement_for_capture_reference_when_hit_does_not_match_in_line() {
        // The lookbehind keeps the pattern from matching at the hit within the line, which
        // exercises the fallback that matches the hit on its own.
        let query = regex_query(r"(?<!x)(\d+)e", "$1ème");
        let line = "x2e";
        assert_eq!(query.replacement_for(line, 1..line.len()).as_deref(), Some("2ème"));
    }

    #[test]
    fn with_replacement_prepares_the_replacement_again() {
        let query = regex_query(r"(\d+)e", "$1ème");
        assert_eq!(query.replacement(), Some("$1ème"));
        assert_eq!(query.replacement_for("2e", 0..2).as_deref(), Some("2ème"));

        let query = query.with_replacement(r"$1è\t".to_owned());
        assert_eq!(query.replacement(), Some(r"$1è\t"));
        assert_eq!(query.replacement_for("2e", 0..2).as_deref(), Some("2è\t"));
    }
}
