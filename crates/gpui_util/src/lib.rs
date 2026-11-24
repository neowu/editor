use core::panic;
use std::ffi::OsStr;
use std::fmt;
use std::hash::BuildHasher;
use std::hash::Hasher;
use std::ops::AddAssign;
use std::panic::Location;
use std::pin::Pin;
use std::process::Command;
use std::task::Context;
use std::task::Poll;

pub fn new_std_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

#[inline]
pub const fn is_utf8_char_boundary(u8: u8) -> bool {
    // This is bit magic equivalent to: b < 128 || b >= 192
    u8.cast_signed() >= -0x40
}

pub fn post_inc<T: From<u8> + AddAssign<T> + Copy>(value: &mut T) -> T {
    let prev = *value;
    *value += T::from(1);
    prev
}

/// The bits of `value` with the sign of zero dropped, so a hash built on them agrees with
/// `f32` equality, where `0.0 == -0.0`.
pub fn canonical_float_bits(value: f32) -> u32 {
    // `-0.0 + 0.0` is `0.0`; every other value is unchanged.
    (value + 0.0).to_bits()
}

pub trait ResultExt<E> {
    type Ok;

    fn log_err(self) -> Option<Self::Ok>;
    fn log_with_level(self, level: log::Level) -> Option<Self::Ok>;
}

impl<T, E> ResultExt<E> for Result<T, E>
where
    E: fmt::Display,
{
    type Ok = T;

    #[track_caller]
    fn log_err(self) -> Option<T> {
        self.log_with_level(log::Level::Error)
    }

    #[track_caller]
    fn log_with_level(self, level: log::Level) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                log_error_with_caller(*Location::caller(), format_args!("{error:#}"), level);
                None
            }
        }
    }
}

#[inline(never)]
fn log_error_with_caller(caller: panic::Location<'_>, arguments: fmt::Arguments<'_>, level: log::Level) {
    let file = caller.file();

    // In this codebase all crates reside in a `crates` directory,
    // so discard the prefix up to that segment to find the crate name
    let file = file.split_once("crates/");
    let target = file.as_ref().and_then(|(_, s)| s.split_once("/src/"));

    let module_path = target.map(|(krate, module)| {
        if module.starts_with(krate) {
            module.trim_end_matches(".rs").replace('/', "::")
        } else {
            krate.to_owned() + "::" + &module.trim_end_matches(".rs").replace('/', "::")
        }
    });
    let file = file.map(|(_, file)| format!("crates/{file}"));
    log::logger().log(
        &log::Record::builder()
            .target(module_path.as_deref().unwrap_or(""))
            .module_path(file.as_deref())
            .args(arguments)
            .file(Some(caller.file()))
            .line(Some(caller.line()))
            .level(level)
            .build(),
    );
}

pub trait TryFutureExt {
    fn log_err(self) -> LogErrorFuture<Self>
    where
        Self: Sized;

    fn log_tracked_err(self, location: panic::Location<'static>) -> LogErrorFuture<Self>
    where
        Self: Sized;
}

impl<F, T, E> TryFutureExt for F
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    #[track_caller]
    fn log_err(self) -> LogErrorFuture<Self>
    where
        Self: Sized,
    {
        let location = Location::caller();
        LogErrorFuture(self, log::Level::Error, *location)
    }

    fn log_tracked_err(self, location: panic::Location<'static>) -> LogErrorFuture<Self>
    where
        Self: Sized,
    {
        LogErrorFuture(self, log::Level::Error, location)
    }
}

#[must_use]
pub struct LogErrorFuture<F>(F, log::Level, panic::Location<'static>);

impl<F, T, E> Future for LogErrorFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: fmt::Display,
{
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let level = self.1;
        let location = self.2;
        let inner = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().0) };
        match inner.poll(cx) {
            Poll::Ready(output) => Poll::Ready(match output {
                Ok(output) => Some(output),
                Err(error) => {
                    log_error_with_caller(location, format_args!("{error:#}"), level);
                    None
                }
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct Deferred<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Deferred<F> {
    /// Drop without running the deferred function.
    pub fn abort(mut self) {
        self.0.take();
    }
}

impl<F: FnOnce()> Drop for Deferred<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// Run the given function when the returned value is dropped (unless it's cancelled).
#[must_use]
pub const fn defer<F: FnOnce()>(f: F) -> Deferred<F> {
    Deferred(Some(f))
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeIdHashBuilder;

impl BuildHasher for TypeIdHashBuilder {
    type Hasher = TypeIdHasher;

    fn build_hasher(&self) -> Self::Hasher {
        TypeIdHasher::default()
    }
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeIdHasher {
    value: u64,
}

impl Hasher for TypeIdHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // TypeId should only hash its first 8 bytes
        if let Some(&array) = bytes.first_chunk::<8>() {
            self.value = u64::from_le_bytes(array);
        } else {
            log::error!("expected a 64-bit value, did you use this hasher with something other than a TypeId?");
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_float_bits_drops_only_the_sign_of_zero() {
        assert_eq!(canonical_float_bits(-0.0), 0.0_f32.to_bits());
        assert_eq!(canonical_float_bits(0.0), 0.0_f32.to_bits());
        for value in [1.5_f32, -1.5, f32::MIN_POSITIVE, -f32::MIN_POSITIVE, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(canonical_float_bits(value), value.to_bits());
        }
    }
}
