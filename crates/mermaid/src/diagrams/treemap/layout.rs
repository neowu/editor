//! Squarified treemap layout. Port of Pele's `src/diagrams/treemap/layout.ts`.

use super::model::TreemapNode;
use crate::svg::builder::js_round;

/// A node's rectangle. Boxes link to their parent and children by index into one list.
#[derive(Clone, Debug)]
pub(crate) struct Layout<'a> {
    /// Indices of the child boxes.
    children: Vec<usize>,
    /// Height.
    pub(crate) h: f64,
    /// The height of a section's header, or 0 when it is too small for one and gives its
    /// whole box to its children.
    pub(crate) header: f64,
    /// The node.
    pub(crate) node: &'a TreemapNode,
    /// Index of the parent box.
    parent: Option<usize>,
    /// Position among the top-level nodes of the branch this box belongs to, which picks its
    /// color.
    pub(crate) series: usize,
    /// A leaf's own value, or the sum of the leaves under a section.
    pub(crate) value: f64,
    /// Width.
    pub(crate) w: f64,
    /// Left edge.
    pub(crate) x: f64,
    /// Top edge.
    pub(crate) y: f64,
}

/// Space around boxes: Pele's `Insets`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Insets {
    /// Space between boxes.
    pub(crate) gap: f64,
    /// Space a section keeps for its header.
    pub(crate) header: f64,
    /// The header of a section too short for `header`.
    pub(crate) short: f64,
    /// Space a section keeps around its children on the other three sides.
    pub(crate) side: f64,
}

/// A box for `node`, valued by its own value when it is a positive leaf.
fn new_box(node: &TreemapNode, parent: Option<usize>, series: usize) -> Layout<'_> {
    let value = match (&node.children, node.value) {
        (None, Some(value)) if value.is_finite() && value > 0.0 => value,
        _ => 0.0,
    };
    Layout { children: Vec::new(), h: 0.0, header: 0.0, node, parent, series, value, w: 0.0, x: 0.0, y: 0.0 }
}

