//! First-class stream and future value types, behind the `streams` and
//! `futures` features respectively.
//!
//! These are real, runnable values (boxed trait objects), not markers: a
//! described function returning [`Stream<T>`] actually produces one. The
//! derive macros fold them to [`TypeDescriptor::Stream`] /
//! [`TypeDescriptor::Future`], which backends map to their native streaming
//! and deferred-value constructs.

use core::pin::Pin;

use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

#[cfg(feature = "streams")]
/// A stream of `T` values crossing the script boundary.
///
/// Wraps a boxed [`futures_core::Stream`], so any stream implementation can
/// be handed over via [`Stream::new`].
pub struct Stream<T>(pub Pin<Box<dyn futures_core::Stream<Item = T> + Send>>);

#[cfg(feature = "streams")]
impl<T> Stream<T> {
    /// Boxes any `Send` stream of `T`.
    pub fn new(inner: impl futures_core::Stream<Item = T> + Send + 'static) -> Self {
        Self(Box::pin(inner))
    }
}

#[cfg(feature = "streams")]
impl<T> futures_core::Stream for Stream<T> {
    type Item = T;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<T>> {
        self.0.as_mut().poll_next(cx)
    }
}

#[cfg(feature = "streams")]
impl<T: HapheType> HapheType for Stream<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Stream(&T::DESCRIPTOR);
}

#[cfg(feature = "futures")]
/// A deferred `T` value crossing the script boundary.
///
/// Wraps a boxed [`core::future::Future`], so any future can be handed over
/// via [`Future::new`].
pub struct Future<T>(pub Pin<Box<dyn core::future::Future<Output = T> + Send>>);

#[cfg(feature = "futures")]
impl<T> Future<T> {
    /// Boxes any `Send` future of `T`.
    pub fn new(inner: impl core::future::Future<Output = T> + Send + 'static) -> Self {
        Self(Box::pin(inner))
    }
}

#[cfg(feature = "futures")]
impl<T> core::future::Future for Future<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<T> {
        self.0.as_mut().poll(cx)
    }
}

#[cfg(feature = "futures")]
impl<T: HapheType> HapheType for Future<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Future(&T::DESCRIPTOR);
}
