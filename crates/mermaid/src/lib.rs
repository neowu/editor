//! Compiles Mermaid source to SVG drawn with a [`Theme`].
//!
//! The pipeline follows Pele (MIT): preprocessing removes front matter, directives, and comments,
//! detection picks a diagram type, and each type parses into a model, lays it out, and emits SVG.
//!
//! The SVG is meant to be rasterized, not shown in a browser: the document always has a fixed
//! size, and there are no links (`click`, `href`, and `link` statements parse but are ignored),
//! tooltips, or accessibility markup (`accTitle` and `accDescr` parse but are not written). Ids
//! that the drawing references internally (markers, gradients, clip paths) use a fixed prefix.
//! Image nodes keep relative, `http`, and `https` addresses; what loads is up to the rasterizer.
#![expect(dead_code, reason = "Shared helpers are ported ahead of the diagram types that use them.")]

mod detect;
mod error;
mod preprocess;
mod yaml;

mod layout {
    pub(crate) mod compound;
    pub(crate) mod layered;
}

mod svg {
    pub(crate) mod builder;
    pub(crate) mod cluster;
    pub(crate) mod edges;
    pub(crate) mod root;
    pub(crate) mod shapes;
    pub(crate) mod theme;
    pub(crate) mod title;
}

mod text {
    pub(crate) mod entities;
    pub(crate) mod icons;
    pub(crate) mod label;
    pub(crate) mod measure;
}

mod util {
    pub(crate) mod chars;
    pub(crate) mod float;
    pub(crate) mod unicode;
}

mod diagrams;

pub(crate) use error::Code;
pub use error::Error;
pub use svg::theme::Color;
pub use svg::theme::Theme;

/// What the host sets for a render. There are no font options, since the font comes from the
/// [`Theme`] and every width is monospace.
#[derive(Clone, Debug)]
pub struct Options {
    /// Font size in pixels.
    font_size: f64,
    /// The most UTF-16 units of source to accept. Mermaid's default `maxTextSize`.
    limit: usize,
    /// The most edges a flowchart may have. `A & B --> C & D` multiplies edges, so a few
    /// kilobytes of source can ask for millions.
    max_edges: usize,
    /// The most UTF-16 units of SVG to produce. A source within the limit can still describe
    /// megabytes of drawing, which would stall rasterizing.
    output_limit: usize,
    /// Space around a diagram. A treemap's `diagramPadding` config takes precedence.
    padding: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self { font_size: 16.0, limit: 50_000, max_edges: 5_000, output_limit: 4_000_000, padding: 8.0 }
    }
}

/// A rendered diagram.
#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    /// Height in pixels.
    height: f64,
    /// The SVG document, sized `width` by `height`.
    pub svg: String,
    /// The detected diagram type, using Pele's names.
    r#type: &'static str,
    /// Width in pixels.
    width: f64,
}

