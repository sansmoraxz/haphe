use crate::types::{GenericParam, TypeDescriptor};

/// How a generic function's registrations are dispatched at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dispatch {
    /// Callers name a declared instantiation; backends key dispatch on
    /// `(name, type_args)`. The default.
    #[default]
    Static,
    /// Declared `dyn`: one callable per name; the backend scans the
    /// registered instantiation candidates at call time and picks the one
    /// whose type arguments match the incoming values (see
    /// [`resolve_dyn_candidate`](crate::dispatch::resolve_dyn_candidate)).
    /// Compilation is unchanged — the same monomorph wrappers back both
    /// modes.
    Dyn,
}

/// A Rust function or method exposed to scripting languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionDescriptor<'a> {
    /// Function name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// If `Some`, this is a method with the given receiver type.
    pub receiver: Option<Receiver>,
    /// Generic parameters declared on the function itself; the signature may
    /// reference them via [`TypeDescriptor::GenericParam`], and each concrete
    /// use is listed in [`instantiations`](Self::instantiations).
    pub generic_params: &'a [GenericParam<'a>],
    /// Concrete instantiations of [`generic_params`](Self::generic_params):
    /// one entry per declared use, each carrying the type arguments in
    /// declaration order. Backends monomorphize the function once per entry.
    /// Empty for non-generic functions.
    pub instantiations: &'a [&'a [TypeDescriptor<'a>]],
    /// Runtime dispatch mode for the generic registrations.
    pub dispatch: Dispatch,
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
    /// Backends use this to generate specific exception or error types in
    /// their target language. `None` means use the backend's default error
    /// type.
    pub error_kind: Option<&'a str>,
    /// Whether the declared return was `Result<T, E>` — the descriptor's
    /// [`return_type`](Self::return_type) is `T` and calls may yield a
    /// [`Callee`](crate::ScriptCallError::Callee) error. Fallibility is explicit
    /// here; [`error_kind`](Self::error_kind) presence is NOT its proxy.
    pub fallible: bool,
}

/// Whether any function in the slice is `async`. Usable in `const` contexts.
pub const fn any_async(fns: &[FunctionDescriptor<'_>]) -> bool {
    let mut i = 0;
    while i < fns.len() {
        if fns[i].is_async {
            return true;
        }
        i += 1;
    }
    false
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamDescriptor<'a> {
    /// Parameter name.
    pub name: &'a str,
    /// The parameter's type.
    pub ty: &'a TypeDescriptor<'a>,
    /// How ownership is handled when this value crosses the FFI boundary.
    pub ownership: Ownership,
}
