//! Frame pacing for macOS windows: each window is paced by a
//! `CAMetalDisplayLink` on its layer ([`MetalFrameSource`]), which hands out
//! the drawable each frame renders into.

use std::cell::Cell;
use std::cell::RefCell;

use objc2::AnyThread as _;
use objc2::DefinedClass as _;
use objc2::MainThreadMarker;
use objc2::MainThreadOnly;
use objc2::Message as _;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::runtime::NSObjectProtocol;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRunLoop;
use objc2_foundation::NSRunLoopCommonModes;
use objc2_quartz_core::CAMetalDisplayLink;
use objc2_quartz_core::CAMetalDisplayLinkDelegate;
use objc2_quartz_core::CAMetalDisplayLinkUpdate;
use objc2_quartz_core::CAMetalLayer;

/// A window's `CAMetalDisplayLink`, shared with GPUI's frame waker so that a
/// wake reaches the current link without taking the window's lock.
///
/// The link pauses after any update in which GPUI neither drew, presented,
/// nor asked for another frame, so an idle window acquires no drawables. GPUI
/// decides when frames are needed (including presenting after high-rate
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

    pub(crate) fn is_attached(&self) -> bool {
        self.source.borrow().is_some()
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

    /// Ends an update, pausing the link if GPUI neither used its drawable nor
    /// asked for another frame while it ran. Returns whether it paused.
    pub(crate) fn end_frame(&self, used_drawable: bool) -> bool {
        let pause = !used_drawable && !self.woken.get();
        if pause && let Some(source) = self.source.borrow().as_ref() {
            source.set_paused(true);
        }
        pause
    }
}

struct MetalDisplayLinkDelegateIvars {
    callback: Box<dyn Fn(&CAMetalDisplayLinkUpdate)>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MetalDisplayLinkDelegateIvars]
    struct MetalDisplayLinkDelegate;

    unsafe impl NSObjectProtocol for MetalDisplayLinkDelegate {}

    unsafe impl CAMetalDisplayLinkDelegate for MetalDisplayLinkDelegate {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn needs_update(&self, _link: &CAMetalDisplayLink, update: &CAMetalDisplayLinkUpdate) {
            // The callback may destroy its frame source, releasing our owner's retain.
            let delegate = self.retain();
            (delegate.ivars().callback)(update);
        }
    }
);

impl MetalDisplayLinkDelegate {
    fn new(main_thread: MainThreadMarker, callback: impl Fn(&CAMetalDisplayLinkUpdate) + 'static) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(MetalDisplayLinkDelegateIvars { callback: Box::new(callback) });
        // SAFETY: `NSObject`'s `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// A main-run-loop source of Metal drawables for one layer (macOS 14+).
///
/// The link hands out each frame's drawable early enough for the frame to
/// reach the display on time, where `nextDrawable` would wait for the
/// compositor to release one. Callbacks run on the main run loop because GPUI
/// renders on the foreground thread.
pub(crate) struct MetalFrameSource {
    link: Retained<CAMetalDisplayLink>,
    // The link holds its delegate weakly.
    _delegate: Retained<MetalDisplayLinkDelegate>,
    _main_thread: MainThreadMarker,
}

impl MetalFrameSource {
    /// Starts delivering updates for `layer`.
    pub(crate) fn new(
        layer: &CAMetalLayer,
        main_thread: MainThreadMarker,
        callback: impl Fn(&CAMetalDisplayLinkUpdate) + 'static,
    ) -> Self {
        let delegate = MetalDisplayLinkDelegate::new(main_thread, callback);
        let link = CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), layer);
        link.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The default frame rate range follows the display and system power
        // policy (Low Power Mode, thermal state), so it is left alone.
        // Common modes keep frames coming during live resize and other tracking loops.
        // SAFETY: the retained marker confines the source to the main thread,
        // and the main run loop outlives the source.
        unsafe {
            link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }
        Self { link, _delegate: delegate, _main_thread: main_thread }
    }

    /// A paused link delivers no updates and acquires no drawables.
    pub(crate) fn set_paused(&self, paused: bool) {
        self.link.setPaused(paused);
    }
}

impl Drop for MetalFrameSource {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::MetalFrames;

    #[test]
    fn metal_frames_pause_only_when_gpui_neither_draws_nor_asks_again() {
        let frames = MetalFrames::default();
        assert!(!frames.is_attached());

        frames.begin_frame();
        assert!(!frames.end_frame(true), "a frame that drew keeps the link running");

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