/// Renders Mermaid `source` to SVG drawn with `theme`.
///
/// # Errors
///
/// Returns an [`Error`] for unsupported diagram types, syntax errors, and exceeded limits.
pub fn render(source: &str, options: &Options, theme: &Theme) -> Result<Rendered, Error> {
    if exceeds(source, options.limit) {
        return Err(Error::new(
            Code::Limit,
            format!("Diagram source is longer than the limit of {} characters.", options.limit),
        ));
    }

    let pre = preprocess::preprocess(source)?;
    let Some(r#type) = detect::detect(&pre.text) else {
        return Err(Error::new(Code::UnsupportedDiagram, "No diagram type detected for the given text."));
    };
    let config = yaml::Value::Object(pre.config);

    let Some(diagram) = diagrams::find(r#type) else {
        let mut error = Error::new(Code::UnsupportedDiagram, format!("Diagram type \"{type}\" is not supported yet."));
        error.r#type = Some(r#type);
        return Err(error);
    };

    let mut source =
        preprocess::encode_entities(if diagram.keep_comments { &pre.with_comments } else { &pre.text }).into_owned();
    source.push('\n');

    let input = diagrams::Input {
        config: &config,
        line_offset: pre.line_offset,
        max_edges: options.max_edges,
        source: &source,
        title: pre.title.as_deref(),
    };

    let drawn = (diagram.render)(&input, options)?;
    let rendered =
        Rendered { height: drawn.height, svg: svg::theme::resolve(&drawn.svg, theme), r#type, width: drawn.width };

    if exceeds(&rendered.svg, options.output_limit) {
        let mut error = Error::new(
            Code::Limit,
            format!("Diagram output is longer than the limit of {} characters.", options.output_limit),
        );
        error.r#type = Some(r#type);
        return Err(error);
    }

    Ok(rendered)
}

/// Whether `text` is longer than `limit` UTF-16 units.
fn exceeds(text: &str, limit: usize) -> bool {
    // No scalar takes more UTF-16 units than UTF-8 bytes, so text within the limit in bytes is
    // within it in units without a count.
    if text.len() <= limit {
        return false;
    }

    // Each scalar starts with one non-continuation byte, and only four-byte scalars need a second
    // UTF-16 unit. Counting bytes avoids decoding the whole SVG.
    let units = text.bytes().map(|byte| usize::from(byte & 0xc0 != 0x80) + usize::from(byte >= 0xf0)).sum::<usize>();
    units > limit
}

#[cfg(test)]
mod tests {
    use super::Code;
    use super::Color;
    use super::Options;
    use super::Theme;
    use super::render;
    use crate::svg::builder::esc;
    use crate::svg::theme::resolve_style;

    #[test]
    fn output_has_no_browser_markup() {
        let theme = Theme::monochrome(Color::rgb(0xff, 0xff, 0xff), Color::rgb(0, 0, 0));
        let sources = [
            "flowchart LR\n  accTitle: Steps\n  accDescr: How it goes\n  A[Start] --> B[Done]\n  click A href \"https://example.com\" \"tip\"\n  click B call back()",
            "sequenceDiagram\n  accTitle: Greeting\n  Alice->>Bob: Hello\n  link Alice: Home @ https://example.com",
            "stateDiagram-v2\n  [*] --> Still\n  click Still \"https://example.com\" \"tip\"",
            "classDiagram\n  class Duck\n  link Duck \"https://example.com\" \"tip\"",
        ];
        for source in sources {
            let rendered = render(source, &Options::default(), &theme).unwrap();
            for needle in ["<a ", "aria-", "role=", "<title", "<desc", "max-width:100%"] {
                assert!(!rendered.svg.contains(needle), "{needle} in {}", rendered.svg);
            }
            assert!(rendered.svg.contains(" width=\""), "{}", rendered.svg);
        }
    }

    #[test]
    fn reads_links_tooltips_and_images_without_drawing_them() {
        let sources = [
            "flowchart LR\n  A --> B\n  click A callback \"tip\"\n  click B \"https://example.com\" \"tip\" _blank\n  click A href \"https://example.com\" _self",
            "classDiagram\n  class Duck\n  callback Duck \"cb\" \"tip\"\n  click Duck call cb() \"tip\"\n  click Duck href \"https://example.com\" \"tip\" _blank",
            "stateDiagram-v2\n  [*] --> S\n  click S href \"https://example.com\"",
            "sequenceDiagram\n  Alice->>Bob: Hi\n  links Alice: {\"Home\": \"https://example.com\"}\n  details Alice: x\n  properties Alice: {\"class\": \"c\"}",
            "flowchart LR\n  A@{ img: \"https://example.com/a.png\", label: \"Pic\" }",
        ];
        for source in sources {
            let out = svg(source);
            assert!(!out.contains("example.com") && !out.contains("<image"), "{out}");
        }
    }

    fn svg(source: &str) -> String {
        let theme = Theme::monochrome(Color::rgb(0xff, 0xff, 0xff), Color::rgb(0, 0, 0));
        render(source, &Options::default(), &theme).unwrap().svg
    }

    #[test]
    fn sequence_self_messages_keep_their_return_leg() {
        for right_angles in [false, true] {
            let out = svg(&format!(
                "---\nconfig:\n  sequence:\n    rightAngles: {right_angles}\n---\nsequenceDiagram\nA->>A: Self\nA->>B: Next"
            ));
            assert!(out.contains("Self"), "{out}");
            assert!(out.contains("Next"), "{out}");
            let own_path = out
                .split("d=\"")
                .skip(1)
                .map(|path| path.split('"').next().unwrap())
                .find(|path| path.starts_with('M') && path.matches('H').count() >= 2 && path.contains('V'));
            assert!(own_path.is_some(), "{out}");
        }
    }

    #[test]
    fn event_modeling_routes_edges() {
        let out = svg(
            "eventmodeling\ntimeframe 01 ui Cart\ntimeframe 02 command AddItem [[Add]]\ntimeframe 03 event ItemAdded [[Added]]",
        );
        assert!(out.contains("ondine-edge"), "{out}");
        assert!(out.contains("AddItem"), "{out}");
        assert!(out.contains("ItemAdded"), "{out}");
        assert!(!out.contains("NaN"), "{out}");
    }

    #[test]
    fn xy_line_reenters_the_plot_after_out_of_range_values() {
        let out = svg("xychart-beta\nx-axis [a, b, c, d, e]\ny-axis 0 --> 10\nline [5, 20, 5, -10, 5]");
        let line = out
            .split("d=\"")
            .skip(1)
            .map(|path| path.split('"').next().unwrap())
            .find(|path| path.matches('M').count() >= 3 && path.matches('L').count() >= 4);
        assert!(line.is_some(), "{out}");
        assert!(!out.contains("NaN"), "{out}");
    }

    #[test]
    fn decodes_entities_in_labels() {
        let out = svg("flowchart LR\n  A[\"GET /plans/&lcub;id&rcub;\"] --> B[\"items#lsqb;0#rsqb; #verbar; none\"]");
        assert!(out.contains("GET /plans/{id}"), "{out}");
        assert!(out.contains("items[0] | none"), "{out}");
        let out = svg("flowchart LR\n  A[\"x #amp;lt; y #38;gt; z\"]");
        assert!(out.contains("x &amp;lt; y &amp;gt; z"), "{out}");
    }

    #[test]
    fn styles_label_markup() {
        let out = svg("flowchart LR\n  A[\"H<sub>2</sub>O\"] --> B[\"x<sup>2</sup>\"]");
        assert!(out.contains("baseline-shift=\"sub\" font-size=\"smaller\">2</tspan>"), "{out}");
        assert!(out.contains("baseline-shift=\"super\" font-size=\"smaller\">2</tspan>"), "{out}");
        assert!(!out.contains("<sub>") && !out.contains("&lt;sub"), "{out}");

        let out = svg("flowchart LR\n  A[\"a <u>underlined</u> word\"]");
        assert!(out.contains("<tspan text-decoration=\"underline\">underlined</tspan>"), "{out}");

        let out = svg("flowchart LR\n  A[\"List<T>\"]");
        assert!(out.contains("List&lt;T&gt;"), "{out}");
        let out = svg("flowchart LR\n  A[\"<span title=x>red</span> and <small>print</small><img src=x>\"]");
        assert!(out.contains("red and print"), "{out}");
    }

    /// Every `name="number"` attribute value in `text`, in order.
    fn numbers(text: &str, name: &str) -> Vec<f64> {
        let needle = format!(" {name}=\"");
        text.match_indices(&needle)
            .filter_map(|(at, _)| {
                let rest = &text[at + needle.len()..];
                rest[..rest.find('"')?].parse().ok()
            })
            .collect()
    }

    #[test]
    fn sequence_starts_below_a_tall_self_message() {
        let out = svg("sequenceDiagram\n  A->>A: one<br/>two<br/>three\n  A->>B: next");
        let label = |id: &str| {
            let start = out.find(&format!("<g class=\"ondine-edge-label\" data-id=\"{id}\">")).unwrap();
            let end = start + out[start..].find("</g>").unwrap();
            numbers(&out[start..end], "y")
        };
        let own = label("i0");
        assert_eq!(own.len(), 3, "{out}");
        let step = label("i1")[0] - own[2];
        assert!(step >= own[1] - own[0], "{out}");
    }

    #[test]
    fn timeline_period_and_first_event_share_a_center() {
        let out = svg(
            "timeline TD\n  2002 : LinkedIn\n  2004 : Facebook : Google\n  2005<br>and on<br>and on : YouTube\n  2006 : first<br>second<br>third<br>fourth\n",
        );
        let rows: Vec<&str> = out.split("<g class=\"ondine-node ondine-period\"").skip(1).collect();
        assert_eq!(rows.len(), 4, "{out}");
        let mut above = 0.0;
        for row in rows {
            let rects: Vec<(f64, f64)> = row
                .split("<rect ")
                .skip(1)
                .map(|rect| {
                    let rect = &rect[..rect.find('>').unwrap()];
                    (numbers(rect, "y")[0], numbers(rect, "height")[0])
                })
                .collect();
            let middle = |(y, h): (f64, f64)| y + h / 2.0;
            assert!((middle(rects[0]) - middle(rects[1])).abs() < 0.01, "{row}");
            assert!(rects.iter().all(|&(y, _)| y > above), "{row}");
            above = rects.iter().fold(0.0, |most: f64, &(y, h)| most.max(y + h));
        }
    }

    /// The `translate(x,y)` of the element with `data-id="id"`.
    fn translate(svg: &str, id: &str) -> (f64, f64) {
        let at = svg.find(&format!("data-id=\"{id}\" transform=\"translate(")).unwrap();
        let rest = &svg[at..];
        let rest = &rest[rest.find('(').unwrap() + 1..rest.find(')').unwrap()];
        let (x, y) = rest.split_once(',').unwrap();
        (x.parse().unwrap(), y.parse().unwrap())
    }

    #[test]
    fn lines_up_unlinked_subgraph_members_with_the_member_an_outside_edge_enters() {
        let out = svg("flowchart TB\n  subgraph S\n    a\n    b\n    c\n  end\n  x --> a");
        let y = |id| translate(&out, id).1;
        assert_eq!(y("b"), y("a"), "{out}");
        assert_eq!(y("c"), y("a"), "{out}");
    }

    #[test]
    fn ignores_a_subgraph_direction_when_its_nodes_link_outside_it() {
        let groups = "flowchart LR\n  subgraph S\n    direction TB\n    s1\n    s2\n  end\n  subgraph P\n    direction TB\n    p1\n    p2\n  end\n";
        let edges = "  s1 --> p2\n  s2 --> p1\n";
        assert_eq!(
            svg(&format!("{groups}{edges}")),
            svg(&format!("{}{edges}", groups.replace("    direction TB\n", "")))
        );

        let out = svg(
            "flowchart LR\n  subgraph one\n    direction TB\n    a1 --> a2\n  end\n  subgraph two\n    direction TB\n    b1 --> b2\n  end\n  subgraph outer\n    direction TB\n    subgraph inner\n      direction TB\n      c1 --> c2\n    end\n    c3\n  end\n  x --> one\n  x --> b1\n  c3 --> x",
        );
        let at = |id| translate(&out, id);
        assert_eq!(at("a1").0, at("a2").0, "{out}");
        assert_eq!(at("b1").1, at("b2").1, "{out}");
        assert_eq!(at("c1").0, at("c2").0, "{out}");
    }

    fn fails(source: &str) -> bool {
        render(source, &Options::default(), &Theme::default()).is_err()
    }

    #[test]
    fn treats_a_bare_title_as_no_title() {
        let out = svg("C4Context\ntitle\nPerson(a, \"Alice\")\nPerson(b, \"Bob\")");
        assert!(out.contains("Alice") && out.contains("Bob"), "{out}");
        assert!(svg("C4Context\ntitle\tOwn\nSystem(a, \"A\")").contains("Own"));

        let out = svg("timeline\ntitle\n2024 : Event\n2025 : Next");
        assert!(!out.contains("2024 : Event") && out.contains("2024") && out.contains("2025"), "{out}");
        assert!(svg("timeline\n  title\u{a0}Kept\n  2024 : Event").contains("Kept"));
    }

    #[test]
    fn keeps_the_character_before_a_comment() {
        let out = svg("timeline\nA%% label : event\nB : e2");
        assert!(out.contains("A%% label"), "{out}");
        assert!(fails("timeline\n  2024 :%% c\n"));

        assert!(!fails("xychart-beta\nx-axis [a, b]\nbar [1,2]%% note"));
        assert!(fails("xychart-beta\ntitle Sales:%% note\nbar [1,2]"));
    }

    #[test]
    fn reads_names_that_start_with_title_or_an_option_value() {
        let out = svg(
            "architecture-beta\ngroup titles(cloud)[G]\nservice title-db(database)[DB] in titles\nservice title_2 in titles\ntitle-db:R -- L:title_2",
        );
        assert!(out.contains("DB") && out.contains("title_2"), "{out}");
        assert!(!fails(
            "architecture-beta\ngroup titles(cloud)[G]\nservice title-x(database)[DB] in titles\njunction title1 in titles\ntitle-x:L -- R:title1\nalign row title-x title1\n"
        ));

        let out = svg(
            "radar-beta\n axis circles, polygonal, trueNorth, falsey, titles\n curve titles{1, 2, 3, 4, 5}, circles{ circles: 1, polygonal 2, trueNorth: 3, falsey 4, titles: 5 }",
        );
        for name in ["circles", "polygonal", "trueNorth", "falsey", "titles"] {
            assert!(out.contains(name), "{name}: {out}");
        }
        assert!(!fails("radar-beta\n axis a, b\n showLegend falsecurve x{1, 2}\n graticule polygonaxis c"));
        assert!(fails("radar-beta\n axis circle, b"));

        let out = svg("eventmodeling\ntf 01 ui titles\ntf 02 cmd title_x\nentity titles\n");
        assert!(out.contains("titles") && out.contains("title_x"), "{out}");

        for src in [
            "railroad-beta\ntitles = nonterminal(\"title-x\") ;\ntitle-x = terminal(\"a\") ;",
            "railroad-ebnf-beta\ntitles = title-x ;\ntitle-x = \"a\" ;",
            "railroad-abnf-beta\ntitles = title-x ;\ntitle-x = \"a\" ;",
            "railroad-peg-beta\ntitles <- title-x ;\ntitle-x <- \"a\" ;",
        ] {
            assert!(svg(src).contains("data-id=\"title-x\""), "{src}");
        }
    }

    #[test]
    fn drops_invalid_xml_characters_and_rejects_styles_with_them() {
        assert_eq!(esc("a\u{fffe}b\u{ffff}<"), "ab&lt;");
        assert_eq!(resolve_style(&["fill:u\u{1}rl(//evil.example/x)"]).shape, "");
    }

    #[test]
    fn falls_back_to_the_data_range_when_set_y_bounds_clamp_together() {
        let big = format!("2{}", "0".repeat(301));
        let out = svg(&format!("xychart-beta\ny-axis {big} --> 3{}\nbar [1, 2]", &big[1..]));
        assert!(!out.contains("NaN"), "{out}");
    }

    #[test]
    fn detects_c4_only_at_the_start() {
        let theme = Theme::default();
        for keyword in ["C4Context", "C4Container", "C4Component", "C4Dynamic", "C4Deployment"] {
            let flow =
                render(&format!("flowchart LR\n  a[Web app] --> b[{keyword} view]"), &Options::default(), &theme);
            assert_eq!(flow.unwrap().r#type, "flowchart");
            let sequence =
                render(&format!("sequenceDiagram\n  Alice->>Bob: {keyword} view"), &Options::default(), &theme);
            assert_eq!(sequence.unwrap().r#type, "sequence");
        }
    }

    #[test]
    fn c4_fits_a_relation_label_between_columns() {
        let out = svg(
            "C4Context\nSystem(a, \"A\")\nSystem(b, \"B\")\nRel(a, b, \"Sends a fairly long description of the relationship\")",
        );
        let (ax, _) = translate(&out, "a");
        let (bx, _) = translate(&out, "b");
        let start = out.find("<g class=\"ondine-edge-label\" data-id=\"a-b\"><rect").unwrap();
        let rect = &out[start..start + out[start..].find("/>").unwrap()];
        let (x, w) = (numbers(rect, "x")[0], numbers(rect, "width")[0]);
        assert!(x >= ax + 108.0 && x + w <= bx - 108.0, "{out}");
    }

    #[test]
    fn state_follows_the_flowchart_curve() {
        let body = "stateDiagram-v2\n direction TB\n state A {\n a --> b\n a --> c\n }\n out --> a\n A --> out";
        let smooth = svg(body);
        let paths = |out: &str| -> Vec<String> {
            out.split("<g class=\"ondine-edge ondine-transition\"")
                .skip(1)
                .map(|edge| {
                    let at = edge.find(" d=\"").unwrap() + 4;
                    edge[at..at + edge[at..].find('"').unwrap()].to_owned()
                })
                .collect()
        };
        assert!(paths(&smooth).iter().any(|d| d.contains('C')), "{smooth}");
        for curve in ["linear", "step", "stepBefore", "stepAfter"] {
            let out = svg(&format!("---\nconfig:\n  flowchart:\n    curve: {curve}\n---\n{body}"));
            assert_ne!(out, smooth, "{curve}");
            let paths = paths(&out);
            assert_eq!(paths.len(), 4, "{out}");
            assert!(paths.iter().all(|d| !d.contains('C')), "{curve}: {paths:?}");
        }
        let ignored = svg(&format!("---\nconfig:\n  state:\n    curve: step\n---\n{body}"));
        assert_eq!(ignored, smooth);
    }

    #[test]
    fn limits_reject_long_source() {
        let theme = Theme::monochrome(Color::rgb(0xff, 0xff, 0xff), Color::rgb(0, 0, 0));
        let options = Options { limit: 10, ..Options::default() };
        let error = render("flowchart LR\n  A --> B", &options, &theme).unwrap_err();
        assert_eq!(error.code, Code::Limit);
    }
}
