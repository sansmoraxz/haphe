/// Errors that can occur during Lua binding.
#[derive(Debug)]
pub enum LuaBindError {
    /// An error from the mlua runtime.
    Lua(mlua::Error),
    /// A constant value could not be parsed into a Lua value.
    InvalidConstant {
        /// The module containing the constant.
        module: String,
        /// The constant's name.
        name: String,
        /// The raw string value that could not be parsed.
        value: String,
    },
    /// A generic function instantiation was registered; Lua dispatches by
    /// name only, so monomorphized instantiations cannot be told apart.
    GenericFunction {
        /// The function's exposed name.
        name: &'static str,
    },
    /// A foreign interface's callbacks table lacks a function (the field is
    /// missing or not a Lua function).
    MissingForeignFunction {
        /// The foreign interface's exposed name.
        interface: &'static str,
        /// The missing function's exposed name.
        function: &'static str,
    },
    /// A statically dispatched generic foreign function's callbacks table
    /// lacks the handler for one of its declared instantiations (the field
    /// under the mangled monomorph name is missing or not a Lua function).
    MissingForeignInstantiation {
        /// The foreign interface's exposed name.
        interface: &'static str,
        /// The generic function's exposed name.
        function: &'static str,
        /// The missing handler's mangled monomorph name.
        mangled: String,
    },
    /// A generic foreign interface was used without the `generics` feature.
    GenericForeignInterface {
        /// The interface's exposed name.
        name: &'static str,
    },
    /// A user-declared method collides with a method this backend registers
    /// implicitly (the `iter` method on iterable types, `hash` for `Hash`,
    /// `debug` for `Debug`).
    ReservedMethod {
        /// The colliding method name.
        name: &'static str,
    },
    /// Two constructors share a name on one type's table (e.g. a declared
    /// constructor named `default` next to the implicit one from
    /// `traits(Default)`); the later registration would silently overwrite
    /// the earlier.
    DuplicateConstructor {
        /// The colliding constructor name.
        name: &'static str,
    },
    /// Two callables resolve to one entry (metatable method or module-table
    /// function) — a static generic monomorph's mangled name collided with
    /// another registration; the later would silently overwrite the earlier.
    DuplicateMethod {
        /// The colliding (mangled) name.
        name: String,
    },
    /// Two unit cases of an enum resolve to the same name in its Lua case
    /// table.
    DuplicateEnumCase {
        /// The enum's exposed name.
        enum_name: String,
        /// The colliding case name.
        case: String,
    },
    /// An operator was declared that the configured Lua version has no
    /// metamethod for (the bitwise family requires Lua 5.3+; Lua 5.1/5.2,
    /// `LuaJIT`, and Luau cannot represent it).
    UnsupportedOperator {
        /// The operator's Rust trait method name (`bitand`, `shl`, ...).
        op: &'static str,
    },
    /// An `AsyncCall` declaration cannot be represented in this
    /// configuration.
    UnsupportedAsyncCall {
        /// What is missing or conflicting.
        reason: &'static str,
    },
    /// A type declares both `Call` and `AsyncCall`, but Lua has exactly one
    /// `__call` metamethod — the binding is ambiguous.
    AmbiguousCall,
    /// An `async` method cannot be represented in this configuration.
    UnsupportedAsyncMethod {
        /// The method's exposed name.
        name: &'static str,
        /// What is missing or conflicting.
        reason: &'static str,
    },
    /// An async free function cannot be registered in this configuration.
    /// An async computed property was declared, but mlua exposes no async
    /// field accessors — declare an async method for awaited access.
    UnsupportedAsyncProperty { name: &'static str },
    /// A computed property shares a name with a plain field; the later
    /// registration would silently shadow the earlier one.
    DuplicateField { name: &'static str },
    UnsupportedAsyncFunction {
        /// The function's exposed name.
        name: &'static str,
        /// What is missing or conflicting.
        reason: &'static str,
    },
    /// A `dyn`-dispatched generic function cannot be bound in this
    /// configuration.
    UnsupportedDynFunction {
        /// The function's exposed name.
        name: &'static str,
        /// What is missing or conflicting.
        reason: &'static str,
    },
}

impl std::fmt::Display for LuaBindError {
    #[allow(
        clippy::too_many_lines,
        reason = "one rendering arm per error kind; length tracks the error surface"
    )]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lua(e) => write!(f, "lua error: {e}"),
            Self::InvalidConstant {
                module,
                name,
                value,
            } => {
                write!(
                    f,
                    "module `{module}`: constant `{name}` has unparseable value `{value}`"
                )
            }
            Self::GenericFunction { name } => {
                write!(
                    f,
                    "generic function `{name}` is not supported by this backend: \
                     Lua dispatches by name only and cannot distinguish instantiations \
                     (foreign dispatch can opt in via the `generics` feature)"
                )
            }
            Self::MissingForeignFunction {
                interface,
                function,
            } => {
                write!(
                    f,
                    "callbacks table for foreign interface `{interface}` has no \
                     function `{function}`"
                )
            }
            Self::MissingForeignInstantiation {
                interface,
                function,
                mangled,
            } => {
                write!(
                    f,
                    "callbacks table for foreign interface `{interface}` has no \
                     function `{mangled}`: static dispatch of generic `{function}` \
                     needs one handler per declared instantiation"
                )
            }
            Self::GenericForeignInterface { name } => {
                write!(
                    f,
                    "foreign interface `{name}` is generic; enable the `generics` \
                     feature to dispatch instantiations through one Lua function"
                )
            }
            Self::ReservedMethod { name } => {
                write!(
                    f,
                    "method `{name}` collides with the implicit `{name}` method this \
                     backend registers for the declared trait; rename the method"
                )
            }
            Self::DuplicateEnumCase { enum_name, case } => {
                write!(f, "enum `{enum_name}` case table already contains `{case}`")
            }
            Self::DuplicateConstructor { name } => {
                write!(
                    f,
                    "two constructors named `{name}` target one type table; the later \
                     registration would silently overwrite the earlier (note that \
                     `traits(Default)` registers an implicit `default` constructor)"
                )
            }
            Self::DuplicateMethod { name } => {
                write!(
                    f,
                    "two callables resolve to the entry `{name}`; a static generic \
                     monomorph's mangled name must not collide with another \
                     registration — rename it or adjust the instantiations"
                )
            }
            Self::UnsupportedAsyncCall { reason } => {
                write!(
                    f,
                    "async call (`AsyncCall`) cannot be bound in this configuration: {reason}"
                )
            }
            Self::UnsupportedAsyncMethod { name, reason } => {
                write!(
                    f,
                    "async method `{name}` cannot be bound in this configuration: {reason}"
                )
            }
            Self::UnsupportedAsyncFunction { name, reason } => {
                write!(
                    f,
                    "async function `{name}` cannot be bound in this configuration: {reason}"
                )
            }
            Self::UnsupportedDynFunction { name, reason } => {
                write!(
                    f,
                    "dyn function `{name}` cannot be bound in this configuration: {reason}"
                )
            }
            Self::UnsupportedAsyncProperty { name } => {
                write!(
                    f,
                    "async property `{name}` cannot be bound: mlua exposes no async field \
                     accessors; declare an async method for awaited access"
                )
            }
            Self::DuplicateField { name } => {
                write!(
                    f,
                    "field `{name}` is registered more than once (a computed property shares \
                     a name with a plain field)"
                )
            }
            Self::AmbiguousCall => {
                write!(
                    f,
                    "type declares both `Call` and `AsyncCall`, but Lua has exactly one \
                     `__call` metamethod; declare one of them"
                )
            }
            Self::UnsupportedOperator { op } => {
                write!(
                    f,
                    "operator `{op}` has no metamethod in the configured Lua version: \
                     the bitwise family requires Lua 5.3+ (not available on Lua \
                     5.1/5.2, LuaJIT, or Luau); floor division (`//`) requires \
                     Lua 5.3+ or Luau"
                )
            }
        }
    }
}

