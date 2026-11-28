use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub(crate) enum ScrollAmount {
    // Scroll N lines (positive is towards the end of the document)
    Line(f32),
    // Scroll N pages (positive is towards the end of the document)
    Page(f32),
    // Scroll N columns (positive is towards the right of the document)
    Column(f32),
    // Scroll N page width (positive is towards the right of the document)
    PageWidth(f32),
}

impl ScrollAmount {
    pub(crate) fn lines(self, mut visible_line_count: f64) -> f64 {
        match self {
            Self::Line(count) => count as f64,
            Self::Page(count) => {
                // for full pages subtract one to leave an anchor line
                if self.is_full_page() {
                    visible_line_count -= 1.0;
                }
                (visible_line_count * (count as f64)).trunc()
            }
            Self::Column(_count) => 0.0,
            Self::PageWidth(_count) => 0.0,
        }
    }

    pub(crate) fn columns(self, visible_column_count: f64) -> f64 {
        match self {
            Self::Line(_count) => 0.0,
            Self::Page(_count) => 0.0,
            Self::Column(count) => count as f64,
            Self::PageWidth(count) => (visible_column_count * count as f64).trunc(),
        }
    }

    fn is_full_page(self) -> bool {
        matches!(self, ScrollAmount::Page(count) if count.abs() == 1.0)
    }
}
