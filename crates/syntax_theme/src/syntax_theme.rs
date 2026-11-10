use std::collections::BTreeMap;
use std::ops::Bound;

use gpui::HighlightStyle;

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct SyntaxTheme {
    highlights: Vec<HighlightStyle>,
    capture_name_map: BTreeMap<String, usize>,
}

impl SyntaxTheme {
    /// A shared theme with no styles, for places that need a `&'static` default.
    pub fn empty() -> &'static Self {
        static EMPTY: SyntaxTheme = SyntaxTheme { highlights: Vec::new(), capture_name_map: BTreeMap::new() };
        &EMPTY
    }

    pub fn new(highlights: impl IntoIterator<Item = (String, HighlightStyle)>) -> Self {
        let (capture_names, highlights) = highlights.into_iter().unzip();

        Self { capture_name_map: Self::create_capture_name_map(capture_names), highlights }
    }

    fn create_capture_name_map(highlights: Vec<String>) -> BTreeMap<String, usize> {
        highlights.into_iter().enumerate().map(|(i, key)| (key, i)).collect()
    }

    pub fn get(&self, highlight_index: impl Into<usize>) -> Option<&HighlightStyle> {
        self.highlights.get(highlight_index.into())
    }

    pub fn style_for_name(&self, name: &str) -> Option<HighlightStyle> {
        self.capture_name_map.get(name).map(|highlight_idx| self.highlights[*highlight_idx])
    }

    pub fn get_capture_name(&self, idx: impl Into<usize>) -> Option<&str> {
        let idx = idx.into();
        self.capture_name_map.iter().find(|(_, value)| **value == idx).map(|(key, _)| key.as_ref())
    }

    pub fn highlight_id(&self, capture_name: &str) -> Option<u32> {
        self.capture_name_map
            .range::<str, _>((
                capture_name.split(".").next().map_or(Bound::Included(capture_name), Bound::Included),
                Bound::Included(capture_name),
            ))
            .rfind(|(prefix, _)| {
                capture_name
                    .strip_prefix(*prefix)
                    .is_some_and(|remainder| remainder.is_empty() || remainder.starts_with('.'))
            })
            .map(|(_, index)| *index as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::SyntaxTheme;

    #[test]
    fn test_empty_is_shared_and_has_no_styles() {
        assert!(std::ptr::eq(SyntaxTheme::empty(), SyntaxTheme::empty()));
        assert_eq!(SyntaxTheme::empty(), &SyntaxTheme::default());
        assert!(SyntaxTheme::empty().style_for_name("keyword").is_none());
    }
}
