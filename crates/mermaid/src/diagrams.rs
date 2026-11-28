//! Diagram types and the contract each one implements.

mod common {
    pub(crate) mod acc_descr;
    pub(crate) mod fit;
    pub(crate) mod fit_text;
    pub(crate) mod generics;
    mod literal_label;
    pub(crate) mod outline;
    pub(crate) mod scan;
    pub(crate) mod tokens;
}

mod architecture;
mod c4;
mod class;
mod er;
mod eventmodeling;
mod flowchart;
mod mindmap;
mod radar;
mod railroad;
mod requirement;
mod sequence;
mod state;
mod timeline;
mod treemap;
mod usecase;
mod xychart;

use crate::Options;
use crate::error::Error;
use crate::yaml::Value;

/// What a diagram type receives, as Pele's `Diagram.parse` does.
pub(crate) struct Input<'a> {
    /// The diagram configuration from front matter and directives, as a YAML mapping.
    pub(crate) config: &'a Value,
    /// Front matter lines removed before `source`, for types that report source positions.
    pub(crate) line_offset: u32,
    /// The most edges a flowchart-like diagram may have.
    pub(crate) max_edges: usize,
    /// Preprocessed, entity-encoded source ending in a newline. It keeps `%%` comment lines
    /// when the type sets `keep_comments`.
    pub(crate) source: &'a str,
    /// The front matter title.
    pub(crate) title: Option<&'a str>,
}

/// A drawn diagram.
#[derive(Clone, Debug)]
pub(crate) struct Rendered {
    /// Height in pixels.
    pub(crate) height: f64,
    /// The SVG document.
    pub(crate) svg: String,
    /// Width in pixels.
    pub(crate) width: f64,
}

/// A diagram type, as Pele's `Diagram` describes one.
#[derive(Clone, Copy)]
pub(crate) struct Diagram {
    /// Whether the type parses source with its `%%` comment lines kept.
    pub(crate) keep_comments: bool,
    /// Parses and draws the diagram.
    pub(crate) render: fn(&Input<'_>, &Options) -> Result<Rendered, Error>,
}

/// The diagram type named `type` (Pele's names), when this build includes it.
pub(crate) fn find(r#type: &str) -> Option<Diagram> {
    match r#type {
        "architecture" => Some(architecture::DIAGRAM),
        "c4" => Some(c4::DIAGRAM),
        "class" => Some(class::DIAGRAM),
        "er" => Some(er::DIAGRAM),
        "eventmodeling" => Some(eventmodeling::DIAGRAM),
        "flowchart" => Some(flowchart::DIAGRAM),
        "mindmap" => Some(mindmap::DIAGRAM),
        "radar" => Some(radar::DIAGRAM),
        "railroad" => Some(railroad::DIAGRAM),
        "requirement" => Some(requirement::DIAGRAM),
        "sequence" => Some(sequence::DIAGRAM),
        "state" => Some(state::DIAGRAM),
        "timeline" => Some(timeline::DIAGRAM),
        "treemap" => Some(treemap::DIAGRAM),
        "usecase" => Some(usecase::DIAGRAM),
        "xychart" => Some(xychart::DIAGRAM),
        _ => None,
    }
}
