//! LuaLS/EmmyLua declaration stubs: a `---@meta` definition file for
//! [lua-language-server](https://luals.github.io/), generated from a haphe
//! registry so editors see the same surface the runtime binder exposes.
//!
//! Mapping decisions (mirroring runtime behavior, verbatim names):
//! - structs → `---@class` with `---@field`s (readonly noted in the field
//!   description), colon-syntax methods, constructors as dot functions on
//!   the module's type table, operator traits as `---@operator` lines;
//! - unit-only enums → `---@enum Name` on a table literal mirroring the
//!   runtime case table (string names, or numeric discriminants per repr)
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
    // Indexed access: LuaLS models `obj[key]` as a bracketed field. Emitted
    // once even when both Index and IndexMut are declared (LuaLS fields are
    // readable and writable).
    if let Some((index, output)) = s.trait_impls.iter().find_map(|ti| match ti {
        haphe::TraitImpl::Index { index, output }
        | haphe::TraitImpl::IndexMut { index, output } => Some((index, output)),
        _ => None,
    }) {
        let context = format!("{} index operator", s.name);
        let k = render_type(registry, index, &context)?;
        let v = render_type(registry, output, &context)?;
        let _ = writeln!(out, "---@field [{k}] {v}");
    }
    emit_operators(out, registry, s)?;
    let _ = writeln!(out, "local {} = {{}}\n", s.name);

    // Iterable types: the runtime registers `__pairs`/`__iter` where the
    // version supports it and a portable `iter()` method everywhere.
    for ti in s.trait_impls {
        let item = match ti {
            haphe::TraitImpl::IntoIterator { item } | haphe::TraitImpl::Iterator { item } => item,
            _ => continue,
        };
        let context = format!("{} iterator item", s.name);
        let stepper = match item {
            TypeDescriptor::Tuple(elems) if elems.len() == 2 => {
                let k = render_type(registry, &elems[0], &context)?;
                let v = render_type(registry, &elems[1], &context)?;
                format!("fun(): {k}, {v}")
            }
            other => format!(
                "fun(): integer, {}",
                render_type(registry, other, &context)?
            ),
        };
        let _ = writeln!(
            out,
            "---Iterable: `pairs(x)` on 5.2+, `x:iter()` everywhere."
        );
        let _ = writeln!(out, "---@return {stepper}");
        let _ = writeln!(out, "function {}:iter() end\n", s.name);
        break;
    }

    // Hash / Debug surface as portable methods (Lua has no hashing or
    // debug-formatting protocol).
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, haphe::TraitImpl::Hash))
    {
        let _ = writeln!(
            out,
            "---Stable hash of the value (Rust `Hash` digest as an integer)."
        );
        let _ = writeln!(out, "---@return integer");
        let _ = writeln!(out, "function {}:hash() end\n", s.name);
    }
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, haphe::TraitImpl::Debug))
    {
        let _ = writeln!(out, "---Rust `Debug` formatting of the value.");
        let _ = writeln!(out, "---@return string");
        let _ = writeln!(out, "function {}:debug() end\n", s.name);
    }

    // Methods, colon syntax. `self`-consuming methods are exposed the same
    // way at runtime; static methods (no receiver) live on the type table
    // with the constructors. Generic methods are dyn-only (one callable
    // scanning instantiations), stubbed like dyn free functions; static
    // generic methods have no runtime dispatch, so nothing truthful exists
    // to describe.
    for m in s.methods {
        if m.receiver.is_some() {
            let prefix = format!("{}:", s.name);
            let context = format!("{}::{}", s.name, m.name);
            if !m.generic_params.is_empty() {
                if m.dispatch == haphe::Dispatch::Dyn {
                    emit_dyn_signature(out, registry, m, m.instantiations, &prefix, &context)?;
                    continue;
                }
                return Err(LuaDeclError::GenericFunction { name: context });
            }
            emit_function(out, registry, m, &prefix, &context)?;
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
    // `%` dedupe: an explicit `Mod` wins over `Rem` (both annotate as
    // `mod`), mirroring the runtime precedence.
    let has_mod_decl = s
        .trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::Mod { .. }));
    for ti in s.trait_impls {
        let (op, rhs, output) = match ti {
            TraitImpl::Add { rhs, output } => ("add", Some(*rhs), *output),
            TraitImpl::Sub { rhs, output } => ("sub", Some(*rhs), *output),
            TraitImpl::Mul { rhs, output } => ("mul", Some(*rhs), *output),
            TraitImpl::Div { rhs, output } => ("div", Some(*rhs), *output),
            TraitImpl::Rem { rhs, output } if !has_mod_decl => ("mod", Some(*rhs), *output),
            TraitImpl::Rem { .. } => continue,
            TraitImpl::Mod { rhs, output } => ("mod", Some(*rhs), *output),
            TraitImpl::Pow { rhs, output } => ("pow", Some(*rhs), *output),
            TraitImpl::IDiv { rhs, output } => ("idiv", Some(*rhs), *output),
            // Bitwise annotations are emitted from the descriptor regardless
            // of the configured Lua version, like every other stub here: the
            // stub describes the type; a pre-5.3 runtime rejects the binding
            // itself at registration. LuaLS supports all six operator names.
            TraitImpl::BitAnd { rhs, output } => ("band", Some(*rhs), *output),
            TraitImpl::BitOr { rhs, output } => ("bor", Some(*rhs), *output),
            TraitImpl::BitXor { rhs, output } => ("bxor", Some(*rhs), *output),
            TraitImpl::Shl { rhs, output } => ("shl", Some(*rhs), *output),
            TraitImpl::Shr { rhs, output } => ("shr", Some(*rhs), *output),
            TraitImpl::Neg { output } => ("unm", None, *output),
            TraitImpl::Not { output } => ("bnot", None, *output),
            // Callable objects: LuaLS documents them as an `---@overload`
            // on the class. Sync and async spell identically — the Lua-side
            // calling convention is the same when mlua drives the future.
            TraitImpl::Call { args, output } | TraitImpl::AsyncCall { args, output } => {
                let context = format!("{} call operator", s.name);
                let mut params = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    let rendered = render_type(registry, arg, &context)?;
                    params.push(format!("a{}: {rendered}", i + 1));
                }
                let out_rendered = render_type(registry, output, &context)?;
                let _ = writeln!(
                    out,
                    "---@overload fun({}): {out_rendered}",
                    params.join(", ")
                );
                continue;
            }
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
        let rhs_rendered = match rhs {
            Some(rhs) => Some(render_type(registry, rhs, &context)?),
            None => None,
        };
        match rhs_rendered {
            Some(ref rhs) => {
                let _ = writeln!(out, "---@operator {op}({rhs}): {output}");
            }
            None => {
                let _ = writeln!(out, "---@operator {op}: {output}");
            }
        }
        // The runtime also answers `//` for integer-typed `Div` declarations
        // (the truncating fallback), so the stub advertises it — describing
        // what the type actually responds to, mirroring how iteration stubs
        // document runtime behavior rather than gating on versions.
        let rhs_is_int = matches!(
            rhs,
            Some(haphe::TypeDescriptor::Primitive(
                haphe::PrimitiveType::I8
                    | haphe::PrimitiveType::I16
                    | haphe::PrimitiveType::I32
                    | haphe::PrimitiveType::I64
                    | haphe::PrimitiveType::U8
                    | haphe::PrimitiveType::U16
                    | haphe::PrimitiveType::U32
                    | haphe::PrimitiveType::U64
            ))
        );
        if op == "div"
            && rhs_is_int
            && crate::declared_idiv_fallback(s.trait_impls)
            && let Some(ref r) = rhs_rendered
        {
            let _ = writeln!(out, "---@operator idiv({r}): {output}");
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
        // LuaLS `---@enum` attaches to a real table literal mirroring the
        // runtime case table: string case names for string-represented
        // enums, integer discriminants for numeric ones (exact Rust
        // `#[repr]` type; flags carry their real bit values).
        let _ = writeln!(out, "---@enum {}", e.name);
        let _ = writeln!(out, "local {} = {{", e.name);
        for v in e.variants {
            match v.discriminant {
                Some(value) => {
                    let _ = writeln!(out, "    {} = {},", v.name, value);
                }
                None => {
                    let _ = writeln!(out, "    {} = \"{}\",", v.name, v.name);
                }
            }
        }
        let _ = writeln!(out, "}}\n");
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
                // `traits(Default)` registers an implicit nullary
                // constructor named `default`.
                if s.trait_impls
                    .iter()
                    .any(|ti| matches!(ti, haphe::TraitImpl::Default))
                {
                    let _ = writeln!(out, "---Constructs the default value (Rust `Default`).");
                    let _ = writeln!(out, "---@return {}", s.name);
                    let _ = writeln!(out, "function {path}.{}.default() end\n", s.name);
                }
                for m in s.methods {
                    if m.receiver.is_none() {
                        let prefix = format!("{path}.{}.", s.name);
                        let context = format!("{}::{}", s.name, m.name);
                        if !m.generic_params.is_empty() {
                            if m.dispatch == haphe::Dispatch::Dyn {
                                emit_dyn_signature(
                                    out,
                                    registry,
                                    m,
                                    m.instantiations,
                                    &prefix,
                                    &context,
                                )?;
                                continue;
                            }
                            return Err(LuaDeclError::GenericFunction { name: context });
                        }
                        emit_function(out, registry, m, &prefix, &context)?;
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
            // Dyn-dispatched generics ARE callable by bare name: one stub
            // with the first instantiation's signature plus an `@overload`
            // per additional instantiation. STATIC generics stay rejected —
            // the Lua runtime cannot dispatch monomorphs by name, so stubs
            // describing them would advertise callables that trap
            // (registry-level instantiations don't lift that).
            if function.dispatch == haphe::Dispatch::Dyn {
                emit_dyn_function(
                    out,
                    registry,
                    module,
                    function,
                    &format!("{path}."),
                    &format!("{path}.{}", function.name),
                )?;
                continue;
            }
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

/// Emits a dyn generic function: the first instantiation's substituted
/// signature, with one `---@overload` line per additional instantiation.
/// Module functions union registry-level instantiations in; methods carry
/// theirs on the descriptor alone.
fn emit_dyn_function(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    module: &haphe::ModuleDescriptor<'_>,
    f: &FunctionDescriptor<'_>,
    prefix: &str,
    context: &str,
) -> Result<(), LuaDeclError> {
    let instantiations: Vec<&[TypeDescriptor<'_>]> = module.instantiations_of(f).collect();
    emit_dyn_signature(out, registry, f, &instantiations, prefix, context)
}

/// The dyn stub body shared by module functions and methods.
fn emit_dyn_signature(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    instantiations: &[&[TypeDescriptor<'_>]],
    prefix: &str,
    context: &str,
) -> Result<(), LuaDeclError> {
    let Some((first, rest)) = instantiations.split_first() else {
        // Unreachable through the macro (dyn requires instantiate), but
        // hand-written descriptors stay loud.
        return Err(LuaDeclError::GenericFunction {
            name: f.name.to_string(),
        });
    };
    emit_doc(out, f.doc);
    for args in rest {
        let subst = haphe::dispatch::GenericSubst {
            params: f.generic_params,
            args,
        };
        let mut parts = Vec::new();
        for p in f.params {
            let ty = render_subst(registry, p.ty, context, Some(&subst))?;
            parts.push(format!("{}: {ty}", p.name));
        }
        let ret = match f.return_type {
            TypeDescriptor::Unit => String::new(),
            ty => format!(": {}", render_subst(registry, ty, context, Some(&subst))?),
        };
        let _ = writeln!(out, "---@overload fun({}){ret}", parts.join(", "));
    }
    let subst = haphe::dispatch::GenericSubst {
        params: f.generic_params,
        args: first,
    };
    let mut names = Vec::new();
    for p in f.params {
        let ty = render_subst(registry, p.ty, context, Some(&subst))?;
        let _ = writeln!(out, "---@param {} {ty}", p.name);
        names.push(p.name);
    }
    match f.return_type {
        TypeDescriptor::Unit => {}
        ty => {
            let ret = render_subst(registry, ty, context, Some(&subst))?;
            let _ = writeln!(out, "---@return {ret}");
        }
    }
    let _ = writeln!(
        out,
        "function {prefix}{}({}) end
",
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
    render_subst(registry, ty, context, None)
}

/// Like [`render_type`], resolving `GenericParam` references through a dyn
/// candidate's concrete type arguments.
fn render_subst(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
    subst: Option<&haphe::dispatch::GenericSubst<'_>>,
) -> Result<String, LuaDeclError> {
    // Lua has no borrow semantics: a `Borrowed` value renders as its inner
    // type, the carried lifetime ignored.
    let ty = crate::peel_borrowed(ty);
    if let TypeDescriptor::GenericParam(name) = ty
        && let Some(sub) = subst
        && let Some(i) = sub.params.iter().position(|p| p.name == *name)
        && let Some(resolved) = sub.args.get(i)
    {
        return render_subst(registry, resolved, context, subst);
    }
    // Composite types containing GenericParam leaves resolve them the same
    // way; plain rendering keeps `any` for unresolved parameters.
    match ty {
        TypeDescriptor::Option(inner) => Ok(format!(
            "{}?",
            render_subst(registry, inner, context, subst)?
        )),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => Ok(format!(
            "{}[]",
            render_subst(registry, inner, context, subst)?
        )),
        TypeDescriptor::Map(k, v) => Ok(format!(
            "table<{}, {}>",
            render_subst(registry, k, context, subst)?,
            render_subst(registry, v, context, subst)?
        )),
        _ => render_impl(registry, ty, context),
    }
}

fn render_impl(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
) -> Result<String, LuaDeclError> {
    let ty = crate::peel_borrowed(ty);
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
