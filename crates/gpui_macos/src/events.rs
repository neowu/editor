use std::borrow::Cow;
use std::ffi::c_void;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;

use gpui::Capslock;
use gpui::KeyDownEvent;
use gpui::KeyUpEvent;
use gpui::Keystroke;
use gpui::Modifiers;
use gpui::ModifiersChangedEvent;
use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::MouseExitEvent;
use gpui::MouseMoveEvent;
use gpui::MousePressureEvent;
use gpui::MouseUpEvent;
use gpui::NavigationDirection;
use gpui::PinchEvent;
use gpui::Pixels;
use gpui::PlatformInput;
use gpui::PressureStage;
use gpui::ScrollDelta;
use gpui::ScrollWheelEvent;
use gpui::TouchPhase;
use gpui::point;
use gpui::px;
use objc2::msg_send;
use objc2_app_kit::NSEvent;
use objc2_app_kit::NSEventModifierFlags;
use objc2_app_kit::NSEventPhase;
use objc2_app_kit::NSEventType;
use objc2_core_foundation::CFData;
use objc2_core_graphics::CGKeyCode;

use crate::carbon::LMGetKbdType;
use crate::carbon::TISCopyCurrentKeyboardLayoutInputSource;
use crate::carbon::TISGetInputSourceProperty;
use crate::carbon::UCKeyTranslate;
use crate::carbon::kTISPropertyUnicodeKeyLayoutData;
use crate::ns::id;

/// AppKit function-key characters as UTF-16 code units, usable in patterns.
#[allow(non_upper_case_globals)]
mod function_keys {
    macro_rules! function_keys {
        ($($name:ident),* $(,)?) => {
            $(pub(super) const $name: u16 = objc2_app_kit::$name as u16;)*
        };
    }

    function_keys!(
        NSUpArrowFunctionKey,
        NSDownArrowFunctionKey,
        NSLeftArrowFunctionKey,
        NSRightArrowFunctionKey,
        NSPageUpFunctionKey,
        NSPageDownFunctionKey,
        NSHomeFunctionKey,
        NSEndFunctionKey,
        NSDeleteFunctionKey,
        NSHelpFunctionKey,
        NSModeSwitchFunctionKey,
        NSF1FunctionKey,
        NSF2FunctionKey,
        NSF3FunctionKey,
        NSF4FunctionKey,
        NSF5FunctionKey,
        NSF6FunctionKey,
        NSF7FunctionKey,
        NSF8FunctionKey,
        NSF9FunctionKey,
        NSF10FunctionKey,
        NSF11FunctionKey,
        NSF12FunctionKey,
        NSF13FunctionKey,
        NSF14FunctionKey,
        NSF15FunctionKey,
        NSF16FunctionKey,
        NSF17FunctionKey,
        NSF18FunctionKey,
        NSF19FunctionKey,
        NSF20FunctionKey,
        NSF21FunctionKey,
        NSF22FunctionKey,
        NSF23FunctionKey,
        NSF24FunctionKey,
        NSF25FunctionKey,
        NSF26FunctionKey,
        NSF27FunctionKey,
        NSF28FunctionKey,
        NSF29FunctionKey,
        NSF30FunctionKey,
        NSF31FunctionKey,
        NSF32FunctionKey,
        NSF33FunctionKey,
        NSF34FunctionKey,
        NSF35FunctionKey,
    );
}

const BACKSPACE_KEY: u16 = 0x7f;
const SPACE_KEY: u16 = b' ' as u16;
const ENTER_KEY: u16 = 0x0d;
const NUMPAD_ENTER_KEY: u16 = 0x03;
pub(crate) const ESCAPE_KEY: u16 = 0x1b;
const TAB_KEY: u16 = 0x09;
const SHIFT_TAB_KEY: u16 = 0x19;

// Menu key equivalents use different character values than the ones NSEvent
// reports for the same physical keys.
//
// See https://developer.apple.com/documentation/appkit/nsmenuitem/keyequivalent#Discussion
const BACKSPACE_KEY_EQUIVALENT: u16 = 0x08;
const DELETE_KEY_EQUIVALENT: u16 = 0x7f;

