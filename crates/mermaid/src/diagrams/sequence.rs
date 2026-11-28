//! Sequence diagrams: participants on lifelines, the messages between them, notes, and blocks.
//! Port of Pele's `src/diagrams/sequence/index.ts`.

mod actors;
mod colors;
mod db;
mod lexer;
mod parser;
mod render;
mod tokens;
mod types;

use super::Diagram;
use super::Input;
use super::Rendered;
use crate::Options;
use crate::error::Error;
use crate::yaml::Value;

/// The `sequence` diagram type.
pub(crate) const DIAGRAM: Diagram = Diagram { keep_comments: false, render: render_input };

fn render_input(input: &Input<'_>, options: &Options) -> Result<Rendered, Error> {
    let db = parse(input.source, input.config, input.title)?;
    Ok(render::render(&db, input.config, options))
}

/// The parsed sequence diagram model: Pele's `SeqDb`.
pub(crate) use db::SeqDb as Model;

/// Parses preprocessed sequence diagram `source`, as Pele's `sequence.parse` does. `config` is
/// the diagram configuration, of which `wrap` and `sequence.wrap` are read. `title` comes from
/// front matter.
///
/// # Errors
///
/// Returns a syntax error for input Mermaid rejects or a participant config that is not valid
/// YAML, and a semantic error for a participant in two boxes, a misplaced `create` or `destroy`,
/// and a deactivation of an inactive participant.
fn parse(source: &str, config: &Value, title: Option<&str>) -> Result<Model, Error> {
    let wrap = match member(config, "wrap") {
        Some(Value::Bool(wrap)) => Some(*wrap),
        _ => None,
    };
    let sequence_wrap = member(config, "sequence")
        .and_then(|section| member(section, "wrap"))
        .is_some_and(|wrap| *wrap == Value::Bool(true));
    let mut db = Model::new(wrap, sequence_wrap);
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        db.set_diagram_title(title);
    }
    parser::parse_sequence(source, &mut db)?;
    Ok(db)
}

/// The value under `key` of a YAML mapping.
fn member<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    match value {
        Value::Object(entries) => entries.iter().find(|(own, _)| own == key).map(|(_, v)| v),
        // lint: expect(wildcard_match) only a few variants of `Value` matter here
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagrams::sequence::types::MessageText;

    fn model(src: &str) -> Result<Model, Error> {
        parse(src, &Value::Null, None)
    }

    fn texts(m: &Model) -> Vec<&str> {
        m.messages
            .iter()
            .filter_map(|x| match &x.message {
                MessageText::Text(t) => Some(t.as_str()),
                MessageText::Numbering(_) => None,
            })
            .collect()
    }

    #[test]
    fn bare_title_is_no_title() {
        for src in ["sequenceDiagram\ntitle\nA->>B: Hi\nB->>A: Bye", "sequenceDiagram\ntitle: \nA->>B: Hi\nB->>A: Bye"]
        {
            let m = model(src).unwrap();
            assert_eq!(m.title, None, "{src}");
            assert_eq!(texts(&m), ["Hi", "Bye"], "{src}");
        }
        assert_eq!(model("sequenceDiagram\n  title My title\n  A->>B: x").unwrap().title.as_deref(), Some("My title"));
    }

    #[test]
    fn keeps_the_character_before_a_comment() {
        assert_eq!(texts(&model("sequenceDiagram\n  A->>B:%% comment").unwrap()), ["%% comment"]);
        model("sequenceDiagram\n  autonumber 5%% comment").unwrap_err();
    }
}
