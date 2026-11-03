//! Frame pacing for macOS windows: each window is paced by a
//! `CAMetalDisplayLink` on its layer ([`MetalFrameSource`]), which hands out
//! the drawable each frame renders into.

use std::cell::Cell;
use std::cell::RefCell;

use gpui_apple::display_link::MetalFrameSource;

/// A window's `CAMetalDisplayLink`, shared with GPUI's frame waker so that a
/// wake reaches the current link without taking the window's lock.
///
/// The link pauses after any update in which GPUI neither drew, presented,
/// nor asked for another frame, so an idle window acquires no drawables. GPUI
/// decides when frames are needed (including presenting after high-rate
/// input); this only stops the link between those decisions.
#[derive(Default)]
pub struct MetalFrames {
    source: RefCell<Option<MetalFrameSource>>,
    /// Whether GPUI asked for a frame since the current update began.
    woken: Cell<bool>,
}

impl MetalFrames {
    pub fn replace(&self, source: Option<MetalFrameSource>) {
        *self.source.borrow_mut() = source;
    }

    pub fn is_attached(&self) -> bool {
        self.source.borrow().is_some()
    }

    pub fn wake(&self) {
        self.woken.set(true);
        // A source being replaced starts unpaused, so skipping it is safe.
        if let Ok(source) = self.source.try_borrow()
            && let Some(source) = source.as_ref()
        {
            source.set_paused(false);
        }
    }

    pub fn begin_frame(&self) {
        self.woken.set(false);
    }

    /// Ends an update, pausing the link if GPUI neither used its drawable nor
    /// asked for another frame while it ran. Returns whether it paused.
    pub fn end_frame(&self, used_drawable: bool) -> bool {
        let pause = !used_drawable && !self.woken.get();
        if pause && let Some(source) = self.source.borrow().as_ref() {
            source.set_paused(true);
        }
        pause
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