impl std::error::Error for LuaBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lua(e) => Some(e),
            _ => None,
        }
    }
}

impl From<mlua::Error> for LuaBindError {
    fn from(e: mlua::Error) -> Self {
        Self::Lua(e)
    }
}

/// Maps a bridge call error onto the mlua error a wrapper raises. `Convert`
/// stays a plain runtime string (boundary misuse rendered in place); `Callee`
/// crosses as an EXTERNAL error carrying the [`haphe::ScriptCallError`]
/// intact — `tostring(e)` in Lua still renders "Kind: message" (mlua's
/// external-error `Display` is a passthrough), and the structured form
/// (message, kind, type, chain) is recovered by the `haphe_error` global
/// installed at bind time.
pub(crate) fn call_error(e: haphe::ScriptCallError) -> mlua::Error {
    match e {
        haphe::ScriptCallError::Convert(_) => mlua::Error::runtime(e.to_string()),
        callee @ haphe::ScriptCallError::Callee { .. } => mlua::Error::external(callee),
    }
}

/// The `haphe_error` global: decodes a caught callee error value into a table.
///
/// ```lua
/// local ok, e = pcall(function() return gauge:checked_add(m) end)
/// local info = haphe_error(e)
/// -- info.message, info.kind (or nil), info.type, info.chain[1]...
/// ```
///
/// Returns `nil` for anything that is not a callee error (plain string errors,
/// conversion errors, foreign values).
pub(crate) fn error_info(lua: &mlua::Lua, value: mlua::Value) -> mlua::Result<mlua::Value> {
    let mlua::Value::Error(err) = value else {
        return Ok(mlua::Value::Nil);
    };
    let Some(call_err) = err.downcast_ref::<haphe::ScriptCallError>() else {
        return Ok(mlua::Value::Nil);
    };
    let haphe::ScriptCallError::Callee {
        kind, type_name, ..
    } = call_err
    else {
        return Ok(mlua::Value::Nil);
    };
    let info = lua.create_table()?;
    info.set("message", call_err.message())?;
    if let Some(kind) = kind {
        info.set("kind", *kind)?;
    }
    info.set("type", *type_name)?;
    info.set("chain", call_err.cause_chain())?;
    Ok(mlua::Value::Table(info))
}
