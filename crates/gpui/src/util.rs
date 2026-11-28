use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

/// Blocks the current thread until the given future completes.
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    crate::executor::block(std::pin::pin!(future), None).expect("block without deadline always completes")
}

/// A helper trait for building complex objects with imperative conditionals in a fluent style.
pub trait FluentBuilder {
    /// Imperatively modify self with the given closure.
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U
    where
        Self: Sized,
    {
        f(self)
    }

    /// Conditionally modify self with the given closure.
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { this })
    }

    /// Conditionally modify self with the given closure.
    fn when_else(self, condition: bool, then: impl FnOnce(Self) -> Self, else_fn: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { else_fn(this) })
    }

    /// Conditionally unwrap and modify self with the given closure, if the given option is Some.
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if let Some(value) = option { then(this, value) } else { this })
    }
    /// Conditionally unwrap and modify self with the given closure, if the given option is None.
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if option.is_some() { this } else { then(this) })
    }
}

/// Increment the given atomic counter if it is not zero.
/// Return the new value of the counter.
pub(crate) fn atomic_incr_if_not_zero(counter: &AtomicUsize) -> usize {
    let mut loaded = counter.load(SeqCst);
    loop {
        if loaded == 0 {
            return 0;
        }
        match counter.compare_exchange_weak(loaded, loaded + 1, SeqCst, SeqCst) {
            Ok(x) => return x + 1,
            Err(actual) => loaded = actual,
        }
    }
}

/// Rounds to the nearest integer with 0.5 ties toward zero.
#[inline]
pub(crate) fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

#[inline]
pub(crate) fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

#[inline]
pub(crate) fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0.0 { 0.0 } else { round_to_device_pixel(logical.max(0.0), scale_factor).max(1.0) }
}

#[inline]
pub(crate) fn ceil_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    (logical * scale_factor).ceil()
}
