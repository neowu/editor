use gpui::WindowAppearance;
use objc2::msg_send;

use crate::NSStringExt as _;
use crate::ns::id;

pub(crate) unsafe fn window_appearance_from_native(appearance: id) -> WindowAppearance {
    let name: id = msg_send![appearance, name];
    // Every dark appearance (dark aqua, vibrant dark, their high-contrast variants) is named so.
    if unsafe { is_dark_appearance_name(name.to_str()) } { WindowAppearance::Dark } else { WindowAppearance::Light }
}

fn is_dark_appearance_name(name: &str) -> bool {
    name.contains("Dark")
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    pub(crate) static NSAppearanceNameAqua: id;
    pub(crate) static NSAppearanceNameDarkAqua: id;
}

#[cfg(test)]
mod tests {
    use super::is_dark_appearance_name;

    #[test]
    fn dark_appearance_names_are_dark() {
        for name in [
            "NSAppearanceNameDarkAqua",
            "NSAppearanceNameVibrantDark",
            "NSAppearanceNameAccessibilityHighContrastDarkAqua",
            "NSAppearanceNameAccessibilityHighContrastVibrantDark",
        ] {
            assert!(is_dark_appearance_name(name), "{name}");
        }
        for name in [
            "NSAppearanceNameAqua",
            "NSAppearanceNameVibrantLight",
            "NSAppearanceNameAccessibilityHighContrastAqua",
            "",
        ] {
            assert!(!is_dark_appearance_name(name), "{name}");
        }
    }
}
