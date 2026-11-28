//! macOS platform implementation for GPUI.
//!
//! macOS screens have a y axis that goes up from the bottom of the screen and
//! an origin at the bottom left of the main display.
//!
//! Scenes are rendered directly with Metal, and text is shaped and rasterized
//! with Core Text.

mod carbon;
mod dispatcher;
mod display;
mod display_link;
mod events;
mod font;
mod metal_atlas;
mod metal_renderer;
mod ns;
mod open_type;
mod pasteboard;
mod platform;
mod text_system;
mod window;
mod window_appearance;

use std::ffi::CStr;
use std::ffi::c_char;
use std::ffi::c_void;
use std::ops::Range;

pub(crate) use dispatcher::AppleDispatcher;
pub(crate) use display::MacDisplay;
use metal_renderer as renderer;
use ns::NSNotFound;
use ns::NSUInteger;
use ns::id;
use objc2::Encode;
use objc2::Encoding;
use objc2::RefEncode;
use objc2::class;
use objc2::msg_send;
pub use platform::MacPlatform;
pub(crate) use window::MacWindow;
pub(crate) use window::set_active_window_cursor_style;

trait NSStringExt {
    unsafe fn to_str(&self) -> &str;
}

impl NSStringExt for id {
    unsafe fn to_str(&self) -> &str {
        if self.is_null() {
            return "";
        }
        unsafe {
            let cstr: *const c_char = msg_send![*self, UTF8String];
            if cstr.is_null() { "" } else { CStr::from_ptr(cstr).to_str().expect("NSString is valid utf-8") }
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct NSRange {
    location: NSUInteger,
    length: NSUInteger,
}

impl NSRange {
    const fn invalid() -> Self {
        Self { location: NSNotFound as NSUInteger, length: 0 }
    }

    const fn is_valid(&self) -> bool {
        self.location != NSNotFound as NSUInteger
    }

    const fn to_range(self) -> Option<Range<usize>> {
        if self.is_valid() {
            let start = self.location;
            let end = start + self.length;
            Some(start..end)
        } else {
            None
        }
    }
}

impl From<Range<usize>> for NSRange {
    fn from(range: Range<usize>) -> Self {
        NSRange { location: range.start as NSUInteger, length: range.len() as NSUInteger }
    }
}

// SAFETY: `NSRange` is `#[repr(C)]` with the same layout as Foundation's `_NSRange`.
unsafe impl Encode for NSRange {
    const ENCODING: Encoding = objc2_foundation::NSRange::ENCODING;
}

// SAFETY: see the `Encode` impl above.
unsafe impl RefEncode for NSRange {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// Returns an autoreleased `NSString`.
unsafe fn ns_string(string: &str) -> id {
    const NS_UTF8_STRING_ENCODING: NSUInteger = 4;
    unsafe {
        let ns_string: id = msg_send![class!(NSString), alloc];
        let ns_string: id = msg_send![
            ns_string,
            initWithBytes: string.as_ptr().cast::<c_void>(),
            length: string.len(),
            encoding: NS_UTF8_STRING_ENCODING
        ];
        msg_send![ns_string, autorelease]
    }
}
