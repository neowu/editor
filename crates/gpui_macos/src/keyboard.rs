use std::ffi::CStr;
use std::ffi::c_void;

use gpui::PlatformKeyboardLayout;
use objc2::msg_send;
use objc2::runtime::AnyObject;

use crate::TISCopyCurrentKeyboardLayoutInputSource;
use crate::TISGetInputSourceProperty;
use crate::kTISPropertyInputSourceID;
use crate::kTISPropertyLocalizedName;

pub(crate) struct MacKeyboardLayout {
    id: String,
    name: String,
}

impl PlatformKeyboardLayout for MacKeyboardLayout {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl MacKeyboardLayout {
    pub(crate) fn new() -> Self {
        unsafe {
            let current_keyboard = TISCopyCurrentKeyboardLayoutInputSource();

            let id: *mut AnyObject =
                TISGetInputSourceProperty(current_keyboard, kTISPropertyInputSourceID as *const c_void);
            let id: *const std::os::raw::c_char = msg_send![id, UTF8String];
            let id = CStr::from_ptr(id).to_str().unwrap().to_string();

            let name: *mut AnyObject =
                TISGetInputSourceProperty(current_keyboard, kTISPropertyLocalizedName as *const c_void);
            let name: *const std::os::raw::c_char = msg_send![name, UTF8String];
            let name = CStr::from_ptr(name).to_str().unwrap().to_string();

            let _: () = msg_send![current_keyboard, release];

            Self { id, name }
        }
    }
}