pub(crate) fn key_to_menu_equivalent(key: &str) -> Cow<'_, str> {
    use function_keys::NSDownArrowFunctionKey;
    use function_keys::NSEndFunctionKey;
    use function_keys::NSF1FunctionKey;
    use function_keys::NSF2FunctionKey;
    use function_keys::NSF3FunctionKey;
    use function_keys::NSF4FunctionKey;
    use function_keys::NSF5FunctionKey;
    use function_keys::NSF6FunctionKey;
    use function_keys::NSF7FunctionKey;
    use function_keys::NSF8FunctionKey;
    use function_keys::NSF9FunctionKey;
    use function_keys::NSF10FunctionKey;
    use function_keys::NSF11FunctionKey;
    use function_keys::NSF12FunctionKey;
    use function_keys::NSF13FunctionKey;
    use function_keys::NSF14FunctionKey;
    use function_keys::NSF15FunctionKey;
    use function_keys::NSF16FunctionKey;
    use function_keys::NSF17FunctionKey;
    use function_keys::NSF18FunctionKey;
    use function_keys::NSF19FunctionKey;
    use function_keys::NSF20FunctionKey;
    use function_keys::NSF21FunctionKey;
    use function_keys::NSF22FunctionKey;
    use function_keys::NSF23FunctionKey;
    use function_keys::NSF24FunctionKey;
    use function_keys::NSF25FunctionKey;
    use function_keys::NSF26FunctionKey;
    use function_keys::NSF27FunctionKey;
    use function_keys::NSF28FunctionKey;
    use function_keys::NSF29FunctionKey;
    use function_keys::NSF30FunctionKey;
    use function_keys::NSF31FunctionKey;
    use function_keys::NSF32FunctionKey;
    use function_keys::NSF33FunctionKey;
    use function_keys::NSF34FunctionKey;
    use function_keys::NSF35FunctionKey;
    use function_keys::NSHelpFunctionKey;
    use function_keys::NSHomeFunctionKey;
    use function_keys::NSLeftArrowFunctionKey;
    use function_keys::NSPageDownFunctionKey;
    use function_keys::NSPageUpFunctionKey;
    use function_keys::NSRightArrowFunctionKey;
    use function_keys::NSUpArrowFunctionKey;
    let code = match key {
        "space" => SPACE_KEY,
        "backspace" => BACKSPACE_KEY_EQUIVALENT,
        "escape" => ESCAPE_KEY,
        "up" => NSUpArrowFunctionKey,
        "down" => NSDownArrowFunctionKey,
        "left" => NSLeftArrowFunctionKey,
        "right" => NSRightArrowFunctionKey,
        "pageup" => NSPageUpFunctionKey,
        "pagedown" => NSPageDownFunctionKey,
        "home" => NSHomeFunctionKey,
        "end" => NSEndFunctionKey,
        "delete" => DELETE_KEY_EQUIVALENT,
        "insert" => NSHelpFunctionKey,
        "f1" => NSF1FunctionKey,
        "f2" => NSF2FunctionKey,
        "f3" => NSF3FunctionKey,
        "f4" => NSF4FunctionKey,
        "f5" => NSF5FunctionKey,
        "f6" => NSF6FunctionKey,
        "f7" => NSF7FunctionKey,
        "f8" => NSF8FunctionKey,
        "f9" => NSF9FunctionKey,
        "f10" => NSF10FunctionKey,
        "f11" => NSF11FunctionKey,
        "f12" => NSF12FunctionKey,
        "f13" => NSF13FunctionKey,
        "f14" => NSF14FunctionKey,
        "f15" => NSF15FunctionKey,
        "f16" => NSF16FunctionKey,
        "f17" => NSF17FunctionKey,
        "f18" => NSF18FunctionKey,
        "f19" => NSF19FunctionKey,
        "f20" => NSF20FunctionKey,
        "f21" => NSF21FunctionKey,
        "f22" => NSF22FunctionKey,
        "f23" => NSF23FunctionKey,
        "f24" => NSF24FunctionKey,
        "f25" => NSF25FunctionKey,
        "f26" => NSF26FunctionKey,
        "f27" => NSF27FunctionKey,
        "f28" => NSF28FunctionKey,
        "f29" => NSF29FunctionKey,
        "f30" => NSF30FunctionKey,
        "f31" => NSF31FunctionKey,
        "f32" => NSF32FunctionKey,
        "f33" => NSF33FunctionKey,
        "f34" => NSF34FunctionKey,
        "f35" => NSF35FunctionKey,
        _ => return Cow::Borrowed(key),
    };
    Cow::Owned(String::from_utf16(&[code]).expect("key code is a valid utf-16 char"))
}