/// Lays out the boxes `shown` in the rectangle as one squarified treemap: rows are added along the
/// shorter side, and a row stops growing when another box would make its boxes less square.
#[expect(clippy::many_single_char_names, reason = "the rectangle's coordinates, as in Pele")]
fn squarify(boxes: &mut [Layout<'_>], shown: &[usize], total: f64, rect: [f64; 4], gap: f64) {
    let [mut x, mut y, mut w, mut h] = rect;
    let value = |boxes: &[Layout<'_>], i: usize| shown.get(i).and_then(|&k| boxes.get(k)).map_or(0.0, |b| b.value);
    let mut rest = total;
    let mut from = 0;
    while from < shown.len() {
        let scale = (w * h) / rest;
        let side = w.min(h);
        let mut sum = value(boxes, from) * scale;
        let largest = sum;
        let mut worst = ((side * side * largest) / (sum * sum)).max((sum * sum) / (side * side * largest));
        let mut to = from + 1;
        while to < shown.len() {
            let smallest = value(boxes, to) * scale;
            let next = sum + smallest;
            let ratio = ((side * side * largest) / (next * next)).max((next * next) / (side * side * smallest));
            if ratio > worst {
                break;
            }
            worst = ratio;
            sum = next;
            to += 1;
        }

        let thickness = sum / side;
        let mut offset = 0.0;
        for i in from..to {
            let Some(b) = shown.get(i).and_then(|&k| boxes.get_mut(k)) else {
                continue;
            };
            let length = if i == to - 1 { side - offset } else { (b.value * scale) / thickness };
            let bx = if w >= h { x } else { x + offset };
            let by = if w >= h { y + offset } else { y };
            let bw = if w >= h { thickness } else { length };
            let bh = if w >= h { length } else { thickness };
            // Omit the gap where it would erase the box.
            let gx = if js_round(bx + bw - gap / 2.0) > js_round(bx + gap / 2.0) { gap / 2.0 } else { 0.0 };
            let gy = if js_round(by + bh - gap / 2.0) > js_round(by + gap / 2.0) { gap / 2.0 } else { 0.0 };
            b.x = js_round(bx + gx);
            b.y = js_round(by + gy);
            b.w = js_round(bx + bw - gx) - b.x;
            b.h = js_round(by + bh - gy) - b.y;
            offset += length;
            rest -= b.value;
        }
        if w >= h {
            x += thickness;
            w -= thickness;
        } else {
            y += thickness;
            h -= thickness;
        }
        from = to;
    }
}

/// `v` within `low..=high`, or `low` when `v` is below it or NaN.
fn clamp(v: f64, low: f64, high: f64) -> f64 {
    if v >= low { v.min(high) } else { low }
}

/// Lays out the children `children` of a box in the rectangle, adding every one with a positive
/// value to `placed`, kept inside the rectangle even when that leaves it no size.
fn place(boxes: &mut [Layout<'_>], placed: &mut Vec<usize>, children: &[usize], rect: [f64; 4], gap: f64) {
    let [x, y, w, h] = rect;
    let (w, h) = (w.max(0.0), h.max(0.0));
    let mut shown: Vec<usize> =
        children.iter().copied().filter(|&k| boxes.get(k).is_some_and(|b| b.value > 0.0)).collect();
    let value = |k: usize| boxes.get(k).map_or(0.0, |b| b.value);
    shown.sort_by(|&a, &b| value(b).total_cmp(&value(a)));
    let mut total = 0.0;
    for &k in &shown {
        total += value(k);
    }
    if shown.is_empty() || !total.is_finite() {
        return;
    }

    if w > 0.0 && h > 0.0 {
        squarify(boxes, &shown, total, [x - gap / 2.0, y - gap / 2.0, w + gap, h + gap], gap);
    }
    for k in shown {
        if let Some(b) = boxes.get_mut(k) {
            let x0 = clamp(b.x, x, x + w);
            let y0 = clamp(b.y, y, y + h);
            b.w = clamp(b.x + b.w, x0, x + w) - x0;
            b.h = clamp(b.y + b.h, y0, y + h) - y0;
            b.x = x0;
            b.y = y0;
        }
        placed.push(k);
    }
}

/// Lays out `roots` in a `width` by `height` rectangle. Returns every box and the indices of
/// those with a positive value, zero-size ones included, parents before their children.
pub(crate) fn layout_treemap(
    roots: &[TreemapNode],
    width: f64,
    height: f64,
    insets: Insets,
) -> (Vec<Layout<'_>>, Vec<usize>) {
    let mut boxes: Vec<Layout<'_>> = roots.iter().enumerate().map(|(index, node)| new_box(node, None, index)).collect();
    let top: Vec<usize> = (0..boxes.len()).collect();
    let mut i = 0;
    while let Some(parent) = boxes.get(i) {
        let node = parent.node;
        let series = parent.series;
        for child in node.children.iter().flatten() {
            let k = boxes.len();
            boxes.push(new_box(child, Some(i), series));
            if let Some(parent) = boxes.get_mut(i) {
                parent.children.push(k);
            }
        }
        i += 1;
    }
    for k in (0..boxes.len()).rev() {
        let Some(b) = boxes.get(k) else {
            continue;
        };
        let (parent, value) = (b.parent, b.value);
        if let Some(parent) = parent.and_then(|parent| boxes.get_mut(parent)) {
            parent.value += value;
        }
    }

    let Insets { gap, header, short, side } = insets;
    let mut placed = Vec::new();
    place(&mut boxes, &mut placed, &top, [0.0, 0.0, width, height], gap);
    let mut i = 0;
    while let Some(&k) = placed.get(i) {
        if let Some(b) = boxes.get_mut(k)
            && !b.children.is_empty()
        {
            let top = if b.h - header - side >= 12.0 { header } else { short };
            let headed = b.w > (2.0 * side).max(24.0) && b.h - top - side >= 8.0;
            b.header = if headed { top } else { 0.0 };
            let rect =
                if headed { [b.x + side, b.y + top, b.w - 2.0 * side, b.h - top - side] } else { [b.x, b.y, b.w, b.h] };
            let children = b.children.clone();
            place(&mut boxes, &mut placed, &children, rect, gap);
        }
        i += 1;
    }
    (boxes, placed)
}

#[cfg(test)]
mod tests {
    use super::Insets;
    use super::Layout;
    use super::TreemapNode;
    use super::layout_treemap;

    const INSETS: Insets = Insets { gap: 4.0, header: 28.0, short: 18.0, side: 6.0 };

    fn leaf(name: &str, value: f64) -> TreemapNode {
        TreemapNode { name: name.to_owned(), value: Some(value), ..TreemapNode::default() }
    }

    fn section(name: &str, children: Vec<TreemapNode>) -> TreemapNode {
        TreemapNode { name: name.to_owned(), children: Some(children), ..TreemapNode::default() }
    }

    fn placed<'a>(boxes: &'a [Layout<'a>], placed: &[usize]) -> Vec<&'a Layout<'a>> {
        placed.iter().map(|&k| &boxes[k]).collect()
    }

    fn overlap(a: &Layout<'_>, b: &Layout<'_>) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn gives_a_section_too_small_for_its_header_its_whole_box() {
        let tree = [section("s", vec![leaf("a", 2.0), section("t", vec![leaf("b", 1.0), leaf("c", 1.0)])])];
        let (boxes, order) = layout_treemap(&tree, 300.0, 30.0, INSETS);
        let shown = placed(&boxes, &order);
        let mut names: Vec<&str> = shown.iter().map(|b| b.node.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["a", "b", "c", "s", "t"]);
        let s = shown[0];
        assert_eq!(s.header, 0.0);
        for b in &shown[1..] {
            assert!(b.w * b.h > 0.0, "{}", b.node.name);
            assert!(b.x >= s.x && b.y >= s.y && b.x + b.w <= s.x + s.w && b.y + b.h <= s.y + s.h);
        }

        let (boxes, _) = layout_treemap(&tree, 300.0, 300.0, INSETS);
        assert_eq!(boxes[0].header, 28.0);
        let (boxes, _) = layout_treemap(&tree, 300.0, 45.0, INSETS);
        assert_eq!(boxes[0].header, 18.0);
    }

    #[test]
    fn keeps_a_box_thinner_than_the_gap_and_one_with_no_room() {
        let tree = [leaf("big", 397.0), leaf("thin", 3.0)];
        let (boxes, order) = layout_treemap(&tree, 400.0, 100.0, INSETS);
        let thin = placed(&boxes, &order);
        assert_eq!(thin.iter().map(|b| b.node.name.as_str()).collect::<Vec<_>>(), ["big", "thin"]);
        assert!(thin[1].w > 0.0);
        assert!(!overlap(thin[0], thin[1]));

        let tree = [leaf("big", 1e6), section("s", vec![leaf("speck", 1.0)])];
        let (boxes, order) = layout_treemap(&tree, 100.0, 100.0, INSETS);
        let speck = placed(&boxes, &order);
        assert_eq!(speck.iter().map(|b| b.node.name.as_str()).collect::<Vec<_>>(), ["big", "s", "speck"]);
        for b in &speck[1..] {
            assert_eq!(b.w * b.h, 0.0);
        }
    }

    #[test]
    fn keeps_every_box_inside_its_area_with_a_finite_size() {
        let tree = [leaf("big", 397.0), leaf("thin", 3.0)];
        let (boxes, _) = layout_treemap(&tree, 400.0, 100.0, Insets { gap: 40.0, ..INSETS });
        assert!(boxes.iter().all(|b| b.x + b.w <= 400.0));

        let tree = [leaf("big", 1e20), leaf("small", 1.0)];
        let (boxes, order) = layout_treemap(&tree, 640.0, 400.0, INSETS);
        assert_eq!(order.len(), 2);
        for b in &boxes {
            assert!([b.x, b.y, b.w, b.h].iter().all(|n| n.is_finite() && *n >= 0.0));
        }
    }
}
