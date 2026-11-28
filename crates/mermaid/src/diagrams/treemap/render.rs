//! Draws a treemap: sections with headers and leaves with names and values. Port of Pele's
//! `src/diagrams/treemap/render.ts`.

use super::format::format;
use super::format::formatter;
use super::layout::Insets;
use super::layout::layout_treemap;
use super::model::TreemapModel;
use super::model::TreemapNode;
use crate::Options;
use crate::diagrams::Rendered;
use crate::diagrams::common::fit_text::fit_text;
use crate::svg::builder::esc;
use crate::svg::builder::esc_text;
use crate::svg::builder::label_svg;
use crate::svg::builder::push_num;
use crate::svg::root::svg_document;
use crate::svg::theme::RADIUS;
use crate::svg::theme::class_names;
use crate::svg::theme::resolve_style;
use crate::svg::theme::series_color;
use crate::text::entities::decode;
use crate::text::label::layout_label;
use crate::text::label::trim;
use crate::text::measure;
use crate::text::measure::BOLD;
use crate::text::measure::ITALIC;
use crate::util::float::js_round;
use crate::yaml::Value;
use crate::yaml::push_number;

/// The height of a section header.
const HEADER: f64 = 28.0;
/// The header of a section too short for [`HEADER`].
const SHORT_HEADER: f64 = 18.0;
/// The space a section keeps around its text.
const SIDE: f64 = 6.0;
/// Line height of leaf text, as a multiple of its size.
const LINE: f64 = 1.25;

/// The number of leaves with a positive value under `roots`.
fn leaf_count(roots: &[TreemapNode]) -> usize {
    let mut count = 0;
    let mut stack: Vec<&TreemapNode> = roots.iter().collect();
    while let Some(node) = stack.pop() {
        match (&node.children, node.value) {
            (Some(children), _) => stack.extend(children),
            (None, Some(value)) if value.is_finite() && value > 0.0 => count += 1,
            _ => {}
        }
    }
    count
}

/// A property of a YAML mapping, as JavaScript's `value.key` reads it.
fn property<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    let Value::Object(entries) = value else {
        return None;
    };
    entries.iter().find(|(own, _)| own == key).map(|(_, value)| value)
}

/// The `treemap` option `key` of `config` when it is a number at least `min`, capped at `max`, or
/// `fallback`.
fn option(config: &Value, key: &str, fallback: f64, min: f64, max: f64) -> f64 {
    match property(config, "treemap").and_then(|treemap| property(treemap, key)) {
        Some(Value::Number(value)) if *value >= min => value.min(max),
        _ => fallback,
    }
}

