use std::ptr::NonNull;
use std::time::Duration;

use anyhow::Result;
use gpui::Bounds;
use gpui::DisplayId;
use gpui::Pixels;
use gpui::PlatformDisplay;
use gpui::point;
use gpui::px;
use gpui::refresh_interval_from_hz;
use gpui::size;
use objc2::class;
use objc2::msg_send;
use objc2_core_foundation::CFRetained;
use objc2_core_foundation::CFUUID;
use objc2_core_graphics::CGDirectDisplayID;
use objc2_core_graphics::CGDisplayBounds;
use objc2_core_graphics::CGDisplayCopyDisplayMode;
use objc2_core_graphics::CGDisplayMode;
use objc2_core_graphics::CGError;
use objc2_core_graphics::CGGetActiveDisplayList;
use uuid::Uuid;

use crate::ns::NSPoint;
use crate::ns::NSRect;
use crate::ns::NSUInteger;
use crate::ns::array_iter;
use crate::ns::id;
use crate::ns::nil;
use crate::ns_string;

#[derive(Debug)]
pub(crate) struct MacDisplay(pub(crate) CGDirectDisplayID);

unsafe impl Send for MacDisplay {}

impl MacDisplay {
    /// Get the screen with the given [`DisplayId`].
    pub(crate) fn find_by_id(id: DisplayId) -> Option<Self> {
        Self::all().find(|screen| screen.id() == id)
    }

    /// The screen the user is working on: the one under the mouse cursor,
    /// else `NSScreen.mainScreen`. Not `mainScreen` first: without a key
    /// window (e.g. at launch) it is the menu bar's screen, which stays on the
    /// primary display unless displays have separate Spaces. `None` without a
    /// window server connection.
    pub(crate) fn main() -> Option<Self> {
        unsafe {
            let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let screens: id = msg_send![class!(NSScreen), screens];
            let screen = array_iter(screens)
                .find(|&screen| {
                    let frame: NSRect = msg_send![screen, frame];
                    contains_point(frame, mouse)
                })
                .unwrap_or_else(|| msg_send![class!(NSScreen), mainScreen]);
            (screen != nil).then(|| Self::for_screen(screen))
        }
    }

    unsafe fn for_screen(screen: id) -> Self {
        unsafe {
            let device_description: id = msg_send![screen, deviceDescription];
            let screen_number_key: id = ns_string("NSScreenNumber");
            let screen_number: id = msg_send![device_description, objectForKey: screen_number_key];
            let screen_number: NSUInteger = msg_send![screen_number, unsignedIntegerValue];
            Self(screen_number as CGDirectDisplayID)
        }
    }

    /// Obtains an iterator over all currently active system displays.
    ///
    /// Empty if the system can't list them, e.g. without a window server
    /// connection.
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        // We're assuming there aren't more than 32 displays connected to the system.
        let mut displays = Vec::with_capacity(32);
        let mut display_count = 0;
        // SAFETY: `displays` has room for the capacity passed in, and
        // `display_count` reports how many entries were written.
        let result = unsafe {
            CGGetActiveDisplayList(displays.capacity() as u32, displays.as_mut_ptr(), &raw mut display_count)
        };
        if result == CGError::Success {
            // SAFETY: `CGGetActiveDisplayList` initialized `display_count`
            // entries, at most the capacity it was given.
            unsafe {
                displays.set_len(display_count as usize);
            }
        } else {
            log::error!("Failed to get active display list. Result: {}", result.0);
        }
        displays.into_iter().map(MacDisplay)
    }

    /// Find the NSScreen corresponding to this display.
    unsafe fn get_nsscreen(&self) -> id {
        let screens: id = unsafe { msg_send![class!(NSScreen), screens] };
        let screen_number_key: id = unsafe { ns_string("NSScreenNumber") };

        for screen in unsafe { array_iter(screens) } {
            let device_description: id = unsafe { msg_send![screen, deviceDescription] };
            let screen_number: id = unsafe { msg_send![device_description, objectForKey: screen_number_key] };
            let screen_id: NSUInteger = msg_send![screen_number, unsignedIntegerValue];
            if screen_id as CGDirectDisplayID == self.0 {
                return screen;
            }
        }
        nil
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> *mut CFUUID;
}

