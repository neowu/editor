use std::borrow::Cow;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

#[derive(Clone, Copy, Debug, Eq, PartialEq, strum::EnumIter, strum::EnumString, strum::IntoStaticStr)]
pub enum QueryFile {
    #[strum(serialize = "highlights.scm")]
    Highlights,
    #[strum(serialize = "brackets.scm")]
    Brackets,
    #[strum(serialize = "outline.scm")]
    Outline,
    #[strum(serialize = "indents.scm")]
    Indents,
    #[strum(serialize = "injections.scm")]
    Injections,
    #[strum(serialize = "overrides.scm")]
    Overrides,
    #[strum(serialize = "runnables.scm")]
    Runnables,
    #[strum(serialize = "textobjects.scm")]
    TextObjects,
    #[strum(serialize = "spellcheck.scm")]
    SpellCheck,
}

impl QueryFile {
    pub fn file_name(self) -> &'static str {
        self.into()
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct QueryFiles: u16 {
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const HIGHLIGHTS = 1 << 0;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const BRACKETS = 1 << 1;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const OUTLINE = 1 << 2;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const INDENTS = 1 << 3;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const INJECTIONS = 1 << 4;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const OVERRIDES = 1 << 5;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const RUNNABLES = 1 << 7;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const TEXT_OBJECTS = 1 << 9;
        // lint: expect(excess_visibility) visibility is written by the `bitflags!` macro
        const SPELL_CHECK = 1 << 10;
    }
}

impl From<QueryFile> for QueryFiles {
    fn from(query_file: QueryFile) -> Self {
        match query_file {
            QueryFile::Highlights => Self::HIGHLIGHTS,
            QueryFile::Brackets => Self::BRACKETS,
            QueryFile::Outline => Self::OUTLINE,
            QueryFile::Indents => Self::INDENTS,
            QueryFile::Injections => Self::INJECTIONS,
            QueryFile::Overrides => Self::OVERRIDES,
            QueryFile::Runnables => Self::RUNNABLES,
            QueryFile::TextObjects => Self::TEXT_OBJECTS,
            QueryFile::SpellCheck => Self::SPELL_CHECK,
        }
    }
}

impl FromIterator<QueryFile> for QueryFiles {
    fn from_iter<T: IntoIterator<Item = QueryFile>>(query_files: T) -> Self {
        query_files.into_iter().fold(Self::empty(), |files, query_file| files | query_file.into())
    }
}

impl Serialize for QueryFiles {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.bits())
    }
}

impl<'de> Deserialize<'de> for QueryFiles {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from_bits_retain(u16::deserialize(deserializer)?))
    }
}

#[derive(Debug)]
pub struct QueryFileContents {
    query_file: QueryFile,
    contents: Cow<'static, str>,
}

impl QueryFileContents {
    pub const fn new(query_file: QueryFile, contents: Cow<'static, str>) -> Self {
        Self { query_file, contents }
    }
}

/// Tree-sitter language queries for a given language.
#[derive(Debug, Default)]
pub struct LanguageQueries {
    pub highlights: Option<Cow<'static, str>>,
    pub(crate) brackets: Option<Cow<'static, str>>,
    pub(crate) indents: Option<Cow<'static, str>>,
    pub(crate) outline: Option<Cow<'static, str>>,
    pub(crate) injections: Option<Cow<'static, str>>,
    pub(crate) overrides: Option<Cow<'static, str>>,
    pub(crate) runnables: Option<Cow<'static, str>>,
    pub(crate) text_objects: Option<Cow<'static, str>>,
    pub(crate) spell_check: Option<Cow<'static, str>>,
}

impl LanguageQueries {
    pub fn from_files(files: impl IntoIterator<Item = QueryFileContents>) -> Self {
        let mut queries = Self::default();
        for QueryFileContents { query_file, contents } in files {
            let field = match query_file {
                QueryFile::Highlights => &mut queries.highlights,
                QueryFile::Brackets => &mut queries.brackets,
                QueryFile::Outline => &mut queries.outline,
                QueryFile::Indents => &mut queries.indents,
                QueryFile::Injections => &mut queries.injections,
                QueryFile::Overrides => &mut queries.overrides,
                QueryFile::Runnables => &mut queries.runnables,
                QueryFile::TextObjects => &mut queries.text_objects,
                QueryFile::SpellCheck => &mut queries.spell_check,
            };
            *field = Some(contents);
        }
        queries
    }
}