fn read_modifiers(native_event: &NSEvent) -> Modifiers {
    let modifiers = native_event.modifierFlags();
    let control = modifiers.contains(NSEventModifierFlags::Control);
    let alt = modifiers.contains(NSEventModifierFlags::Option);
    let shift = modifiers.contains(NSEventModifierFlags::Shift);
    let command = modifiers.contains(NSEventModifierFlags::Command);
    let function = modifiers.contains(NSEventModifierFlags::Function);

    Modifiers { control, alt, shift, platform: command, function }
}

pub(crate) unsafe fn platform_input_from_native(
    native_event: id,
    window_height: Option<Pixels>,
) -> Option<PlatformInput> {
    unsafe {
        let native_event: &NSEvent = &*native_event.cast();
        let event_type = native_event.r#type();

        // Filter out event types that aren't in the NSEventType enum.
        // See https://github.com/servo/cocoa-rs/issues/155#issuecomment-323482792 for details.
        match event_type.0 {
            0 | 21 | 32 | 33 | 35 | 36 | 37 => {
                return None;
            }
            _ => {}
        }

        match event_type {
            NSEventType::FlagsChanged => Some(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers: read_modifiers(native_event),
                capslock: Capslock { on: native_event.modifierFlags().contains(NSEventModifierFlags::CapsLock) },
            })),
            NSEventType::KeyDown => Some(PlatformInput::KeyDown(KeyDownEvent {
                keystroke: parse_keystroke(native_event),
                is_held: native_event.isARepeat(),
                prefer_character_input: false,
            })),
            NSEventType::KeyUp => Some(PlatformInput::KeyUp(KeyUpEvent { keystroke: parse_keystroke(native_event) })),
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };
                window_height.map(|window_height| {
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            // MacOS screen coordinates are relative to bottom left
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                        click_count: native_event.clickCount() as usize,
                        first_mouse: false,
                    })
                })
            }
            NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp => {
                let button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                        click_count: native_event.clickCount() as usize,
                    })
                })
            }
            NSEventType::Pressure => {
                let stage = native_event.stage();
                let pressure = native_event.pressure();

                window_height.map(|window_height| {
                    PlatformInput::MousePressure(MousePressureEvent {
                        stage: match stage {
                            1 => PressureStage::Normal,
                            2 => PressureStage::Force,
                            _ => PressureStage::Zero,
                        },
                        pressure,
                        modifiers: read_modifiers(native_event),
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                    })
                })
            }
            // Some mice (like Logitech MX Master) send navigation buttons as swipe events
            NSEventType::Swipe => {
                let navigation_direction = match native_event.phase() {
                    NSEventPhase::Ended => match native_event.deltaX() {
                        x if x > 0.0 => Some(NavigationDirection::Back),
                        x if x < 0.0 => Some(NavigationDirection::Forward),
                        _ => return None,
                    },
                    _ => return None,
                };

                match navigation_direction {
                    Some(direction) => window_height.map(|window_height| {
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Navigate(direction),
                            position: point(
                                px(native_event.locationInWindow().x as f32),
                                window_height - px(native_event.locationInWindow().y as f32),
                            ),
                            modifiers: read_modifiers(native_event),
                            click_count: 1,
                            first_mouse: false,
                        })
                    }),
                    _ => None,
                }
            }
            NSEventType::Magnify => window_height.map(|window_height| {
                let phase = match native_event.phase() {
                    NSEventPhase::MayBegin | NSEventPhase::Began => TouchPhase::Started,
                    NSEventPhase::Ended => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                };

                let magnification = native_event.magnification() as f32;

                PlatformInput::Pinch(PinchEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    delta: magnification,
                    modifiers: read_modifiers(native_event),
                    phase,
                })
            }),
            NSEventType::ScrollWheel => window_height.map(|window_height| {
                let phase = match native_event.phase() {
                    NSEventPhase::MayBegin | NSEventPhase::Began => TouchPhase::Started,
                    NSEventPhase::Ended => TouchPhase::Ended,
                    _ => TouchPhase::Moved,
                };

                let raw_data = point(native_event.scrollingDeltaX() as f32, native_event.scrollingDeltaY() as f32);

                let delta = if native_event.hasPreciseScrollingDeltas() {
                    ScrollDelta::Pixels(raw_data.map(px))
                } else {
                    ScrollDelta::Lines(raw_data)
                };

                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    delta,
                    touch_phase: phase,
                    modifiers: read_modifiers(native_event),
                })
            }),
            NSEventType::LeftMouseDragged | NSEventType::RightMouseDragged | NSEventType::OtherMouseDragged => {
                let pressed_button = match native_event.buttonNumber() {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Navigate(NavigationDirection::Back),
                    4 => MouseButton::Navigate(NavigationDirection::Forward),
                    // Other mouse buttons aren't tracked currently
                    _ => return None,
                };

                window_height.map(|window_height| {
                    PlatformInput::MouseMove(MouseMoveEvent {
                        pressed_button: Some(pressed_button),
                        position: point(
                            px(native_event.locationInWindow().x as f32),
                            window_height - px(native_event.locationInWindow().y as f32),
                        ),
                        modifiers: read_modifiers(native_event),
                    })
                })
            }
            NSEventType::MouseMoved => window_height.map(|window_height| {
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),
                    pressed_button: None,
                    modifiers: read_modifiers(native_event),
                })
            }),
            NSEventType::MouseExited => window_height.map(|window_height| {
                PlatformInput::MouseExited(MouseExitEvent {
                    position: point(
                        px(native_event.locationInWindow().x as f32),
                        window_height - px(native_event.locationInWindow().y as f32),
                    ),

                    pressed_button: None,
                    modifiers: read_modifiers(native_event),
                })
            }),
            _ => None,
        }
    }
}

