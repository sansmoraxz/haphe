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
                     Lua dispatches by name only and cannot distinguish instantiations"
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
        }
    }
}

impl From<mlua::Error> for LuaBindError {
    fn from(e: mlua::Error) -> Self {
        Self::Lua(e)
    }
}
