use crate::types::TypeDescriptor;

/// A Rust function or method exposed to scripting languages.
#[derive(Debug, Clone, Copy)]
pub struct FunctionDescriptor<'a> {
    /// Function name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// If `Some`, this is a method with the given receiver type.
    pub receiver: Option<Receiver>,
    /// Positional parameters (excluding `self`).
    pub params: &'a [ParamDescriptor<'a>],
    /// The return type of the function.
    pub return_type: &'a TypeDescriptor<'a>,
    /// How ownership of the return value is handled at the FFI boundary.
    pub return_ownership: Ownership,
    /// Whether this function is `async`.
    pub is_async: bool,
    /// Optional error class hint for backend exception mapping.
    ///
    /// Backends use this to generate specific exception types: pyo3 →
    /// `PyValueError`, wasm → specific `Error` subclass, etc. `None` means
    /// use the backend's default error type.
    pub error_kind: Option<&'a str>,
}

/// How a method receives `self`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `self` — takes ownership.
    Owned,
    /// `&self` — shared reference.
    Ref,
    /// `&mut self` — exclusive reference.
    RefMut,
}

/// Ownership semantics for values crossing the FFI boundary.
///
/// Backends use this to generate optimal wrapping code for function
/// parameters and return values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// Ownership is transferred across the boundary.
    /// The source side loses access; the receiving side is responsible for drop.
    Owned,
    /// A shared reference is passed. The source retains ownership.
    Ref,
    /// An exclusive reference is passed. The source retains ownership.
    RefMut,
    /// A clone is made at the boundary. Both sides hold independent values.
    /// This is the most common case for scripting-language FFI where true
    /// borrows are not expressible.
    Clone,
}

/// A single function parameter.
#[derive(Debug, Clone, Copy)]
pub struct ParamDescriptor<'a> {
    /// Parameter name.
    pub name: &'a str,
    /// The parameter's type.
    pub ty: &'a TypeDescriptor<'a>,
    /// How ownership is handled when this value crosses the FFI boundary.
    pub ownership: Ownership,
}
