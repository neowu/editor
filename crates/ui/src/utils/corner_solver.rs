use gpui::Pixels;

/// Calculates the child’s content-corner radius for a single nested level.
///
/// child_content_radius = max(0, parent_radius - parent_border - parent_padding + self_border)
///
/// - parent_radius: outer corner radius of the parent element
/// - parent_border: border width of the parent element
/// - parent_padding: padding of the parent element
/// - self_border: border width of this child element (for content inset)
pub fn inner_corner_radius(
    parent_radius: Pixels,
    parent_border: Pixels,
    parent_padding: Pixels,
    self_border: Pixels,
) -> Pixels {
    (parent_radius - parent_border - parent_padding + self_border).max(Pixels::ZERO)
}