fn parse_keystroke(native_event: &NSEvent) -> Keystroke {
    use function_keys::NSDeleteFunctionKey;
    use function_keys::NSDownArrowFunctionKey;
    use function_keys::NSEndFunctionKey;
    use function_keys::NSF1FunctionKey;
    use function_keys::NSF2FunctionKey;
    use function_keys::NSF3FunctionKey;
    use function_keys::NSF4FunctionKey;
    use function_keys::NSF5FunctionKey;
    use function_keys::NSF6FunctionKey;
    use function_keys::NSF7FunctionKey;
    use function_keys::NSF8FunctionKey;
    use function_keys::NSF9FunctionKey;
    use function_keys::NSF10FunctionKey;
    use function_keys::NSF11FunctionKey;
    use function_keys::NSF12FunctionKey;
    use function_keys::NSF13FunctionKey;
    use function_keys::NSF14FunctionKey;
    use function_keys::NSF15FunctionKey;
    use function_keys::NSF16FunctionKey;
    use function_keys::NSF17FunctionKey;
    use function_keys::NSF18FunctionKey;
    use function_keys::NSF19FunctionKey;
    use function_keys::NSF20FunctionKey;
    use function_keys::NSF21FunctionKey;
    use function_keys::NSF22FunctionKey;
    use function_keys::NSF23FunctionKey;
    use function_keys::NSF24FunctionKey;
    use function_keys::NSF25FunctionKey;
    use function_keys::NSF26FunctionKey;
    use function_keys::NSF27FunctionKey;
    use function_keys::NSF28FunctionKey;
    use function_keys::NSF29FunctionKey;
    use function_keys::NSF30FunctionKey;
    use function_keys::NSF31FunctionKey;
    use function_keys::NSF32FunctionKey;
    use function_keys::NSF33FunctionKey;
    use function_keys::NSF34FunctionKey;
    use function_keys::NSF35FunctionKey;
    use function_keys::NSHelpFunctionKey;
    use function_keys::NSHomeFunctionKey;
    use function_keys::NSLeftArrowFunctionKey;
    use function_keys::NSModeSwitchFunctionKey;
    use function_keys::NSPageDownFunctionKey;
    use function_keys::NSPageUpFunctionKey;
    use function_keys::NSRightArrowFunctionKey;
    use function_keys::NSUpArrowFunctionKey;

    let characters = native_event.charactersIgnoringModifiers().map_or_default(|characters| characters.to_string());
    let mut key_char = None;
    let first_char = characters.chars().next().map(|ch| ch as u16);
    let modifiers = native_event.modifierFlags();

    let control = modifiers.contains(NSEventModifierFlags::Control);
    let alt = modifiers.contains(NSEventModifierFlags::Option);
    let mut shift = modifiers.contains(NSEventModifierFlags::Shift);
    let command = modifiers.contains(NSEventModifierFlags::Command);
    let function = modifiers.contains(NSEventModifierFlags::Function)
        && first_char.is_none_or(|ch| !(NSUpArrowFunctionKey..=NSModeSwitchFunctionKey).contains(&ch));

    #[allow(non_upper_case_globals)]
    let key = match first_char {
        Some(SPACE_KEY) => {
            key_char = Some(" ".to_owned());
            "space".to_owned()
        }
        Some(TAB_KEY) => {
            key_char = Some("\t".to_owned());
            "tab".to_owned()
        }
        Some(ENTER_KEY | NUMPAD_ENTER_KEY) => {
            key_char = Some("\n".to_owned());
            "enter".to_owned()
        }
        Some(BACKSPACE_KEY) => "backspace".to_owned(),
        Some(ESCAPE_KEY) => "escape".to_owned(),
        Some(SHIFT_TAB_KEY) => "tab".to_owned(),
        Some(NSUpArrowFunctionKey) => "up".to_owned(),
        Some(NSDownArrowFunctionKey) => "down".to_owned(),
        Some(NSLeftArrowFunctionKey) => "left".to_owned(),
        Some(NSRightArrowFunctionKey) => "right".to_owned(),
        Some(NSPageUpFunctionKey) => "pageup".to_owned(),
        Some(NSPageDownFunctionKey) => "pagedown".to_owned(),
        Some(NSHomeFunctionKey) => "home".to_owned(),
        Some(NSEndFunctionKey) => "end".to_owned(),
        Some(NSDeleteFunctionKey) => "delete".to_owned(),
        // Observed Insert==NSHelpFunctionKey not NSInsertFunctionKey.
        Some(NSHelpFunctionKey) => "insert".to_owned(),
        Some(NSF1FunctionKey) => "f1".to_owned(),
        Some(NSF2FunctionKey) => "f2".to_owned(),
        Some(NSF3FunctionKey) => "f3".to_owned(),
        Some(NSF4FunctionKey) => "f4".to_owned(),
        Some(NSF5FunctionKey) => "f5".to_owned(),
        Some(NSF6FunctionKey) => "f6".to_owned(),
        Some(NSF7FunctionKey) => "f7".to_owned(),
        Some(NSF8FunctionKey) => "f8".to_owned(),
        Some(NSF9FunctionKey) => "f9".to_owned(),
        Some(NSF10FunctionKey) => "f10".to_owned(),
        Some(NSF11FunctionKey) => "f11".to_owned(),
        Some(NSF12FunctionKey) => "f12".to_owned(),
        Some(NSF13FunctionKey) => "f13".to_owned(),
        Some(NSF14FunctionKey) => "f14".to_owned(),
        Some(NSF15FunctionKey) => "f15".to_owned(),
        Some(NSF16FunctionKey) => "f16".to_owned(),
        Some(NSF17FunctionKey) => "f17".to_owned(),
        Some(NSF18FunctionKey) => "f18".to_owned(),
        Some(NSF19FunctionKey) => "f19".to_owned(),
        Some(NSF20FunctionKey) => "f20".to_owned(),
        Some(NSF21FunctionKey) => "f21".to_owned(),
        Some(NSF22FunctionKey) => "f22".to_owned(),
        Some(NSF23FunctionKey) => "f23".to_owned(),
        Some(NSF24FunctionKey) => "f24".to_owned(),
        Some(NSF25FunctionKey) => "f25".to_owned(),
        Some(NSF26FunctionKey) => "f26".to_owned(),
        Some(NSF27FunctionKey) => "f27".to_owned(),
        Some(NSF28FunctionKey) => "f28".to_owned(),
        Some(NSF29FunctionKey) => "f29".to_owned(),
        Some(NSF30FunctionKey) => "f30".to_owned(),
        Some(NSF31FunctionKey) => "f31".to_owned(),
        Some(NSF32FunctionKey) => "f32".to_owned(),
        Some(NSF33FunctionKey) => "f33".to_owned(),
        Some(NSF34FunctionKey) => "f34".to_owned(),
        Some(NSF35FunctionKey) => "f35".to_owned(),
        _ => {
            // Cases to test when modifying this:
            //
            //           qwerty key | none | cmd   | cmd-shift
            // * Armenian         s | ս    | cmd-s | cmd-shift-s  (layout is non-ASCII, so we use cmd layout)
            // * Dvorak+QWERTY    s | o    | cmd-s | cmd-shift-s  (layout switches on cmd)
            // * Ukrainian+QWERTY s | с    | cmd-s | cmd-shift-s  (macOS reports cmd-s instead of cmd-S)
            // * Czech            7 | ý    | cmd-ý | cmd-7        (layout has shifted numbers)
            // * Norwegian        7 | 7    | cmd-7 | cmd-/        (macOS reports cmd-shift-7 instead of cmd-/)
            // * Russian          7 | 7    | cmd-7 | cmd-&        (shift-7 is . but when cmd is down, should use cmd layout)
            // * German QWERTZ    ; | ö    | cmd-ö | cmd-Ö        (ed's shift special case only applies to a-z)
            //
            let code = native_event.keyCode();
            let mut chars_ignoring_modifiers = chars_for_modified_key(code, NO_MOD);
            let mut chars_with_shift = chars_for_modified_key(code, SHIFT_MOD);

            if !control && !command && !function {
                key_char = Some(match (shift, alt) {
                    (false, false) => chars_ignoring_modifiers.clone(),
                    (true, false) => chars_with_shift.clone(),
                    (false, true) => chars_for_modified_key(code, OPTION_MOD),
                    (true, true) => chars_for_modified_key(code, SHIFT_MOD | OPTION_MOD),
                });
            }

            // Handle Dvorak+QWERTY / Russian / Armenian
            if command || always_use_command_layout() {
                let chars_with_cmd = chars_for_modified_key(code, CMD_MOD);
                let chars_with_both = chars_for_modified_key(code, CMD_MOD | SHIFT_MOD);

                // We don't do this in the case that the shifted command key generates
                // the same character as the unshifted command key (Norwegian, e.g.)
                if chars_with_both != chars_with_cmd {
                    chars_with_shift = chars_with_both;

                // Handle edge-case where cmd-shift-s reports cmd-s instead of
                // cmd-shift-s (Ukrainian, etc.)
                } else if chars_with_cmd.to_ascii_uppercase() != chars_with_cmd {
                    chars_with_shift = chars_with_cmd.to_ascii_uppercase();
                }
                chars_ignoring_modifiers = chars_with_cmd;
            }

            if shift && chars_ignoring_modifiers.chars().all(|c| c.is_ascii_lowercase()) {
                chars_ignoring_modifiers
            } else if shift {
                shift = false;
                chars_with_shift
            } else {
                chars_ignoring_modifiers
            }
        }
    };

    Keystroke { modifiers: Modifiers { control, alt, shift, platform: command, function }, key, key_char }
}

