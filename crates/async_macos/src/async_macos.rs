//! Async IO for macOS on GCD dispatch sources: no reactor, reaper, or
//! blocking-pool threads.

pub mod command;
pub mod fs;
mod gcd;
pub mod net;

use std::future::poll_fn;
use std::task::Poll;

pub use gcd::unblock;

/// Yields to the executor once so other tasks can run.
pub async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::Context;
    use std::task::Poll;
    use std::task::Waker;

    #[test]
    fn yield_now_is_pending_exactly_once() {
        let mut future = pin!(super::yield_now());
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
    }
}
