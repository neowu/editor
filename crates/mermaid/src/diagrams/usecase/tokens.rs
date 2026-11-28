//! Token types of Mermaid's use case lexer. Port of Pele's `src/diagrams/usecase/tokens.ts`.

/// Mermaid's token types. The order is observable: it makes the token categories ranges, so WORD
/// is `Usecase` to `Identifier`, and `LABEL_TEXT` is `Usecase` to `LabelSymbol`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum T {
    /// End of input.
    Eof,
    /// `usecase-beta`.
    Usecase,
    /// `actor`.
    Actor,
    /// `systemBoundary`.
    SystemBoundary,
    /// `end`.
    End,
    /// `direction`.
    Direction,
    /// `TD`.
    Td,
    /// `TB`.
    Tb,
    /// `BT`.
    Bt,
    /// `LR`.
    Lr,
    /// `RL`.
    Rl,
    /// `note`.
    Note,
    /// `for`.
    For,
    /// `json`.
    Json,
    /// `classDef`.
    ClassDef,
    /// `class`.
    Class,
    /// `style`.
    Style,
    /// `include`, in any case.
    Include,
    /// `extend`, in any case.
    Extend,
    /// `true`.
    True,
    /// `false`.
    False,
    /// A word.
    Identifier,
    /// A decimal number with a fraction and an optional unit.
    Number,
    /// `@`.
    At,
    /// `,`.
    Comma,
    /// `:`.
    Colon,
    /// `#` and hexadecimal digits.
    HashColor,
    /// A hyphenated word.
    CssIdentifier,
    /// `\,`.
    CssEscapedComma,
    /// `-`.
    Dash,
    /// `.`.
    Dot,
    /// `%`.
    Percent,
    /// Punctuation of a style value.
    CssPunctuation,
    /// Other punctuation.
    LabelPunctuation,
    /// A run of other characters.
    LabelSymbol,
    /// A quoted string.
    PlainString,
    /// A markdown string.
    MarkdownString,
    /// A markdown string that is never closed.
    UnclosedMarkdownString,
    /// A `%%` comment.
    Comment,
    /// A line break.
    Newline,
    /// `accDescr { ... }`.
    AccDescrBlock,
    /// `accTitle: ...`.
    AccTitleLine,
    /// `accDescr: ...`.
    AccDescrLine,
    /// `json id@`.
    JsonDeclarationStart,
    /// A JSON object.
    JsonObjectLiteral,
    /// A JSON object that is never closed.
    UnclosedJsonObjectLiteral,
    /// `--|>`.
    Generalization,
    /// `..>`.
    DependencyArrow,
    /// `<<`.
    StereotypeStart,
    /// `>>`.
    StereotypeEnd,
    /// The text of a stereotype.
    StereotypeText,
    /// The text of a stereotype that is never closed.
    UnclosedStereotypeText,
    /// `:::`.
    ClassSeparator,
    /// `-->`.
    ForwardSolid,
    /// `<--`.
    BackwardSolid,
    /// `--o`.
    ForwardCircle,
    /// `o--`.
    BackwardCircle,
    /// `--x`.
    ForwardCross,
    /// `x--`.
    BackwardCross,
    /// `--`.
    MarkerlessSolid,
    /// `@{`.
    MetadataStart,
    /// `{`.
    LBrace,
    /// `}`.
    RBrace,
    /// `[`.
    LBracket,
    /// `]`.
    RBracket,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
}
