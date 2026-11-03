use objc2::AnyThread as _;
use objc2::DefinedClass;
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
pub struct MetalFrameSource {
    link: Retained<CAMetalDisplayLink>,
    // The link holds its delegate weakly.
    _delegate: Retained<MetalDisplayLinkDelegate>,
    _main_thread: MainThreadMarker,
}

impl MetalFrameSource {
    /// Starts delivering updates for `layer`.
    pub fn new(
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
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Self { link, _delegate: delegate, _main_thread: main_thread }
    }

    /// A paused link delivers no updates and acquires no drawables.
    pub fn set_paused(&self, paused: bool) {
        self.link.setPaused(paused);
    }
}

impl Drop for MetalFrameSource {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}
