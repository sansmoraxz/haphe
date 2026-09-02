//! Compile-time verification helpers used by macro-generated code.
//!
//! Not part of the public API.

/// Verifies a `#[script(thread_safety = send_sync)]` declaration.
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `thread_safety = send_sync` but is not `Send + Sync`",
    note = "declare `thread_safety = send` or `thread_safety = none`, or make `{Self}` thread-safe"
)]
pub trait DeclaredSendSync {}
impl<T: Send + Sync + ?Sized> DeclaredSendSync for T {}

/// Verifies a `#[script(thread_safety = send)]` declaration.
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `thread_safety = send` but is not `Send`",
    note = "declare `thread_safety = none`, or make `{Self}` sendable"
)]
pub trait DeclaredSend {}
impl<T: Send + ?Sized> DeclaredSend for T {}

/// Verifies a `#[script(bytes)]` field/parameter override.
#[diagnostic::on_unimplemented(
    message = "`#[script(bytes)]` requires a byte-slice-shaped type, found `{Self}`",
    note = "supported types: `Vec<u8>`, `&[u8]`, `[u8; N]`, `Box<[u8]>`"
)]
pub trait BytesLike {}
impl BytesLike for Vec<u8> {}
impl BytesLike for [u8] {}
impl<const N: usize> BytesLike for [u8; N] {}
impl<T: BytesLike + ?Sized> BytesLike for &T {}
impl<T: BytesLike + ?Sized> BytesLike for &mut T {}
impl<T: BytesLike + ?Sized> BytesLike for Box<T> {}

/// Reverse handshake for `#[script] impl` blocks: implemented by
/// `#[derive(Script)]` when the type declares `#[script(methods)]`, asserted
/// by the `#[script]` impl-block macro. Guarantees an annotated impl block is
/// never silently dropped from the type's descriptor.
#[diagnostic::on_unimplemented(
    message = "this `#[script] impl` block is not referenced by `{Self}`'s descriptor",
    note = "add `methods` to the `#[script(...)]` attribute on `{Self}`'s `#[derive(Script)]`"
)]
pub trait HasScriptMethods {}

/// Const helper behind `#[script(bytes)]` verification.
pub const fn assert_bytes_like<T: BytesLike + ?Sized>() {}
