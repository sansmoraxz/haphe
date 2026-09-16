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
    /// A generic foreign interface was used without the `generics` feature.
    GenericForeignInterface {
        /// The interface's exposed name.
        name: &'static str,
    },
    /// A user-declared method collides with a method this backend registers
    /// implicitly (the `iter` method on iterable types).
    ReservedMethod {
        /// The colliding method name.
        name: &'static str,
    },
    /// An operator was declared that the configured Lua version has no
    /// metamethod for (the bitwise family requires Lua 5.3+; Lua 5.1/5.2,
    /// LuaJIT, and Luau cannot represent it).
    UnsupportedOperator {
        /// The operator's Rust trait method name (`bitand`, `shl`, ...).
        op: &'static str,
    },
}

impl std::fmt::Display for LuaBindError {
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
                     backend registers on iterable types; rename the method"
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
            Self::InvalidConstant { .. } => None,
            Self::GenericFunction { .. } => None,
            Self::MissingForeignFunction { .. } => None,
            Self::GenericForeignInterface { .. } => None,
            Self::ReservedMethod { .. } => None,
            Self::UnsupportedOperator { .. } => None,
        }
    }
}

impl From<mlua::Error> for LuaBindError {
    fn from(e: mlua::Error) -> Self {
        Self::Lua(e)
    }
}
