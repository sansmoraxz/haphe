use crate::function::FunctionDescriptor;
use crate::types::{ThreadSafety, TypeId};

/// A group of host-supplied functions that Rust calls out to.
///
/// This is the reverse of [`FunctionDescriptor`]s in a
/// [`ModuleDescriptor`](crate::ModuleDescriptor): the signature is declared in
/// Rust, but the body is provided by whoever embeds or hosts the program.
/// Backends produce a handle that dispatches each call into the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeignInterfaceDescriptor<'a> {
    /// Unique registry identifier (the declaring trait's Rust path).
    pub id: TypeId<'a>,
    /// Exposed interface name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The host-supplied functions. Every entry has
    /// `receiver: Some(Receiver::Ref)`.
    pub functions: &'a [FunctionDescriptor<'a>],
    /// Thread-safety markers of the generated handle.
    pub thread_safety: ThreadSafety,
}
