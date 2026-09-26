use rhai::EvalAltResult;

#[derive(Debug)]
pub enum RhaiBindError {
    Rhai(Box<EvalAltResult>),
    InvalidConstant {
        module: String,
        name: String,
        value: String,
    },
    GenericFunction {
        name: &'static str,
    },
    MissingForeignFunction {
        interface: &'static str,
        function: &'static str,
    },
    MissingForeignInstantiation {
        interface: &'static str,
        function: &'static str,
        mangled: String,
    },
    GenericForeignInterface {
        name: &'static str,
    },
    ReservedMethod {
        name: &'static str,
    },
    DuplicateConstructor {
        name: &'static str,
    },
    DuplicateMethod {
        name: String,
    },
    DuplicateEnumCase {
        enum_name: String,
        case: String,
    },
    DuplicateField {
        name: &'static str,
    },
    UnsupportedAsyncMethod {
        name: &'static str,
    },
    UnsupportedAsyncFunction {
        name: &'static str,
    },
    UnsupportedAsyncProperty {
        name: &'static str,
    },
    UnsupportedAsyncCall,
    UnsupportedDynFunction {
        name: &'static str,
        reason: &'static str,
    },
    UnsupportedBorrowedType {
        context: &'static str,
        name: &'static str,
    },
}

impl std::fmt::Display for RhaiBindError {
    #[allow(
        clippy::too_many_lines,
        reason = "one rendering arm per error kind; length tracks the error surface"
    )]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rhai(e) => write!(f, "rhai error: {e}"),
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
                     Rhai dispatches by name only and cannot distinguish instantiations \
                     (foreign dispatch can opt in via the `generics` feature)"
                )
            }
            Self::MissingForeignFunction {
                interface,
                function,
            } => {
                write!(
                    f,
                    "callbacks map for foreign interface `{interface}` has no \
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
                    "callbacks map for foreign interface `{interface}` has no \
                     function `{mangled}`: static dispatch of generic `{function}` \
                     needs one handler per declared instantiation"
                )
            }
            Self::GenericForeignInterface { name } => {
                write!(
                    f,
                    "foreign interface `{name}` is generic; enable the `generics` \
                     feature to dispatch instantiations through one Rhai function"
                )
            }
            Self::ReservedMethod { name } => {
                write!(
                    f,
                    "method `{name}` collides with an implicit method this backend \
                     registers for the declared trait; rename the method"
                )
            }
            Self::DuplicateConstructor { name } => {
                write!(
                    f,
                    "two constructors named `{name}` target one type; the later \
                     registration would silently overwrite the earlier"
                )
            }
            Self::DuplicateMethod { name } => {
                write!(
                    f,
                    "two callables resolve to the entry `{name}`; a static generic \
                     monomorph's mangled name must not collide with another \
                     registration"
                )
            }
            Self::DuplicateEnumCase { enum_name, case } => {
                write!(f, "enum `{enum_name}` case table already contains `{case}`")
            }
            Self::DuplicateField { name } => {
                write!(
                    f,
                    "field `{name}` is registered more than once (a computed property \
                     shares a name with a plain field)"
                )
            }
            Self::UnsupportedAsyncMethod { name } => {
                write!(
                    f,
                    "async method `{name}` cannot be bound: Rhai has no native async support"
                )
            }
            Self::UnsupportedAsyncFunction { name } => {
                write!(
                    f,
                    "async function `{name}` cannot be bound: Rhai has no native async support"
                )
            }
            Self::UnsupportedAsyncProperty { name } => {
                write!(
                    f,
                    "async property `{name}` cannot be bound: Rhai has no native async support"
                )
            }
            Self::UnsupportedAsyncCall => {
                write!(
                    f,
                    "async call (`AsyncCall`) cannot be bound: Rhai has no native async support"
                )
            }
            Self::UnsupportedDynFunction { name, reason } => {
                write!(
                    f,
                    "dyn function `{name}` cannot be bound in this configuration: {reason}"
                )
            }
            Self::UnsupportedBorrowedType { context, name } => {
                write!(
                    f,
                    "{context} `{name}` uses a borrowed type (`Cow`): Rhai has no borrow \
                     semantics and the value would be silently copied at every crossing; \
                     enable the `borrowed` feature to opt in to this behavior"
                )
            }
        }
    }
}

impl std::error::Error for RhaiBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rhai(e) => Some(e.as_ref()),
            Self::InvalidConstant { .. }
            | Self::GenericFunction { .. }
            | Self::MissingForeignFunction { .. }
            | Self::MissingForeignInstantiation { .. }
            | Self::GenericForeignInterface { .. }
            | Self::ReservedMethod { .. }
            | Self::DuplicateConstructor { .. }
            | Self::DuplicateMethod { .. }
            | Self::DuplicateEnumCase { .. }
            | Self::DuplicateField { .. }
            | Self::UnsupportedAsyncMethod { .. }
            | Self::UnsupportedAsyncFunction { .. }
            | Self::UnsupportedAsyncProperty { .. }
            | Self::UnsupportedAsyncCall
            | Self::UnsupportedDynFunction { .. }
            | Self::UnsupportedBorrowedType { .. } => None,
        }
    }
}

impl From<Box<EvalAltResult>> for RhaiBindError {
    fn from(e: Box<EvalAltResult>) -> Self {
        Self::Rhai(e)
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "consumed by `.map_err(call_error)` at each bridge call site"
)]
#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's error type is Box<EvalAltResult>; callers return Result<_, Box<EvalAltResult>>"
)]
pub(crate) fn call_error(e: haphe::ScriptCallError) -> Box<EvalAltResult> {
    runtime_error(&e)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "consumed by `.map_err(convert_error)` at each bridge call site"
)]
#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's error type is Box<EvalAltResult>; callers return Result<_, Box<EvalAltResult>>"
)]
pub(crate) fn convert_error(e: haphe::ScriptConvertError) -> Box<EvalAltResult> {
    runtime_error(&e)
}

#[allow(
    clippy::unnecessary_box_returns,
    reason = "Rhai's error type is Box<EvalAltResult>"
)]
fn runtime_error(e: &dyn std::fmt::Display) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        e.to_string().into(),
        rhai::Position::NONE,
    ))
}

#[derive(Debug)]
pub enum RhaiDeclError {
    DuplicateTypeName { name: String },
}

impl std::fmt::Display for RhaiDeclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTypeName { name } => {
                write!(
                    f,
                    "type name `{name}` is used by more than one struct/enum/alias"
                )
            }
        }
    }
}

impl std::error::Error for RhaiDeclError {}