impl PlatformDisplay for MacDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(self.0 as u64)
    }

    fn uuid(&self) -> Result<Uuid> {
        let cfuuid = NonNull::new(unsafe { CGDisplayCreateUUIDFromDisplayID(self.0) })
            .ok_or_else(|| anyhow::anyhow!("AppKit returned a null from CGDisplayCreateUUIDFromDisplayID"))?;
        // SAFETY: the function follows the Create rule, so we own this reference.
        let cfuuid = unsafe { CFRetained::from_raw(cfuuid) };
        let bytes = cfuuid.uuid_bytes();
        Ok(Uuid::from_bytes([
            bytes.byte0,
            bytes.byte1,
            bytes.byte2,
            bytes.byte3,
            bytes.byte4,
            bytes.byte5,
            bytes.byte6,
            bytes.byte7,
            bytes.byte8,
            bytes.byte9,
            bytes.byte10,
            bytes.byte11,
            bytes.byte12,
            bytes.byte13,
            bytes.byte14,
            bytes.byte15,
        ]))
    }

    fn bounds(&self) -> Bounds<Pixels> {
        // CGDisplayBounds is in "global display" coordinates, where 0 is
        // the top left of the primary display.
        let bounds = CGDisplayBounds(self.0);

        Bounds {
            origin: gpui::Point::default(),
            size: size(px(bounds.size.width as f32), px(bounds.size.height as f32)),
        }
    }

    fn visible_bounds(&self) -> Bounds<Pixels> {
        unsafe {
            let dominated_screen = self.get_nsscreen();

            if dominated_screen == nil {
                return self.bounds();
            }

            let screen_frame: NSRect = msg_send![dominated_screen, frame];
            let visible_frame: NSRect = msg_send![dominated_screen, visibleFrame];

            // Convert from bottom-left origin (AppKit) to top-left origin
            let origin_y =
                screen_frame.size.height - visible_frame.origin.y - visible_frame.size.height + screen_frame.origin.y;

            Bounds {
                origin: point(px(visible_frame.origin.x as f32 - screen_frame.origin.x as f32), px(origin_y as f32)),
                size: size(px(visible_frame.size.width as f32), px(visible_frame.size.height as f32)),
            }
        }
    }

    fn refresh_interval(&self) -> Option<Duration> {
        // `maximumFramesPerSecond` is the ProMotion maximum, where the
        // display mode's rate is 0 on built-in panels.
        let screen_hertz = unsafe {
            let screen = self.get_nsscreen();
            if screen == nil {
                0.0
            } else {
                let frames_per_second: isize = msg_send![screen, maximumFramesPerSecond];
                frames_per_second as f64
            }
        };
        refresh_interval_from_hz(screen_hertz).or_else(|| {
            let mode = CGDisplayCopyDisplayMode(self.0)?;
            refresh_interval_from_hz(CGDisplayMode::refresh_rate(Some(&mode)))
        })
    }
}

/// Whether AppKit's bottom-left-origin `frame` contains `point`, matching
/// `NSMouseInRect` (unflipped): the top edge is in, the bottom edge is not, so
/// a cursor on the boundary between two stacked screens belongs to one.
fn contains_point(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y > frame.origin.y
        && point.y <= frame.origin.y + frame.size.height
}

#[cfg(test)]
mod tests {
    use objc2_foundation::NSSize;

    use super::MacDisplay;
    use super::contains_point;
    use crate::ns::NSPoint;
    use crate::ns::NSRect;

    #[test]
    fn contains_point_works() {
        // A second display to the right of and above the primary one.
        let primary = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1728., 1117.));
        let second = NSRect::new(NSPoint::new(1728., -323.), NSSize::new(2560., 1440.));
        let on_second = NSPoint::new(2946., -267.);
        assert!(contains_point(second, on_second));
        assert!(!contains_point(primary, on_second));
        // Shared vertical edge: belongs to the right screen only.
        let edge = NSPoint::new(1728., 500.);
        assert!(contains_point(second, edge));
        assert!(!contains_point(primary, edge));
        // Top edge in, bottom edge out.
        assert!(contains_point(primary, NSPoint::new(10., 1117.)));
        assert!(!contains_point(primary, NSPoint::new(10., 0.)));
    }

    #[test]
    fn main_display_is_an_active_display() {
        // `None` without a window server connection (e.g. headless CI).
        if let Some(main) = MacDisplay::main() {
            assert!(MacDisplay::all().any(|display| display.0 == main.0), "{}", main.0);
        }
    }
}