/// Draws `model` with the `treemap` section of `config`.
pub(crate) fn render(model: &TreemapModel, config: &Value, options: &Options) -> Rendered {
    let size = options.font_size;
    let small = js_round(size * 0.875);
    let tiny = js_round(size * 0.75);
    let least = js_round(size * 0.5625);
    let treemap = property(config, "treemap");
    let pad = option(config, "diagramPadding", options.padding, 0.0, 2000.0);

    // Mermaid sizes the diagram as ten times `nodeWidth` by ten times `nodeHeight`.
    let width = js_round(option(config, "nodeWidth", 64.0, 1.0, 400.0) * 10.0);
    let height = js_round(option(config, "nodeHeight", 40.0, 1.0, 400.0) * 10.0);
    let show_values = !matches!(treemap.and_then(|treemap| property(treemap, "showValues")), Some(Value::Bool(false)));
    let spec = match treemap.and_then(|treemap| property(treemap, "valueFormat")) {
        Some(Value::String(spec)) => formatter(spec),
        _ => formatter(","),
    };
    // Gaps and insets shrink as the average leaf area does.
    #[expect(clippy::cast_precision_loss, reason = "a leaf count")]
    let leaves = leaf_count(&model.roots).max(1) as f64;
    let crowd = ((width * height) / leaves / 800.0).log10().clamp(0.0, 1.0);
    let (boxes, placed) = layout_treemap(
        &model.roots,
        width,
        height,
        Insets {
            gap: option(config, "padding", js_round(2.0 + 2.0 * crowd), 0.0, 200.0),
            header: HEADER,
            short: SHORT_HEADER,
            side: js_round(3.0 + 3.0 * crowd),
        },
    );

    let title = layout_label(model.title.as_deref(), false, size, 4000.0, BOLD, None);
    let title_height = if title.height > 0.0 { title.height + 8.0 } else { 0.0 };
    let left = pad + 0.0_f64.max((title.width - width) / 2.0);
    let top = pad + title_height;

    let mut sections = String::new();
    let mut leaves = String::new();
    for b in placed.iter().filter_map(|&k| boxes.get(k)) {
        let node = b.node;
        let style = resolve_style(node.css_compiled_styles.as_deref().unwrap_or(&[]));
        let color = series_color(i64::try_from(b.series).unwrap_or(0));
        let x = left + b.x;
        let y = top + b.y;
        let decoded = decode(&node.name);
        let name = trim(&decoded);
        let value = if show_values { format(&spec, b.value) } else { String::new() };
        let classes = class_names(node.class_selector.as_deref().unwrap_or(""));
        // A class's font size overrides the size attributes, so text is cut instead of shrunk.
        let fixed = style.font_size;
        let text_style = if style.bold { BOLD } else { 0 } | if style.italic { ITALIC } else { 0 };

        let mut rect = String::from("data-id=\"");
        rect.push_str(&esc_text(&node.name));
        rect.push_str("\"><rect x=\"");
        push_num(&mut rect, x);
        rect.push_str("\" y=\"");
        push_num(&mut rect, y);
        rect.push_str("\" width=\"");
        push_num(&mut rect, b.w);
        rect.push_str("\" height=\"");
        push_num(&mut rect, b.h);
        rect.push_str("\" rx=\"");
        rect.push_str(RADIUS);
        rect.push_str("\" fill=\"");
        rect.push_str(&color);
        rect.push('"');

        if node.children.is_some() {
            // A section without a header draws nothing, so its children blend into the grid.
            if b.header == 0.0 {
                continue;
            }
            let mut text = String::new();
            let short = b.header < HEADER;
            let label_size = fixed.unwrap_or(if short { tiny } else { small });
            let value_size = fixed.unwrap_or(tiny);
            let room = b.w - 2.0 * SIDE - 4.0;
            let value_width = if value.is_empty() { 0.0 } else { measure::width(&value, value_size, text_style) };
            let with_value = !short && value_width > 0.0 && room - value_width - 12.0 >= 3.0 * label_size;
            let label = if label_size * LINE < b.header {
                let room = if with_value { room - value_width - 12.0 } else { room };
                fit_text(name, room, label_size, label_size, text_style | BOLD)
            } else {
                None
            };
            let baseline = y + b.header / 2.0 + 1.0;
            if let Some(label) = label {
                text.push_str("<text x=\"");
                push_num(&mut text, x + SIDE + 2.0);
                text.push_str("\" y=\"");
                push_num(&mut text, baseline + label_size * 0.35);
                text.push_str("\" font-weight=\"var(--_hw)\"");
                if short {
                    text.push_str(" font-size=\"");
                    push_number(&mut text, label_size);
                    text.push('"');
                }
                text.push_str(&style.text);
                text.push('>');
                text.push_str(&esc(&label.text));
                text.push_str("</text>");
            }
            if with_value && value_size * LINE <= HEADER {
                text.push_str("<text class=\"ondine-value\" x=\"");
                push_num(&mut text, x + b.w - SIDE - 2.0);
                text.push_str("\" y=\"");
                push_num(&mut text, baseline + value_size * 0.35);
                text.push_str("\" font-size=\"");
                push_number(&mut text, tiny);
                text.push_str("\" text-anchor=\"end\" fill=\"var(--_m)\"");
                text.push_str(&style.text);
                text.push('>');
                text.push_str(&esc(&value));
                text.push_str("</text>");
            }
            sections.push_str("<g class=\"ondine-node ondine-section");
            sections.push_str(&classes);
            sections.push_str("\" ");
            // A section is a tint of its color, unless a class gives it a fill of its own.
            let own_fill = style.shape.contains("\"fill:") || style.shape.contains(";fill:");
            sections.push_str(&rect);
            if !own_fill {
                sections.push_str(" fill-opacity=\"0.12\"");
            }
            sections.push_str(" stroke=\"");
            sections.push_str(&color);
            sections.push('"');
            sections.push_str(&style.shape);
            sections.push_str("/>");
            sections.push_str(&text);
            sections.push_str("</g>");
            continue;
        }

        let mut text = String::new();
        let room = b.w - 8.0;
        let smallest = fixed.unwrap_or(least);
        let tallest = match fixed {
            None => size.min(((b.h - 4.0) / LINE).floor()),
            Some(fixed) if fixed * LINE <= b.h - 4.0 => fixed,
            Some(_) => 0.0,
        };
        let label = if tallest >= smallest { fit_text(name, room, tallest, smallest, text_style) } else { None };
        if let Some(label) = label {
            let mut cx = String::new();
            push_num(&mut cx, x + b.w / 2.0);
            let cy = y + b.h / 2.0;
            let value_size = fixed.unwrap_or_else(|| least.max(js_round(label.size * 0.8)));
            let lines = (label.size + value_size) * LINE;
            let push_text = |text: &mut String, class: &str, y: f64, size: f64, body: &str| {
                text.push_str("<text");
                text.push_str(class);
                text.push_str(" x=\"");
                text.push_str(&cx);
                text.push_str("\" y=\"");
                push_num(text, y + size * 0.35);
                text.push_str("\" font-size=\"");
                push_num(text, size);
                text.push('"');
                text.push_str(&style.text);
                text.push('>');
                text.push_str(&esc(body));
                text.push_str("</text>");
            };
            if !value.is_empty() && lines <= b.h - 4.0 && measure::width(&value, value_size, text_style) <= room {
                let first = cy - lines / 2.0 + (label.size * LINE) / 2.0;
                let second = cy + lines / 2.0 - (value_size * LINE) / 2.0;
                push_text(&mut text, "", first, label.size, &label.text);
                push_text(&mut text, " class=\"ondine-value\"", second, value_size, &value);
            } else {
                push_text(&mut text, "", cy, label.size, &label.text);
            }
        }
        leaves.push_str("<g class=\"ondine-node ondine-leaf");
        leaves.push_str(&classes);
        leaves.push_str("\" ");
        leaves.push_str(&rect);
        leaves.push_str(&style.shape);
        leaves.push_str("/>");
        leaves.push_str(&text);
        leaves.push_str("</g>");
    }

    let total_width = (width.max(title.width) + 2.0 * pad).ceil();
    let total_height = (height + title_height + 2.0 * pad).ceil();
    let mut content = label_svg(
        &title,
        total_width / 2.0,
        pad + title.height / 2.0,
        " class=\"ondine-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    if !sections.is_empty() {
        content.push_str("<g class=\"ondine-sections\" font-size=\"");
        push_number(&mut content, small);
        content.push_str("\">");
        content.push_str(&sections);
        content.push_str("</g>");
    }
    if !leaves.is_empty() {
        content.push_str("<g class=\"ondine-leaves\" text-anchor=\"middle\" fill=\"var(--_bg)\">");
        content.push_str(&leaves);
        content.push_str("</g>");
    }

    let svg = svg_document("treemap", total_width, total_height, size, &content);
    Rendered { height: total_height, svg, width: total_width }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::leaf_count;
    use crate::Options;
    use crate::Theme;
    use crate::diagrams::treemap::parse;
    use crate::text::measure;

    const BUDGET: &str = "treemap-beta\n\"Budget\"\n    \"Operations\"\n        \"Salaries\": 700000\n        \"Equipment\": 200000\n        \"Supplies\": 100000\n    \"Marketing\"\n        \"Advertising\": 400000\n        \"Events\": 100000";

    struct Rect {
        id: String,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    fn attr(tag: &str, name: &str) -> f64 {
        let at = tag.find(&format!(" {name}=\"")).unwrap() + name.len() + 3;
        tag[at..at + tag[at..].find('"').unwrap()].parse().unwrap()
    }

    fn rects(svg: &str) -> Vec<Rect> {
        svg.split("data-id=\"")
            .skip(1)
            .filter_map(|part| {
                let id = &part[..part.find('"')?];
                let tag = part.strip_prefix(&format!("{id}\"><rect"))?;
                let tag = &tag[..tag.find("/>")?];
                Some(Rect {
                    id: id.to_owned(),
                    x: attr(tag, "x"),
                    y: attr(tag, "y"),
                    w: attr(tag, "width"),
                    h: attr(tag, "height"),
                })
            })
            .collect()
    }

    // Boxes too thin to keep a gap are left out.
    fn smallest_gap<'a>(boxes: impl Iterator<Item = &'a Rect> + Clone) -> f64 {
        let mut least = f64::INFINITY;
        for a in boxes.clone() {
            for b in boxes.clone() {
                if a.w.min(a.h).min(b.w).min(b.h) < 8.0 {
                    continue;
                }
                if a.y < b.y + b.h && b.y < a.y + a.h && b.x >= a.x + a.w {
                    least = least.min(b.x - a.x - a.w);
                }
                if a.x < b.x + b.w && b.x < a.x + a.w && b.y >= a.y + a.h {
                    least = least.min(b.y - a.y - a.h);
                }
            }
        }
        least
    }

    fn deep_tree() -> String {
        let mut src = String::from("treemap\n");
        for i in 0_u64..6 {
            writeln!(src, "\"top {i}\"").unwrap();
            for j in 0..8 {
                writeln!(src, "  \"mid {i}.{j}\"\n    \"inner {i}.{j}\"").unwrap();
                for k in 0..8 {
                    writeln!(src, "      \"leaf {i}.{j}.{k}\": {}", 1 + ((i * 64 + j * 8 + k) % 37).pow(3)).unwrap();
                }
            }
        }
        src
    }

    fn config(line: &str, src: &str) -> String {
        format!("---\nconfig:\n  treemap:\n{line}\n---\n{src}")
    }

    fn svg_at(src: &str, font_size: f64) -> String {
        crate::render(src, &Options { font_size, ..Options::default() }, &Theme::default()).unwrap().svg
    }

    fn svg(src: &str) -> String {
        svg_at(src, 16.0)
    }

    fn find<'a>(boxes: &'a [Rect], id: &str) -> &'a Rect {
        boxes.iter().find(|r| r.id == id).unwrap()
    }

    /// The text of every `<text>` in `svg`.
    fn texts(svg: &str) -> Vec<&str> {
        svg.split("<text").skip(1).filter_map(|t| Some(&t[t.find('>')? + 1..t.find("</text>")?])).collect()
    }

    #[test]
    fn counts_leaves_with_a_positive_value() {
        let model = parse("treemap\n\"s\"\n  \"a\": 1\n  \"b\": 0\n\"c\": 2\n\"e\"", None).unwrap();
        assert_eq!(leaf_count(&model.roots), 2);
    }

    #[test]
    fn draws_every_leaf_of_a_large_deep_tree() {
        assert_eq!(svg(&deep_tree()).matches("class=\"ondine-node ondine-leaf\"").count(), 6 * 8 * 8);
    }

    #[test]
    fn spaces_a_crowded_treemap_more_tightly_with_one_gap_throughout() {
        let boxes = rects(&svg(&deep_tree()));
        let top = find(&boxes, "top 0");
        let mid = boxes.iter().filter(|r| r.id.starts_with("mid 0.")).map(|r| r.x).fold(f64::INFINITY, f64::min);
        assert_eq!(mid - top.x, 3.0);
        let mut gaps = vec![smallest_gap(boxes.iter().filter(|r| r.id.starts_with("top ")))];
        for i in 0_u64..6 {
            gaps.push(smallest_gap(boxes.iter().filter(|r| r.id.starts_with(&format!("mid {i}.")))));
            for j in 0..8 {
                gaps.push(smallest_gap(boxes.iter().filter(|r| r.id.starts_with(&format!("leaf {i}.{j}.")))));
            }
        }
        gaps.retain(|g| g.is_finite());
        assert!(!gaps.is_empty() && gaps.iter().all(|&g| g == 2.0), "{gaps:?}");
    }

    #[test]
    fn keeps_the_spacing_of_a_roomy_treemap() {
        let boxes = rects(&svg(BUDGET));
        assert_eq!(find(&boxes, "Operations").x - find(&boxes, "Budget").x, 6.0);
        assert_eq!(find(&boxes, "Operations").y - find(&boxes, "Budget").y, 28.0);
        let named = |names: &[&str]| boxes.iter().filter(|r| names.contains(&r.id.as_str())).collect::<Vec<_>>();
        assert_eq!(smallest_gap(named(&["Salaries", "Equipment", "Supplies"]).into_iter()), 4.0);
        assert_eq!(smallest_gap(named(&["Operations", "Marketing"]).into_iter()), 4.0);
    }

    #[test]
    fn uses_the_padding_an_author_sets_as_the_gap() {
        let crowded = rects(&svg(&config("    padding: 6", &deep_tree())));
        assert_eq!(smallest_gap(crowded.iter().filter(|r| r.id.starts_with("top "))), 6.0);
        let roomy = rects(&svg(&config("    padding: 1", BUDGET)));
        let leaves = ["Salaries", "Equipment", "Supplies"];
        assert_eq!(smallest_gap(roomy.iter().filter(|r| leaves.contains(&r.id.as_str()))), 1.0);
    }

    #[test]
    fn draws_nothing_for_a_section_without_a_header() {
        let out = svg(&config("    nodeHeight: 3", "treemap\n\"Section\"\n  \"Leaf\": 1"));
        assert!(!out.contains("data-id=\"Section\""));
        assert!(out.contains("data-id=\"Leaf\""));
    }

    #[test]
    fn gives_a_small_section_a_short_header() {
        let out = svg(&config("    nodeHeight: 4", "treemap\n\"Section\"\n  \"Leaf\": 1"));
        assert!(out.contains("font-size=\"12\">Section<"), "{out}");
        let boxes = rects(&out);
        assert_eq!(find(&boxes, "Leaf").y - find(&boxes, "Section").y, 18.0);
        let tall = svg(&config(
            "    nodeHeight: 4",
            "treemap\n\"Section\":::big\n  \"Leaf\": 1\nclassDef big font-size:14.4px",
        ));
        assert!(!texts(&tall).contains(&"Section"));
    }

    #[test]
    fn fits_text_at_the_font_size_a_class_sets() {
        let out = svg(
            "treemap\n\"A section with a long name\":::head\n  \"A leaf with a fairly long name\": 30:::big\n  \"Another leaf with a long name\": 20:::big\n  \"rest\": 400\nclassDef big font-size:20px\nclassDef head font-size:18px",
        );
        let mut groups = 0;
        for (kind, size, style) in [("leaf big", 20.0, 0), ("section head", 18.0, measure::BOLD)] {
            for group in out.split(&format!("class=\"ondine-node ondine-{kind}\"")).skip(1) {
                let body = &group[..group.find("</g>").unwrap()];
                let w = attr(body, "width");
                let drawn = texts(body);
                assert!(!drawn.is_empty());
                for text in drawn {
                    assert!(measure::width(text, size, style) <= w - 8.0, "{text}");
                }
                groups += 1;
            }
        }
        assert_eq!(groups, 3);
    }

    #[test]
    fn keeps_section_names_at_large_font_sizes_that_fit_the_header() {
        let tree = "treemap\n\"Section\"\n  \"Leaf\": 1";
        assert!(texts(&svg_at(tree, 24.0)).contains(&"Section"));
        let styled = |size: &str| {
            svg(&format!("{tree}\nclassDef big font-size:{size}").replace("\"Section\"", "\"Section\":::big"))
        };
        assert!(texts(&styled("20px")).contains(&"Section"));
        assert!(!texts(&styled("32px")).contains(&"Section"));
    }

    #[test]
    fn draws_a_label_at_a_fractional_class_font_size_when_it_fits() {
        let out = svg(&config("    nodeHeight: 2", "treemap\n\"Label\": 1:::frac\nclassDef frac font-size:12.5px"));
        assert!(texts(&out).contains(&"Label"));
    }

    #[test]
    fn reads_class_names_that_start_with_title() {
        let out = svg("treemap\n\"S\":::titles\n  \"a\": 1:::title_2\nclassDef title_2 fill:red");
        assert!(out.contains("class=\"ondine-node ondine-section titles\""));
        assert!(out.contains("class=\"ondine-node ondine-leaf title_2\" data-id=\"a\""));
        let titled = parse("treemap\ntitle Sizes\n\"a\": 1", None).unwrap();
        assert_eq!(titled.title.as_deref(), Some("Sizes"));
    }
}