fn always_use_command_layout() -> bool {
    static ALWAYS: OnceLock<bool> = OnceLock::new();
    *ALWAYS
        .get_or_init(|| !chars_for_modified_key(0, NO_MOD).is_ascii() && chars_for_modified_key(0, CMD_MOD).is_ascii())
}

/// The `UCKeyboardLayout` of the keyboard layout in use when a key was first translated.
///
/// It is read once: fetching the current input source costs far more than a translation, so a
/// layout switched to while the app runs is not picked up.
fn keyboard_layout_data() -> *const c_void {
    static LAYOUT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    let layout = LAYOUT.load(Ordering::Relaxed);
    if !layout.is_null() {
        return layout;
    }
    // The input source is never released, which keeps its layout data alive.
    let keyboard = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if keyboard.is_null() {
        return ptr::null();
    }
    let layout_data = unsafe {
        TISGetInputSourceProperty(keyboard, kTISPropertyUnicodeKeyLayoutData.cast::<c_void>()) as *const CFData
    };
    if layout_data.is_null() {
        unsafe {
            let _: () = msg_send![keyboard, release];
        }
        return ptr::null();
    }
    let layout = unsafe { (*layout_data).byte_ptr() }.cast::<c_void>().cast_mut();
    LAYOUT.store(layout, Ordering::Relaxed);
    layout
}

