use gpui::Hsla;

/// A collection of colors that are used to color indent aware lines in the editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccentColors(pub [Hsla; 7]);

impl AccentColors {
    /// Returns the color for the given index.
    pub const fn color_for_index(&self, index: u32) -> Hsla {
        self.0[index as usize % self.0.len()]
    }
}
