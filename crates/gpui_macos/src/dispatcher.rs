use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Duration;

use async_task::Runnable;
use dispatch2::DispatchQueue;
use dispatch2::DispatchQueueGlobalPriority;
use dispatch2::DispatchTime;
use dispatch2::GlobalQueueIdentifier;
use gpui::PlatformDispatcher;
use gpui::Priority;
use gpui::RunnableMeta;
use objc2_foundation::NSThread;

pub(crate) struct AppleDispatcher;

impl AppleDispatcher {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl Default for AppleDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformDispatcher for AppleDispatcher {
    fn is_main_thread(&self) -> bool {
        NSThread::isMainThread_class()
    }

    fn dispatch(&self, runnable: Runnable<RunnableMeta>, priority: Priority) {
        let context = runnable.into_raw().as_ptr().cast::<c_void>();

        let queue_priority = match priority {
            Priority::High => DispatchQueueGlobalPriority::High,
            Priority::Medium => DispatchQueueGlobalPriority::Default,
            Priority::Low => DispatchQueueGlobalPriority::Low,
        };

        unsafe {
            DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(queue_priority))
                .exec_async_f(context, trampoline);
        }
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable<RunnableMeta>) {
        let context = runnable.into_raw().as_ptr().cast::<c_void>();
        unsafe {
            DispatchQueue::main().exec_async_f(context, trampoline);
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable<RunnableMeta>) {
        let context = runnable.into_raw().as_ptr().cast::<c_void>();
        let queue = DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::High));
        let when = DispatchTime::NOW.time(duration.as_nanos() as i64);
        unsafe {
            DispatchQueue::exec_after_f(when, &queue, context, trampoline);
        }
    }
}

extern "C" fn trampoline(context: *mut c_void) {
    let runnable = unsafe { Runnable::<RunnableMeta>::from_raw(NonNull::new_unchecked(context.cast::<()>())) };

    runnable.run();
}