const NO_MOD: u32 = 0;
const CMD_MOD: u32 = 1;
const SHIFT_MOD: u32 = 2;
const OPTION_MOD: u32 = 8;

fn chars_for_modified_key(code: CGKeyCode, modifiers: u32) -> String {
    // Values from: https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/Carbon.framework/Versions/A/Frameworks/HIToolbox.framework/Versions/A/Headers/Events.h#L126
    // shifted >> 8 for UCKeyTranslate
    const CG_SPACE_KEY: u16 = 49;
    // https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.6.sdk/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/CarbonCore.framework/Versions/A/Headers/UnicodeUtilities.h#L278
    #[allow(non_upper_case_globals)]
    const kUCKeyActionDown: u16 = 0;
    #[allow(non_upper_case_globals)]
    const kUCKeyTranslateNoDeadKeysMask: u32 = 0;

    let keyboard_type = unsafe { LMGetKbdType() as u32 };
    const BUFFER_SIZE: usize = 4;
    let mut dead_key_state = 0;
    let mut buffer: [u16; BUFFER_SIZE] = [0; BUFFER_SIZE];
    let mut buffer_size: usize = 0;

    let keyboard_layout = keyboard_layout_data();
    if keyboard_layout.is_null() {
        return String::new();
    }

    unsafe {
        UCKeyTranslate(
            keyboard_layout,
            code,
            kUCKeyActionDown,
            modifiers,
            keyboard_type,
            kUCKeyTranslateNoDeadKeysMask,
            &raw mut dead_key_state,
            BUFFER_SIZE,
            &raw mut buffer_size,
            buffer.as_mut_ptr(),
        );
        if dead_key_state != 0 {
            UCKeyTranslate(
                keyboard_layout,
                CG_SPACE_KEY,
                kUCKeyActionDown,
                modifiers,
                keyboard_type,
                kUCKeyTranslateNoDeadKeysMask,
                &raw mut dead_key_state,
                BUFFER_SIZE,
                &raw mut buffer_size,
                buffer.as_mut_ptr(),
            );
        }
    }
    String::from_utf16(&buffer[..buffer_size]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::NO_MOD;
    use super::SHIFT_MOD;
    use super::always_use_command_layout;
    use super::chars_for_modified_key;
    use super::key_to_menu_equivalent;
    use super::keyboard_layout_data;

    #[test]
    fn key_translation_reuses_the_launch_layout() {
        let layout = keyboard_layout_data();
        assert!(!layout.is_null());
        assert_eq!(keyboard_layout_data(), layout);

        // Key code 49 is the space bar on every layout.
        assert_eq!(chars_for_modified_key(49, NO_MOD), " ");
        assert_eq!(chars_for_modified_key(49, SHIFT_MOD), " ");
        assert_eq!(always_use_command_layout(), always_use_command_layout());
    }

    #[test]
    fn menu_equivalents_use_appkit_delete_characters() {
        assert_eq!(key_to_menu_equivalent("backspace"), "\u{8}");
        assert_eq!(key_to_menu_equivalent("delete"), "\u{7f}");
        assert_eq!(key_to_menu_equivalent("a"), "a");
    }
}
