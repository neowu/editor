use std::ptr;

use gpui::FontFeatures;
use objc2_core_foundation::CFDictionary;
use objc2_core_foundation::CFMutableArray;
use objc2_core_foundation::CFNumber;
use objc2_core_foundation::CFRetained;
use objc2_core_foundation::CFString;
use objc2_core_foundation::CFType;
use objc2_core_text::CTFont;
use objc2_core_text::CTFontDescriptor;
use objc2_core_text::kCTFontFeatureSettingsAttribute;
use objc2_core_text::kCTFontOpenTypeFeatureTag;
use objc2_core_text::kCTFontOpenTypeFeatureValue;

pub(crate) fn apply_features(font: &mut CFRetained<CTFont>, features: &FontFeatures) {
    let feature_array = generate_feature_array(features);
    let attributes: CFRetained<CFDictionary<CFString, CFType>> =
        CFDictionary::from_slices(&[unsafe { kCTFontFeatureSettingsAttribute }], &[feature_array.as_ref()]);
    unsafe {
        let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
        *font = font.copy_with_attributes(0.0, ptr::null(), Some(&descriptor));
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFRetained<CFMutableArray<CFDictionary<CFString, CFType>>> {
    let feature_array: CFRetained<CFMutableArray<CFDictionary<CFString, CFType>>> = CFMutableArray::empty();
    for (tag, value) in features.tag_value_list() {
        let tag = CFString::from_str(tag);
        let value = CFNumber::new_i32((*value).cast_signed());
        let feature: CFRetained<CFDictionary<CFString, CFType>> = CFDictionary::from_slices(
            &[unsafe { kCTFontOpenTypeFeatureTag }, unsafe { kCTFontOpenTypeFeatureValue }],
            &[tag.as_ref(), value.as_ref()],
        );
        feature_array.append(&*feature);
    }
    feature_array
}

#[cfg(test)]
mod tests {
    use gpui::FontFeatures;

    use super::apply_features;
    use super::generate_feature_array;
    use crate::font;

    #[test]
    fn apply_features_keeps_the_font() {
        let features = FontFeatures::disable_ligatures();
        assert_eq!(generate_feature_array(&features).len(), 1);

        let mut font = font::family_fonts("Menlo").unwrap().remove(0);
        let name = unsafe { font.post_script_name() }.to_string();
        apply_features(&mut font, &features);
        assert_eq!(unsafe { font.post_script_name() }.to_string(), name);
        assert!(font::glyph_for_char(&font, 'm').is_some());
    }
}
