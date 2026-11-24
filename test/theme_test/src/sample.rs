//! The hardcoded editor state the preview paints: code tagged by hand with the capture names a
//! grammar would produce, and the fills an editor session would put behind it.

/// What is painted behind a span of code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fill {
    None,
    Selection,
    /// An occurrence of the symbol under the cursor, or the matching bracket.
    Symbol,
    Search,
    ActiveSearch,
    WordAdded,
    WordDeleted,
}

/// What is painted behind a whole line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Row {
    Plain,
    /// The line the cursor is on.
    Active,
    /// A go-to-line style flash.
    Highlighted,
    Added,
    Deleted,
}

/// How a span gets its text colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ink {
    /// A syntax capture name; empty is the editor foreground.
    Syntax(&'static str),
    /// Unused code: the capture, faded.
    Unnecessary(&'static str),
    Error,
    Warning,
}

pub(crate) struct Span {
    pub(crate) text: &'static str,
    pub(crate) ink: Ink,
    pub(crate) fill: Fill,
}

const fn s(text: &'static str, capture: &'static str) -> Span {
    Span { text, ink: Ink::Syntax(capture), fill: Fill::None }
}

const fn on(fill: Fill, text: &'static str, capture: &'static str) -> Span {
    Span { text, ink: Ink::Syntax(capture), fill }
}

const fn ink(text: &'static str, ink: Ink) -> Span {
    Span { text, ink, fill: Fill::None }
}

pub(crate) const CODE: &[(Row, &[Span])] = &[
    (Row::Plain, &[s("// Fades a colour towards the card, for surfaces that stay opaque.", "comment")]),
    (Row::Plain, &[s("/// `top` at `percent` opacity over `bottom`.", "comment.doc")]),
    (Row::Plain, &[s("#[derive(Debug, Clone)]", "attribute")]),
    (
        Row::Plain,
        &[
            s("pub struct ", "keyword"),
            s("Palette", "type"),
            s("<", "punctuation.bracket"),
            s("'a", "lifetime"),
            s("> {", "punctuation.bracket"),
        ],
    ),
    (
        Row::Plain,
        &[
            s("    name", "property"),
            s(": ", "punctuation.delimiter"),
            s("&", "operator"),
            s("'a ", "lifetime"),
            s("str", "type.builtin"),
            s(",", "punctuation.delimiter"),
        ],
    ),
    (Row::Plain, &[s("}", "punctuation.bracket")]),
    (Row::Plain, &[]),
    (
        Row::Plain,
        &[
            s("fn ", "keyword"),
            s("mix", "definition.function"),
            s("(", "punctuation.bracket"),
            on(Fill::Symbol, "top", "variable.parameter"),
            s(": ", "punctuation.delimiter"),
            s("u32", "type.builtin"),
            s(", ", "punctuation.delimiter"),
            s("bottom", "variable.parameter"),
            s(": ", "punctuation.delimiter"),
            s("u32", "type.builtin"),
            s(") ", "punctuation.bracket"),
            s("-> ", "operator"),
            s("u32 ", "type.builtin"),
            on(Fill::Symbol, "{", "punctuation.bracket"),
        ],
    ),
    (
        Row::Active,
        &[
            s("    let ", "keyword"),
            s("ratio", "variable"),
            s(": f32", "hint"),
            s(" = ", "operator"),
            s("0.65", "number"),
            s(" * ", "operator"),
            on(Fill::Symbol, "top", "variable"),
            s(" as ", "keyword"),
            s("f32", "type.builtin"),
            s(";", "punctuation.delimiter"),
            s("  predicted_call()", "predictive"),
        ],
    ),
    (
        Row::Plain,
        &[
            s("    let ", "keyword"),
            s("label", "variable"),
            s(" = ", "operator"),
            on(Fill::Selection, "format!", "function"),
            on(Fill::Selection, "(", "punctuation.bracket"),
            on(Fill::Selection, "\"card {}", "string"),
            on(Fill::Selection, "\\n", "string.escape"),
            s("\"", "string"),
            s(", ", "punctuation.delimiter"),
            s("Shade", "type"),
            s("::", "punctuation.delimiter"),
            s("Dark", "variant"),
            s("(", "punctuation.bracket"),
            s("true", "boolean"),
            s("));", "punctuation.bracket"),
        ],
    ),
    (
        Row::Plain,
        &[
            s("    let ", "keyword"),
            ink("unused", Ink::Unnecessary("variable")),
            s(" = ", "operator"),
            s("bottom", "variable"),
            s(".", "punctuation.delimiter"),
            s("count_ones", "function.method"),
            s("();", "punctuation.bracket"),
            ink("    unused variable: `unused`", Ink::Warning),
        ],
    ),
    (
        Row::Plain,
        &[
            s("    ", ""),
            on(Fill::Search, "search", "variable"),
            s("(", "punctuation.bracket"),
            on(Fill::ActiveSearch, "\"search\"", "string"),
            s(", ", "punctuation.delimiter"),
            on(Fill::Search, "SEARCH", "constant"),
            s(", ", "punctuation.delimiter"),
            on(Fill::Search, "// search", "comment"),
            s(", ", "punctuation.delimiter"),
            on(Fill::ActiveSearch, "if", "keyword"),
            s(" ", ""),
            on(Fill::ActiveSearch, "1", "number"),
            s(");", "punctuation.bracket"),
        ],
    ),
    (
        Row::Deleted,
        &[
            s("    ", ""),
            s("return ", "keyword"),
            s("shade", "function"),
            s("(", "punctuation.bracket"),
            s("top", "variable"),
            s(", ", "punctuation.delimiter"),
            on(Fill::WordDeleted, "-30", "number"),
            s(");", "punctuation.bracket"),
        ],
    ),
    (
        Row::Added,
        &[
            s("    ", ""),
            s("return ", "keyword"),
            s("shade", "function"),
            s("(", "punctuation.bracket"),
            s("top", "variable"),
            s(", ", "punctuation.delimiter"),
            on(Fill::WordAdded, "ratio as i32", "variable"),
            s(");", "punctuation.bracket"),
        ],
    ),
    (
        Row::Highlighted,
        &[
            s("    ", ""),
            s("missing", "function"),
            s("(", "punctuation.bracket"),
            s("label", "variable"),
            s(");", "punctuation.bracket"),
            ink("    cannot find function `missing`", Ink::Error),
        ],
    ),
    (Row::Plain, &[on(Fill::Symbol, "}", "punctuation.bracket")]),
];

#[cfg(test)]
mod tests {
    use theme::Appearance;
    use theme::built_in::theme_for;

    use super::*;

    #[test]
    fn sample_shows_every_fill_and_row() {
        for fill in
            [Fill::Selection, Fill::Symbol, Fill::Search, Fill::ActiveSearch, Fill::WordAdded, Fill::WordDeleted]
        {
            assert!(CODE.iter().any(|(_, spans)| spans.iter().any(|span| span.fill == fill)), "{fill:?}");
        }
        for row in [Row::Plain, Row::Active, Row::Highlighted, Row::Added, Row::Deleted] {
            assert!(CODE.iter().any(|(kind, _)| *kind == row), "{row:?}");
        }
    }

    #[test]
    fn sample_captures_resolve_in_the_theme() {
        let syntax = theme_for(Appearance::Dark).syntax();
        for (_, spans) in CODE {
            for span in *spans {
                if let Ink::Syntax(capture) | Ink::Unnecessary(capture) = span.ink
                    && !capture.is_empty()
                {
                    assert!(syntax.style_for_name(capture).is_some(), "{capture}");
                }
            }
        }
    }
}
