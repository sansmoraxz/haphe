//! LuaLS/EmmyLua declaration stubs: a `---@meta` definition file for
//! [lua-language-server](https://luals.github.io/), generated from a haphe
//! registry so editors see the same surface the runtime binder exposes.
//!
//! Mapping decisions (mirroring runtime behavior, verbatim names):
//! - structs → `---@class` with `---@field`s (readonly noted in the field
//!   description), colon-syntax methods, constructors as dot functions on
//!   the module's type table, operator traits as `---@operator` lines;
//! - unit-only enums → `---@alias Name "Case"|...` with declared case names
//!   (payload enums are an opaque `---@class`; enum methods are not exposed
//!   by the runtime binder and are omitted here too);
//! - type aliases → `---@alias` to the underlying type;
//! - generic types → one erased class (as the runtime exposes), with
//!   generic parameters typed `any`;
//! - foreign interfaces → a `---@class` describing the callbacks table a
//!   script must supply;
//! - `Result<T, E>` returns render as `T` (failures raise Lua errors).

use std::fmt::Write as _;

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, EnumDescriptor,
    ForeignInterfaceDescriptor, FunctionDescriptor, GeneratedFile, GeneratedOutput,
    ModuleDescriptor, PrimitiveType, Receiver, StructDescriptor, TypeDescriptor, ValidatedRegistry,
    VariantKind,
};

use crate::LuaBinder;

/// Generates a LuaLS `---@meta` declaration stub from a registry.
#[derive(Debug, Clone)]
pub struct LuaDeclGenerator {
    path: String,
}

impl LuaDeclGenerator {
    /// Creates a generator writing to `definitions.lua`.
    pub fn new() -> Self {
        Self {
            path: "definitions.lua".to_string(),
        }
    }

