//! Carbon text input source and key translation FFI.

use std::ffi::c_void;

use objc2::runtime::AnyObject;
use objc2_core_foundation::CFString;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub(super) fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut AnyObject;
    pub(super) fn TISCopyCurrentKeyboardInputSource() -> *mut AnyObject;
    pub(super) fn TISGetInputSourceProperty(inputSource: *mut AnyObject, propertyKey: *const c_void) -> *mut AnyObject;

    pub(super) fn UCKeyTranslate(
        keyLayoutPtr: *const c_void,
        virtualKeyCode: u16,
        keyAction: u16,
        modifierKeyState: u32,
        keyboardType: u32,
        keyTranslateOptions: u32,
        deadKeyState: *mut u32,
        maxStringLength: usize,
        actualStringLength: *mut usize,
        unicodeString: *mut u16,
    ) -> u32;
    pub(super) fn LMGetKbdType() -> u16;
    pub(super) static kTISPropertyUnicodeKeyLayoutData: *const CFString;
    pub(super) static kTISPropertyInputSourceIsASCIICapable: *const CFString;
    pub(super) static kTISPropertyInputSourceType: *const CFString;
    pub(super) static kTISTypeKeyboardInputMode: *const CFString;
}
