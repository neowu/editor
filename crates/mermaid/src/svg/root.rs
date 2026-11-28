//! The root SVG document around a diagram's content. Port of Pele's `src/svg/root.ts`.

use crate::yaml::push_number;

/// The root's stroke width and text color: shapes and edges inherit the stroke width, and an
/// icon is drawn in `currentColor`, which would otherwise be whatever color the host gives.
const ROOT_STYLE: &str = "stroke-width:var(--_sw);color:var(--_fg)";

/// The root's font declarations at `size` pixels. Every font is measured as monospace, so what
/// text inherits is pinned, as none of it was measured. Kerning and ligatures are off and glyphs
/// are placed at their exact size, so every character keeps the uniform advance that layout
/// assumed. Spacing is `0` rather than `normal`, which usvg does not parse.
fn font_style(size: f64) -> String {
    let mut out = String::with_capacity(256);
    out.push_str(";font-size:");
    push_number(&mut out, size);
    out.push_str("px;font-family:var(--_fm);letter-spacing:0;word-spacing:0;text-transform:none;direction:ltr;font-kerning:none;font-variant-ligatures:none;text-rendering:geometricPrecision");
    out
}

/// Wraps a diagram's content in the root `<svg>` document of the given fixed size, with its
/// theme, font at `font_size`, and diagram `type` class.
pub(crate) fn svg_document(r#type: &str, width: f64, height: f64, font_size: f64, content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 1024);
    out.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"ondine ondine-");
    out.push_str(r#type);
    out.push_str("\" viewBox=\"0 0 ");
    push_number(&mut out, width);
    out.push(' ');
    push_number(&mut out, height);
    out.push_str("\" width=\"");
    push_number(&mut out, width);
    out.push_str("\" height=\"");
    push_number(&mut out, height);
    out.push_str("\" style=\"");
    out.push_str(ROOT_STYLE);
    out.push_str(&font_style(font_size));
    out.push_str("\" fill=\"var(--_fg)\">");
    out.push_str(content);
    out.push_str("</svg>");
    out
}