    /// Sets the generated file's path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

impl Default for LuaDeclGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors produced while generating declaration stubs.
#[derive(Debug)]
pub enum LuaDeclError {
    /// Two registered types share an exposed name; a flat stub file cannot
    /// declare both.
    DuplicateTypeName {
        /// The shared exposed name.
        name: String,
    },
    /// A generic module function; the runtime binder rejects these, so there
    /// is nothing truthful to describe.
    GenericFunction {
        /// The function's exposed name.
        name: String,
    },
    /// A generic foreign interface or foreign function without the
    /// `generics` feature.
    GenericForeignInterface {
        /// The interface's exposed name.
        name: String,
    },
    /// A type has no Lua representation.
    Unrepresentable {
        /// Where the type appears.
        context: String,
        /// Why it cannot be represented.
        detail: String,
    },
}

impl std::fmt::Display for LuaDeclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTypeName { name } => write!(
                f,
                "two registered types expose the name `{name}`; declaration stubs need unique type names"
            ),
            Self::GenericFunction { name } => write!(
                f,
                "generic function `{name}` is not supported by this backend: \
                 Lua dispatches by name only and cannot distinguish instantiations"
            ),
            Self::GenericForeignInterface { name } => write!(
                f,
                "foreign interface `{name}` is generic; enable the `generics` feature to \
                 describe dynamic single-function dispatch"
            ),
            Self::Unrepresentable { context, detail } => {
                write!(
                    f,
                    "type in `{context}` cannot be represented in Lua: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for LuaDeclError {}

impl BindingGenerator for LuaDeclGenerator {
    type Error = LuaDeclError;

    fn language_name(&self) -> &'static str {
        "lua"
    }

    fn capabilities(&self) -> BackendCapabilities {
        LuaBinder::default_capabilities()
    }

    fn generate(&self, registry: &ValidatedRegistry<'_>) -> Result<GeneratedOutput, Self::Error> {
        let mut out = String::from("---@meta\n\n");
        check_unique_type_names(registry)?;

        // Type declarations, in registry order.
        for s in registry.structs() {
            emit_struct(&mut out, registry, s)?;
        }
        for e in registry.enums() {
            emit_enum(&mut out, e)?;
        }
        for a in registry.type_aliases() {
            emit_doc(&mut out, a.doc);
            let inner = render_type(registry, a.inner, &format!("type alias {}", a.name))?;
            let _ = writeln!(out, "---@alias {} {inner}\n", a.name);
        }

        // Foreign interfaces: callbacks-table shapes the script supplies.
        for fi in registry.foreign_interfaces() {
            emit_foreign(&mut out, registry, fi)?;
        }

        // Module tables, in registry order.
        for module in registry.modules() {
            emit_module(&mut out, registry, module, module.name)?;
        }

        Ok(GeneratedOutput {
            files: vec![GeneratedFile {
                path: self.path.clone(),
                content: out.into_bytes(),
                encoding: Some("utf-8".to_string()),
            }],
        })
    }
}

fn check_unique_type_names(registry: &ValidatedRegistry<'_>) -> Result<(), LuaDeclError> {
    let mut names: Vec<&str> = Vec::new();
    let all = registry
        .structs()
        .iter()
        .map(|s| s.name)
        .chain(registry.enums().iter().map(|e| e.name))
        .chain(registry.type_aliases().iter().map(|a| a.name))
        .chain(registry.foreign_interfaces().iter().map(|f| f.name));
    for name in all {
        if names.contains(&name) {
            return Err(LuaDeclError::DuplicateTypeName {
                name: name.to_string(),
            });
        }
        names.push(name);
    }
    Ok(())
}

fn emit_doc(out: &mut String, doc: Option<&str>) {
    if let Some(doc) = doc {
        for line in doc.lines() {
            let _ = writeln!(out, "---{line}");
        }
    }
}

fn emit_struct(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    s: &StructDescriptor<'_>,
) -> Result<(), LuaDeclError> {
    emit_doc(out, s.doc);
    let _ = writeln!(out, "---@class {}", s.name);
    for field in s.fields {
        let ty = render_type(registry, field.ty, &format!("{}.{}", s.name, field.name))?;
        let note = if field.readonly {
            " Readonly in the source API."
        } else {
            ""
        };
        let _ = writeln!(out, "---@field {} {ty}{note}", field.name);
    }
    for prop in s.properties {
        let ty = render_type(registry, prop.ty, &format!("{}.{}", s.name, prop.name))?;
        let note = if prop.readonly {
            " Readonly in the source API."
        } else {
            ""
        };
        let _ = writeln!(out, "---@field {} {ty}{note}", prop.name);
    }
    emit_operators(out, registry, s)?;
    let _ = writeln!(out, "local {} = {{}}\n", s.name);

    // Methods, colon syntax. `self`-consuming methods are exposed the same
    // way at runtime; static methods (no receiver) live on the type table
    // with the constructors.
    for m in s.methods {
        if m.receiver.is_some() {
            emit_function(
                out,
                registry,
                m,
                &format!("{}:", s.name),
                &format!("{}::{}", s.name, m.name),
            )?;
        }
    }
    Ok(())
}

fn emit_operators(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    s: &StructDescriptor<'_>,
) -> Result<(), LuaDeclError> {
    use haphe::TraitImpl;
    for ti in s.trait_impls {
        let (op, rhs, output) = match ti {
            TraitImpl::Add { rhs, output } => ("add", Some(*rhs), *output),
            TraitImpl::Sub { rhs, output } => ("sub", Some(*rhs), *output),
            TraitImpl::Mul { rhs, output } => ("mul", Some(*rhs), *output),
            TraitImpl::Div { rhs, output } => ("div", Some(*rhs), *output),
            TraitImpl::Rem { rhs, output } => ("mod", Some(*rhs), *output),
            TraitImpl::Neg { output } => ("unm", None, *output),
            // LuaLS supports `---@operator concat`; `ToString` types accept
            // string-like counterparts and produce a string.
            TraitImpl::ToString => {
                let _ = writeln!(out, "---@operator concat(string|number): string");
                continue;
            }
            _ => continue,
        };
        // Operator annotations don't resolve overloads; the runtime picks
        // exact rhs-type matches first, then declaration order.
        let context = format!("{} operator {op}", s.name);
        let output = render_type(registry, output, &context)?;
        match rhs {
            Some(rhs) => {
                let rhs = render_type(registry, rhs, &context)?;
                let _ = writeln!(out, "---@operator {op}({rhs}): {output}");
            }
            None => {
                let _ = writeln!(out, "---@operator {op}: {output}");
            }
        }
    }
    Ok(())
}

fn emit_enum(out: &mut String, e: &EnumDescriptor<'_>) -> Result<(), LuaDeclError> {
    emit_doc(out, e.doc);
    let unit_only = e
        .variants
        .iter()
        .all(|v| matches!(v.kind, VariantKind::Unit));
    if unit_only {
        let cases: Vec<String> = e
            .variants
            .iter()
            .map(|v| format!("\"{}\"", v.name))
            .collect();
        let _ = writeln!(out, "---@alias {} {}\n", e.name, cases.join("|"));
    } else {
        let _ = writeln!(
            out,
            "---Opaque enum value (payload variants cross as userdata)."
        );
        let _ = writeln!(out, "---@class {}\n", e.name);
    }
    Ok(())
}

fn emit_foreign(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    fi: &ForeignInterfaceDescriptor<'_>,
) -> Result<(), LuaDeclError> {
    if !cfg!(feature = "generics") && !fi.generic_params.is_empty() {
        return Err(LuaDeclError::GenericForeignInterface {
            name: fi.name.to_string(),
        });
    }
    emit_doc(out, fi.doc);
    let _ = writeln!(
        out,
        "---Callbacks table the script supplies to the host (foreign interface)."
    );
    let _ = writeln!(out, "---@class {}", fi.name);
    for f in fi.functions {
        if !cfg!(feature = "generics") && !f.generic_params.is_empty() {
            return Err(LuaDeclError::GenericForeignInterface {
                name: fi.name.to_string(),
            });
        }
        let context = format!("{}.{}", fi.name, f.name);
        let mut params = Vec::new();
        for p in f.params {
            let ty = render_type(registry, p.ty, &context)?;
            params.push(format!("{}: {ty}", p.name));
        }
        let ret = match f.return_type {
            TypeDescriptor::Unit => String::new(),
            ty => format!(": {}", render_type(registry, ty, &context)?),
        };
        let note = match f.error_kind {
            Some(kind) => format!(" May raise ({kind})."),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "---@field {} fun({}){ret}{note}",
            f.name,
            params.join(", ")
        );
    }
    out.push('\n');
    Ok(())
}

fn emit_module(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    module: &ModuleDescriptor<'_>,
    path: &str,
) -> Result<(), LuaDeclError> {
    emit_doc(out, module.doc);
    let _ = writeln!(out, "{path} = {{}}\n");

    for constant in module.constants {
        emit_constant(out, registry, constant, path)?;
    }

    // Type tables (constructors and static methods attach here).
    for type_id in module.type_ids {
        let Some(kind) = registry.get_type(type_id) else {
            continue;
        };
        match kind {
            haphe::TypeKind::Struct(s) => {
                let _ = writeln!(out, "---@type table");
                let _ = writeln!(out, "{path}.{} = {{}}\n", s.name);
                for ctor in s.constructors {
                    emit_function(
                        out,
                        registry,
                        ctor,
                        &format!("{path}.{}.", s.name),
                        &format!("{}::{}", s.name, ctor.name),
                    )?;
                }
                for m in s.methods {
                    if m.receiver.is_none() {
                        emit_function(
                            out,
                            registry,
                            m,
                            &format!("{path}.{}.", s.name),
                            &format!("{}::{}", s.name, m.name),
                        )?;
                    }
                }
            }
            haphe::TypeKind::Enum(e) => {
                let _ = writeln!(out, "---@type table");
                let _ = writeln!(out, "{path}.{} = {{}}\n", e.name);
            }
            haphe::TypeKind::TypeAlias(_) => {}
        }
    }

    for function in module.functions {
        if !function.generic_params.is_empty() {
            return Err(LuaDeclError::GenericFunction {
                name: function.name.to_string(),
            });
        }
        emit_function(
            out,
            registry,
            function,
            &format!("{path}."),
            &format!("{path}.{}", function.name),
        )?;
    }

    for submodule in module.submodules {
        emit_module(
            out,
            registry,
            submodule,
            &format!("{path}.{}", submodule.name),
        )?;
    }
    Ok(())
}

fn emit_constant(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    constant: &ConstantDescriptor<'_>,
    path: &str,
) -> Result<(), LuaDeclError> {
    emit_doc(out, constant.doc);
    let _ = writeln!(out, "---Constant value: {}", constant.value);
    let ty = render_type(
        registry,
        constant.ty,
        &format!("constant {}", constant.name),
    )?;
    let _ = writeln!(out, "---@type {ty}");
    let _ = writeln!(out, "{path}.{} = nil\n", constant.name);
    Ok(())
}

fn emit_function(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    prefix: &str,
    context: &str,
) -> Result<(), LuaDeclError> {
    emit_doc(out, f.doc);
    if let Some(kind) = f.error_kind {
        let _ = writeln!(out, "---Errors raise ({kind}).");
    }
    let mut names = Vec::new();
    for p in f.params {
        let ty = render_type(registry, p.ty, context)?;
        let _ = writeln!(out, "---@param {} {ty}", p.name);
        names.push(p.name);
    }
    match f.return_type {
        TypeDescriptor::Unit => {}
        ty => {
            let ret = render_type(registry, ty, context)?;
            let _ = writeln!(out, "---@return {ret}");
        }
    }
    let receiver_note = matches!(f.receiver, Some(Receiver::Owned));
    if receiver_note {
        let _ = writeln!(out, "---Consumes the receiver in the source API.");
    }
    let _ = writeln!(
        out,
        "function {prefix}{}({}) end\n",
        f.name,
        names.join(", ")
    );
    Ok(())
}

/// Renders a descriptor as a LuaLS type, resolving `Ref`/`Instance` through
/// the registry.
fn render_type(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
) -> Result<String, LuaDeclError> {
    render_impl(registry, ty, context)
}

fn render_impl(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
) -> Result<String, LuaDeclError> {
    let err = |detail: &str| LuaDeclError::Unrepresentable {
        context: context.to_string(),
        detail: detail.to_string(),
    };
    Ok(match ty {
        TypeDescriptor::Primitive(p) => match p {
            PrimitiveType::Bool => "boolean".into(),
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64 => "integer".into(),
            PrimitiveType::I128 | PrimitiveType::U128 => {
                return Err(err("128-bit integers exceed Lua's integer width"));
            }
            PrimitiveType::F32 | PrimitiveType::F64 => "number".into(),
            PrimitiveType::Char => "string".into(),
            _ => return Err(err("unsupported primitive")),
        },
        TypeDescriptor::String | TypeDescriptor::Bytes => "string".into(),
        TypeDescriptor::Unit => "nil".into(),
        TypeDescriptor::Option(inner) => format!("{}?", render_impl(registry, inner, context)?),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => {
            format!("{}[]", render_impl(registry, inner, context)?)
        }
        TypeDescriptor::Map(k, v) => format!(
            "table<{}, {}>",
            render_impl(registry, k, context)?,
            render_impl(registry, v, context)?
        ),
        TypeDescriptor::Tuple(elems) => {
            let parts: Vec<String> = elems
                .iter()
                .map(|e| render_impl(registry, e, context))
                .collect::<Result<_, _>>()?;
            format!("[{}]", parts.join(", "))
        }
        // Failures raise Lua errors; the value type is the ok side.
        TypeDescriptor::Result(ok, _) => render_impl(registry, ok, context)?,
        TypeDescriptor::Callback {
            params,
            return_type,
        } => {
            let parts: Vec<String> = params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    Ok(format!(
                        "a{}: {}",
                        i + 1,
                        render_impl(registry, p, context)?
                    ))
                })
                .collect::<Result<_, LuaDeclError>>()?;
            let ret = match return_type {
                TypeDescriptor::Unit => String::new(),
                ty => format!(": {}", render_impl(registry, ty, context)?),
            };
            format!("fun({}){ret}", parts.join(", "))
        }
        TypeDescriptor::Ref(id) | TypeDescriptor::Instance { id, .. } => {
            match registry.get_type(id) {
                Some(haphe::TypeKind::Struct(s)) => s.name.to_string(),
                Some(haphe::TypeKind::Enum(e)) => e.name.to_string(),
                Some(haphe::TypeKind::TypeAlias(a)) => a.name.to_string(),
                None => return Err(err("reference to an unregistered type")),
            }
        }
        // Erased generic parameter: the runtime accepts any instantiation.
        TypeDescriptor::GenericParam(_) => "any".into(),
        TypeDescriptor::Stream(_) | TypeDescriptor::Future(_) => "userdata".into(),
        _ => return Err(err("unsupported type")),
    })
}
