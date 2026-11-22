//! Frame pacing for macOS windows: each window is paced by its view's
//! `CADisplayLink` ([`MetalFrameSource`]), which follows the display the
//! window is on.

use std::cell::Cell;
use std::cell::RefCell;

use objc2::DefinedClass as _;
use objc2::MainThreadMarker;
use objc2::MainThreadOnly;
use objc2::Message as _;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::NSObject;
use objc2::runtime::NSObjectProtocol;
use objc2::sel;
use objc2_foundation::NSRunLoop;
use objc2_foundation::NSRunLoopCommonModes;
use objc2_quartz_core::CADisplayLink;

/// A window's display link, shared with GPUI's frame waker so that a wake
/// reaches the current link without taking the window's lock.
///
/// The link pauses after any update in which GPUI neither drew, presented,
/// nor asked for another frame, so an idle window does no per-refresh work.
/// GPUI decides when frames are needed (including presenting after high-rate
/// input); this only stops the link between those decisions.
#[derive(Default)]
pub(crate) struct MetalFrames {
    source: RefCell<Option<MetalFrameSource>>,
    /// Whether GPUI asked for a frame since the current update began.
    woken: Cell<bool>,
}

impl MetalFrames {
    pub(crate) fn replace(&self, source: Option<MetalFrameSource>) {
        *self.source.borrow_mut() = source;
    }

    pub(crate) fn wake(&self) {
        self.woken.set(true);
        // A source being replaced starts unpaused, so skipping it is safe.
        if let Ok(source) = self.source.try_borrow()
            && let Some(source) = source.as_ref()
        {
            source.set_paused(false);
        }
    }

    pub(crate) fn begin_frame(&self) {
        self.woken.set(false);
    }

    /// Ends an update, pausing the link if GPUI neither showed a frame nor
    /// asked for another while it ran. Returns whether it paused.
    pub(crate) fn end_frame(&self, presented: bool) -> bool {
        let pause = !presented && !self.woken.get();
        if pause && let Some(source) = self.source.borrow().as_ref() {
            source.set_paused(true);
        }
        pause
    }
}

struct DisplayLinkTargetIvars {
    callback: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DisplayLinkTargetIvars]
    struct DisplayLinkTarget;

    unsafe impl NSObjectProtocol for DisplayLinkTarget {}

    impl DisplayLinkTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {
            // The callback may destroy its frame source, releasing the link's retain.
            let target = self.retain();
            (target.ivars().callback)();
        }
    }
);

impl DisplayLinkTarget {
    fn new(main_thread: MainThreadMarker, callback: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(DisplayLinkTargetIvars { callback: Box::new(callback) });
        // SAFETY: `NSObject`'s `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// A main-run-loop source of frame callbacks for one view (macOS 14+).
///
/// Each callback targets the next refresh of the view's display. Callbacks
/// run on the main run loop because GPUI renders on the foreground thread.
pub(crate) struct MetalFrameSource {
    link: Retained<CADisplayLink>,
    _main_thread: MainThreadMarker,
}

impl MetalFrameSource {
    /// Starts delivering updates for `view`, an `NSView`.
    pub(crate) fn new(view: &AnyObject, main_thread: MainThreadMarker, callback: impl Fn() + 'static) -> Self {
        let target = DisplayLinkTarget::new(main_thread, callback);
        // SAFETY: `view` is an `NSView` and `tick:` takes the link.
        let link: Retained<CADisplayLink> =
            unsafe { msg_send![view, displayLinkWithTarget: &*target, selector: sel!(tick:)] };
        // The default frame rate range follows the display and system power
        // policy (Low Power Mode, thermal state), so it is left alone.
        // Common modes keep frames coming during live resize and other tracking loops.
        // SAFETY: the retained marker confines the source to the main thread,
        // and the main run loop outlives the source.
        unsafe {
            link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }
        Self { link, _main_thread: main_thread }
    }

    /// A paused link delivers no updates.
    pub(crate) fn set_paused(&self, paused: bool) {
        self.link.setPaused(paused);
    }
}

impl Drop for MetalFrameSource {
    fn drop(&mut self) {
        // The link retains its target until invalidated.
        self.link.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::MetalFrames;

    #[test]
    fn metal_frames_pause_only_when_gpui_neither_draws_nor_asks_again() {
        let frames = MetalFrames::default();
        frames.begin_frame();
        assert!(!frames.end_frame(true), "a frame that was shown keeps the link running");

        frames.begin_frame();
        frames.wake();
        assert!(!frames.end_frame(false), "a wake during the update, like a throttled frame's retry, keeps it running");

        frames.wake();
        frames.begin_frame();
        assert!(frames.end_frame(false), "a wake before the update was served by it");

        frames.begin_frame();
        assert!(frames.end_frame(false), "an idle update pauses the link");
    }
}
