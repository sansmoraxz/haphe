//! Luau-native declaration stubs: a `.d.luau` definition file for
//! [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp), generated from a
//! haphe registry so editors see the same surface the runtime binder exposes.
//!
//! Mapping decisions (mirroring runtime behavior, verbatim names):
//! - structs → `declare class` with fields, `read` for readonly, methods and
//!   metamethods inside the class block;
//! - unit-only enums → `type Name = "A" | "B"` string-literal union (or
//!   `type Name = number` for numeric-repr enums);
//!   enums with payload cases → `declare class` (they cross as userdata);
//! - type aliases → `type Alias = underlying`;
//! - generic types → one erased class (as the runtime exposes), with generic
//!   parameters typed `any`;
//! - foreign interfaces → `type Name = { ... }` describing the callbacks
//!   table a script must supply;
//! - `Result<T, E>` returns render as `T` (failures raise Luau errors);
//! - modules → `declare mod: { ... }` table type literals;
//! - operators → metamethod declarations inside `declare class`.

use std::fmt::Write as _;

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, EnumDescriptor,
    ForeignInterfaceDescriptor, FunctionDescriptor, GeneratedFile, GeneratedOutput,
    ModuleDescriptor, PrimitiveType, StructDescriptor, TypeDescriptor, ValidatedRegistry,
    VariantKind,
};

use crate::LuaBinder;

/// Generates a Luau `.d.luau` declaration stub from a registry.
#[derive(Debug, Clone)]
pub struct LuauDeclGenerator {
    path: String,
}

impl LuauDeclGenerator {
    /// Creates a generator writing to `definitions.d.luau`.
    pub fn new() -> Self {
        Self {
            path: "definitions.d.luau".to_string(),
        }
    }

    /// Sets the generated file's path.
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

impl Default for LuauDeclGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors produced while generating Luau declaration stubs.
#[derive(Debug)]
pub enum LuauDeclError {
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
    /// A type has no Luau representation.
    Unrepresentable {
        /// Where the type appears.
        context: String,
        /// Why it cannot be represented.
        detail: String,
    },
}

impl std::fmt::Display for LuauDeclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTypeName { name } => write!(
                f,
                "two registered types expose the name `{name}`; declaration stubs need unique type names"
            ),
            Self::GenericFunction { name } => write!(
                f,
                "generic function `{name}` is not supported by this backend: \
                 Luau dispatches by name only and cannot distinguish instantiations"
            ),
            Self::GenericForeignInterface { name } => write!(
                f,
                "foreign interface `{name}` is generic; enable the `generics` feature to \
                 describe dynamic single-function dispatch"
            ),
            Self::Unrepresentable { context, detail } => {
                write!(
                    f,
                    "type in `{context}` cannot be represented in Luau: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for LuauDeclError {}

impl BindingGenerator for LuauDeclGenerator {
    type Error = LuauDeclError;

    fn language_name(&self) -> &'static str {
        "luau"
    }

    fn capabilities(&self) -> BackendCapabilities {
        LuaBinder::default_capabilities()
    }

    fn generate(&self, registry: &ValidatedRegistry<'_>) -> Result<GeneratedOutput, Self::Error> {
        let mut out = String::new();
        check_unique_type_names(registry)?;

        for s in registry.structs() {
            emit_struct(&mut out, registry, s)?;
        }
        for e in registry.enums() {
            emit_enum(&mut out, registry, e)?;
        }
        for a in registry.type_aliases() {
            emit_doc(&mut out, a.doc, "");
            let inner = render_type(registry, a.inner, &format!("type alias {}", a.name))?;
            let _ = writeln!(out, "type {} = {inner}\n", a.name);
        }

        for fi in registry.foreign_interfaces() {
            emit_foreign(&mut out, registry, fi)?;
        }

        for module in registry.modules() {
            emit_module(&mut out, registry, module, module.name, "")?;
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

fn check_unique_type_names(registry: &ValidatedRegistry<'_>) -> Result<(), LuauDeclError> {
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
            return Err(LuauDeclError::DuplicateTypeName {
                name: name.to_string(),
            });
        }
        names.push(name);
    }
    Ok(())
}

fn emit_doc(out: &mut String, doc: Option<&str>, indent: &str) {
    if let Some(doc) = doc {
        for line in doc.lines() {
            let _ = writeln!(out, "{indent}-- {line}");
        }
    }
}

fn emit_struct(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    s: &StructDescriptor<'_>,
) -> Result<(), LuauDeclError> {
    emit_doc(out, s.doc, "");
    let _ = writeln!(out, "declare class {}", s.name);
    let indent = "    ";

    for field in s.fields {
        let ty = render_type(registry, field.ty, &format!("{}.{}", s.name, field.name))?;
        if field.readonly {
            let _ = writeln!(out, "{indent}read {}: {ty}", field.name);
        } else {
            let _ = writeln!(out, "{indent}{}: {ty}", field.name);
        }
    }
    for prop in s.properties {
        let ty = render_type(registry, prop.ty, &format!("{}.{}", s.name, prop.name))?;
        if prop.readonly {
            let _ = writeln!(out, "{indent}read {}: {ty}", prop.name);
        } else {
            let _ = writeln!(out, "{indent}{}: {ty}", prop.name);
        }
    }

    emit_operators(out, registry, s)?;

    // Iteration: Luau uses `__iter` metamethod for `for ... in obj do`.
    for ti in s.trait_impls {
        let (haphe::TraitImpl::IntoIterator { item } | haphe::TraitImpl::Iterator { item }) = ti
        else {
            continue;
        };
        let context = format!("{} iterator item", s.name);
        match item {
            TypeDescriptor::Tuple(elems) if elems.len() == 2 => {
                let k = render_type(registry, &elems[0], &context)?;
                let v = render_type(registry, &elems[1], &context)?;
                let _ = writeln!(
                    out,
                    "{indent}function __iter(self): (({k}, {v}) -> ({k}, {v}), {{}})"
                );
            }
            other => {
                let v = render_type(registry, other, &context)?;
                let _ = writeln!(
                    out,
                    "{indent}function __iter(self): ((number, {v}) -> (number, {v}), {{}})"
                );
            }
        }
        let _ = writeln!(
            out,
            "{indent}function iter(self): ((number, any) -> (number, any), {{}})"
        );
        break;
    }

    // Hash / Debug as methods.
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, haphe::TraitImpl::Hash))
    {
        let _ = writeln!(out, "{indent}function hash(self): number");
    }
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, haphe::TraitImpl::Debug))
    {
        let _ = writeln!(out, "{indent}function debug(self): string");
    }

    // Methods inside declare class.
    for m in s.methods {
        if m.receiver.is_some() {
            let context = format!("{}::{}", s.name, m.name);
            if !m.generic_params.is_empty() {
                if m.dispatch == haphe::Dispatch::Dyn {
                    emit_dyn_class_method(out, registry, m, m.instantiations, &context)?;
                } else {
                    emit_static_class_methods(out, registry, m, m.instantiations, &context)?;
                }
                continue;
            }
            emit_class_method(out, registry, m, &context)?;
        }
    }

    let _ = writeln!(out, "end\n");
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    clippy::match_same_arms,
    reason = "the arm table is a POLICY TABLE — one documented rule per operator, including deliberately suppressed ones; length tracks the operator surface"
)]
fn emit_operators(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    s: &StructDescriptor<'_>,
) -> Result<(), LuauDeclError> {
    use haphe::TraitImpl;
    let indent = "    ";
    let has_mod_decl = s
        .trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::Mod { .. }));

    // Display → __tostring
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::Display))
    {
        let _ = writeln!(out, "{indent}function __tostring(self): string");
    }

    // Comparison operators from PartialEq/Eq.
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::PartialEq | TraitImpl::Eq))
    {
        let _ = writeln!(
            out,
            "{indent}function __eq(self, other: {}): boolean",
            s.name
        );
    }

    // Comparison operators from PartialOrd/Ord.
    if s.trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::PartialOrd | TraitImpl::Ord))
    {
        let _ = writeln!(
            out,
            "{indent}function __lt(self, other: {}): boolean",
            s.name
        );
        let _ = writeln!(
            out,
            "{indent}function __le(self, other: {}): boolean",
            s.name
        );
    }

    for ti in s.trait_impls {
        let (meta, rhs, output) = match ti {
            TraitImpl::Add { rhs, output } => ("__add", Some(*rhs), *output),
            TraitImpl::Sub { rhs, output } => ("__sub", Some(*rhs), *output),
            TraitImpl::Mul { rhs, output } => ("__mul", Some(*rhs), *output),
            TraitImpl::Div { rhs, output } => ("__div", Some(*rhs), *output),
            TraitImpl::Rem { rhs, output } if !has_mod_decl => ("__mod", Some(*rhs), *output),
            TraitImpl::Rem { .. } => continue,
            TraitImpl::Mod { rhs, output } => ("__mod", Some(*rhs), *output),
            TraitImpl::Pow { rhs, output } => ("__pow", Some(*rhs), *output),
            TraitImpl::IDiv { rhs, output } => ("__idiv", Some(*rhs), *output),
            // Luau has NO bitwise metamethods — skip them.
            TraitImpl::BitAnd { .. }
            | TraitImpl::BitOr { .. }
            | TraitImpl::BitXor { .. }
            | TraitImpl::Shl { .. }
            | TraitImpl::Shr { .. }
            | TraitImpl::Not { .. } => continue,
            TraitImpl::Neg { output } => ("__unm", None, *output),
            TraitImpl::Call { args, output } | TraitImpl::AsyncCall { args, output } => {
                let context = format!("{} call operator", s.name);
                let mut params = Vec::new();
                params.push("self".to_string());
                for (i, arg) in args.iter().enumerate() {
                    let rendered = render_type(registry, arg, &context)?;
                    params.push(format!("a{}: {rendered}", i + 1));
                }
                let out_rendered = render_type(registry, output, &context)?;
                let _ = writeln!(
                    out,
                    "{indent}function __call({}): {out_rendered}",
                    params.join(", ")
                );
                continue;
            }
            TraitImpl::ToString => {
                let _ = writeln!(
                    out,
                    "{indent}function __concat(self, rhs: string | number): string"
                );
                continue;
            }
            TraitImpl::Index { index, output } => {
                let context = format!("{} index operator", s.name);
                let k = render_type(registry, index, &context)?;
                let v = render_type(registry, output, &context)?;
                let _ = writeln!(out, "{indent}function __index(self, key: {k}): {v}");
                continue;
            }
            TraitImpl::IndexMut { index, output } => {
                let context = format!("{} index operator", s.name);
                let k = render_type(registry, index, &context)?;
                let v = render_type(registry, output, &context)?;
                let _ = writeln!(
                    out,
                    "{indent}function __newindex(self, key: {k}, value: {v})"
                );
                continue;
            }
            _ => continue,
        };
        let context = format!("{} operator {meta}", s.name);
        let output = render_type(registry, output, &context)?;
        match rhs {
            Some(rhs) => {
                let rhs_rendered = render_type(registry, rhs, &context)?;
                let _ = writeln!(
                    out,
                    "{indent}function {meta}(self, rhs: {rhs_rendered}): {output}"
                );
            }
            None => {
                let _ = writeln!(out, "{indent}function {meta}(self): {output}");
            }
        }
        // idiv fallback: mirrors decl.rs logic.
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
        if meta == "__div"
            && rhs_is_int
            && crate::declared_idiv_fallback(s.trait_impls)
            && let Some(rhs_td) = rhs
        {
            let rhs_rendered = render_type(registry, rhs_td, &context)?;
            let _ = writeln!(
                out,
                "{indent}function __idiv(self, rhs: {rhs_rendered}): {output}"
            );
        }
    }
    Ok(())
}

/// Emits a single method inside a `declare class` block.
fn emit_class_method(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    context: &str,
) -> Result<(), LuauDeclError> {
    let indent = "    ";
    emit_doc(out, f.doc, indent);
    let mut params = vec!["self".to_string()];
    for p in f.params {
        let ty = render_type(registry, p.ty, context)?;
        params.push(format!("{}: {ty}", p.name));
    }
    let ret = render_return(registry, f.return_type, context)?;
    let _ = writeln!(
        out,
        "{indent}function {}({}){ret}",
        f.name,
        params.join(", ")
    );
    Ok(())
}

/// Emits dyn-dispatched generic methods as overloads inside `declare class`.
fn emit_dyn_class_method(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    instantiations: &[&[TypeDescriptor<'_>]],
    context: &str,
) -> Result<(), LuauDeclError> {
    let indent = "    ";
    if instantiations.is_empty() {
        return Err(LuauDeclError::GenericFunction {
            name: f.name.to_string(),
        });
    }
    emit_doc(out, f.doc, indent);
    for args in instantiations {
        let subst = haphe::dispatch::GenericSubst {
            params: f.generic_params,
            args,
        };
        let mut params = vec!["self".to_string()];
        for p in f.params {
            let ty = render_subst(registry, p.ty, context, Some(&subst))?;
            params.push(format!("{}: {ty}", p.name));
        }
        let ret = render_return_subst(registry, f.return_type, context, Some(&subst))?;
        let _ = writeln!(
            out,
            "{indent}function {}({}){ret}",
            f.name,
            params.join(", ")
        );
    }
    Ok(())
}

/// Emits static generic methods as per-monomorph entries inside `declare class`.
fn emit_static_class_methods(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    instantiations: &[&[TypeDescriptor<'_>]],
    context: &str,
) -> Result<(), LuauDeclError> {
    let indent = "    ";
    if instantiations.is_empty() {
        return Err(LuauDeclError::GenericFunction {
            name: context.to_string(),
        });
    }
    for args in instantiations {
        let mangled = crate::binder::mangle_generic_name(f.name, args);
        let subst = haphe::dispatch::GenericSubst {
            params: f.generic_params,
            args,
        };
        emit_doc(out, f.doc, indent);
        let mut params = vec!["self".to_string()];
        for p in f.params {
            let ty = render_subst(registry, p.ty, context, Some(&subst))?;
            params.push(format!("{}: {ty}", p.name));
        }
        let ret = render_return_subst(registry, f.return_type, context, Some(&subst))?;
        let _ = writeln!(
            out,
            "{indent}function {mangled}({}){ret}",
            params.join(", ")
        );
    }
    Ok(())
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "uniform error-returning signature across all emit_* helpers; callers expect Result"
)]
fn emit_enum(
    out: &mut String,
    _registry: &ValidatedRegistry<'_>,
    e: &EnumDescriptor<'_>,
) -> Result<(), LuauDeclError> {
    emit_doc(out, e.doc, "");
    let unit_only = e
        .variants
        .iter()
        .all(|v| matches!(v.kind, VariantKind::Unit));
    if unit_only {
        let has_discriminants = e.variants.iter().any(|v| v.discriminant.is_some());
        if has_discriminants {
            // Luau cannot express specific integer values in a type; use
            // `number` as the type.
            let _ = writeln!(out, "type {} = number\n", e.name);
        } else {
            // String literal union.
            let cases: Vec<String> = e.variants.iter().map(|v| format!("\"{}\"", v.name)).collect();
            let _ = writeln!(out, "type {} = {}\n", e.name, cases.join(" | "));
        }
    } else {
        // Payload enum → declare class (crosses as userdata).
        let _ = writeln!(out, "declare class {}", e.name);
        let _ = writeln!(out, "end\n");

        // Constructor functions are emitted on the module table, not here.
        // Unit-case constants also live on the module table.
    }
    Ok(())
}

fn emit_foreign(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    fi: &ForeignInterfaceDescriptor<'_>,
) -> Result<(), LuauDeclError> {
    if !cfg!(feature = "generics") && !fi.generic_params.is_empty() {
        return Err(LuauDeclError::GenericForeignInterface {
            name: fi.name.to_string(),
        });
    }
    emit_doc(out, fi.doc, "");
    let _ = writeln!(out, "type {} = {{", fi.name);
    let indent = "    ";
    for f in fi.functions {
        if !cfg!(feature = "generics") && !f.generic_params.is_empty() {
            return Err(LuauDeclError::GenericForeignInterface {
                name: fi.name.to_string(),
            });
        }
        let context = format!("{}.{}", fi.name, f.name);
        let note = match f.error_kind {
            Some(kind) => format!(" -- May raise ({kind})."),
            None => String::new(),
        };
        if !f.generic_params.is_empty() {
            if f.instantiations.is_empty() {
                return Err(LuauDeclError::GenericFunction { name: context });
            }
            if f.dispatch == haphe::Dispatch::Dyn {
                let mut sigs = Vec::new();
                for args in f.instantiations {
                    let subst = haphe::dispatch::GenericSubst {
                        params: f.generic_params,
                        args,
                    };
                    sigs.push(render_fun_sig(registry, f, &context, Some(&subst))?);
                }
                let _ = writeln!(
                    out,
                    "{indent}{}: {},{}",
                    f.name,
                    sigs.join(" & "),
                    note
                );
            } else {
                for args in f.instantiations {
                    let mangled = crate::binder::mangle_generic_name(f.name, args);
                    let subst = haphe::dispatch::GenericSubst {
                        params: f.generic_params,
                        args,
                    };
                    let sig = render_fun_sig(registry, f, &context, Some(&subst))?;
                    let _ = writeln!(out, "{indent}{mangled}: {sig},{note}");
                }
            }
            continue;
        }
        let sig = render_fun_sig(registry, f, &context, None)?;
        let _ = writeln!(out, "{indent}{}: {sig},{note}", f.name);
    }
    let _ = writeln!(out, "}}\n");
    Ok(())
}

/// Renders `(p1: T1, p2: T2) -> R` for a foreign callback field.
fn render_fun_sig(
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    context: &str,
    subst: Option<&haphe::dispatch::GenericSubst<'_>>,
) -> Result<String, LuauDeclError> {
    let mut params = Vec::new();
    for p in f.params {
        let ty = render_subst(registry, p.ty, context, subst)?;
        params.push(format!("{}: {ty}", p.name));
    }
    let ret = match f.return_type {
        TypeDescriptor::Unit => " -> ()".to_string(),
        ty => format!(" -> {}", render_subst(registry, ty, context, subst)?),
    };
    Ok(format!("({}){ret}", params.join(", ")))
}

#[allow(
    clippy::too_many_lines,
    reason = "one emission arm per module surface; splitting the pass would scatter the per-surface rules"
)]
fn emit_module(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    module: &ModuleDescriptor<'_>,
    path: &str,
    indent: &str,
) -> Result<(), LuauDeclError> {
    let inner = format!("{indent}    ");
    emit_doc(out, module.doc, indent);
    let _ = writeln!(out, "{indent}declare {path}: {{");

    for constant in module.constants {
        emit_module_constant(out, registry, constant, &inner)?;
    }

    for type_id in module.type_ids {
        let Some(kind) = registry.get_type(type_id) else {
            continue;
        };
        match kind {
            haphe::TypeKind::Struct(s) => {
                let _ = writeln!(out, "{inner}{}: {{", s.name);
                let deep = format!("{inner}    ");
                for ctor in s.constructors {
                    emit_module_fn_entry(
                        out,
                        registry,
                        ctor,
                        &deep,
                        &format!("{}::{}", s.name, ctor.name),
                    )?;
                }
                if s.trait_impls
                    .iter()
                    .any(|ti| matches!(ti, haphe::TraitImpl::Default))
                {
                    let _ = writeln!(out, "{deep}default: () -> {},", s.name);
                }
                for m in s.methods {
                    if m.receiver.is_none() {
                        let context = format!("{}::{}", s.name, m.name);
                        if !m.generic_params.is_empty() {
                            if m.dispatch == haphe::Dispatch::Dyn {
                                emit_dyn_table_entry(
                                    out,
                                    registry,
                                    m,
                                    m.instantiations,
                                    &deep,
                                    &context,
                                )?;
                            } else {
                                emit_static_table_entries(
                                    out,
                                    registry,
                                    m,
                                    m.instantiations,
                                    &deep,
                                    &context,
                                )?;
                            }
                            continue;
                        }
                        emit_module_fn_entry(out, registry, m, &deep, &context)?;
                    }
                }
                let _ = writeln!(out, "{inner}}},");
            }
            haphe::TypeKind::Enum(e) => {
                let _ = writeln!(out, "{inner}{}: {{", e.name);
                let deep = format!("{inner}    ");
                let unit_only = e
                    .variants
                    .iter()
                    .all(|v| matches!(v.kind, VariantKind::Unit));
                for v in e.variants {
                    if matches!(v.kind, VariantKind::Unit) {
                        let _ = writeln!(out, "{deep}{}: {},", v.name, e.name);
                    }
                }
                if !unit_only {
                    for v in e.variants {
                        let context = format!("{}.{}", e.name, v.name);
                        let params: Vec<(String, String)> = match &v.kind {
                            VariantKind::Unit => continue,
                            VariantKind::Tuple(tys) => tys
                                .iter()
                                .enumerate()
                                .map(|(i, ty)| {
                                    Ok((
                                        format!("arg{}", i + 1),
                                        render_type(registry, ty, &context)?,
                                    ))
                                })
                                .collect::<Result<_, LuauDeclError>>()?,
                            VariantKind::Struct(fields) => fields
                                .iter()
                                .map(|f| {
                                    Ok((
                                        f.name.to_string(),
                                        render_type(registry, f.ty, &context)?,
                                    ))
                                })
                                .collect::<Result<_, LuauDeclError>>()?,
                        };
                        let param_strs: Vec<String> =
                            params.iter().map(|(n, t)| format!("{n}: {t}")).collect();
                        let _ = writeln!(
                            out,
                            "{deep}{}: ({}) -> {},",
                            v.name,
                            param_strs.join(", "),
                            e.name
                        );
                    }
                }
                let _ = writeln!(out, "{inner}}},");
            }
            haphe::TypeKind::TypeAlias(_) => {}
        }
    }

    for function in module.functions {
        let context = format!("{path}.{}", function.name);
        if !function.generic_params.is_empty() {
            if function.dispatch == haphe::Dispatch::Dyn {
                let instantiations: Vec<&[TypeDescriptor<'_>]> =
                    module.instantiations_of(function).collect();
                emit_dyn_table_entry(
                    out,
                    registry,
                    function,
                    &instantiations,
                    &inner,
                    &context,
                )?;
            } else {
                let instantiations: Vec<&[TypeDescriptor<'_>]> =
                    module.instantiations_of(function).collect();
                emit_static_table_entries(
                    out,
                    registry,
                    function,
                    &instantiations,
                    &inner,
                    &context,
                )?;
            }
            continue;
        }
        emit_module_fn_entry(out, registry, function, &inner, &context)?;
    }

    for submodule in module.submodules {
        emit_submodule(out, registry, submodule, &inner)?;
    }

    let _ = writeln!(out, "{indent}}}\n");
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "one emission arm per submodule surface; splitting the pass would scatter the per-surface rules"
)]
fn emit_submodule(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    module: &ModuleDescriptor<'_>,
    indent: &str,
) -> Result<(), LuauDeclError> {
    let inner = format!("{indent}    ");
    emit_doc(out, module.doc, indent);
    let _ = writeln!(out, "{indent}{}: {{", module.name);

    for constant in module.constants {
        emit_module_constant(out, registry, constant, &inner)?;
    }

    for type_id in module.type_ids {
        let Some(kind) = registry.get_type(type_id) else {
            continue;
        };
        match kind {
            haphe::TypeKind::Struct(s) => {
                let _ = writeln!(out, "{inner}{}: {{", s.name);
                let deep = format!("{inner}    ");
                for ctor in s.constructors {
                    emit_module_fn_entry(
                        out,
                        registry,
                        ctor,
                        &deep,
                        &format!("{}::{}", s.name, ctor.name),
                    )?;
                }
                if s.trait_impls
                    .iter()
                    .any(|ti| matches!(ti, haphe::TraitImpl::Default))
                {
                    let _ = writeln!(out, "{deep}default: () -> {},", s.name);
                }
                for m in s.methods {
                    if m.receiver.is_none() {
                        let context = format!("{}::{}", s.name, m.name);
                        if !m.generic_params.is_empty() {
                            if m.dispatch == haphe::Dispatch::Dyn {
                                emit_dyn_table_entry(
                                    out,
                                    registry,
                                    m,
                                    m.instantiations,
                                    &deep,
                                    &context,
                                )?;
                            } else {
                                emit_static_table_entries(
                                    out,
                                    registry,
                                    m,
                                    m.instantiations,
                                    &deep,
                                    &context,
                                )?;
                            }
                            continue;
                        }
                        emit_module_fn_entry(out, registry, m, &deep, &context)?;
                    }
                }
                let _ = writeln!(out, "{inner}}},");
            }
            haphe::TypeKind::Enum(e) => {
                let _ = writeln!(out, "{inner}{}: {{", e.name);
                let deep = format!("{inner}    ");
                for v in e.variants {
                    if matches!(v.kind, VariantKind::Unit) {
                        let _ = writeln!(out, "{deep}{}: {},", v.name, e.name);
                    }
                }
                let _ = writeln!(out, "{inner}}},");
            }
            haphe::TypeKind::TypeAlias(_) => {}
        }
    }

    for function in module.functions {
        let context = format!("{}.{}", module.name, function.name);
        if !function.generic_params.is_empty() {
            if function.dispatch == haphe::Dispatch::Dyn {
                let instantiations: Vec<&[TypeDescriptor<'_>]> =
                    module.instantiations_of(function).collect();
                emit_dyn_table_entry(
                    out,
                    registry,
                    function,
                    &instantiations,
                    &inner,
                    &context,
                )?;
            } else {
                let instantiations: Vec<&[TypeDescriptor<'_>]> =
                    module.instantiations_of(function).collect();
                emit_static_table_entries(
                    out,
                    registry,
                    function,
                    &instantiations,
                    &inner,
                    &context,
                )?;
            }
            continue;
        }
        emit_module_fn_entry(out, registry, function, &inner, &context)?;
    }

    for submodule in module.submodules {
        emit_submodule(out, registry, submodule, &inner)?;
    }

    let _ = writeln!(out, "{indent}}},");
    Ok(())
}

fn emit_module_constant(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    constant: &ConstantDescriptor<'_>,
    indent: &str,
) -> Result<(), LuauDeclError> {
    emit_doc(out, constant.doc, indent);
    let ty = render_type(
        registry,
        constant.ty,
        &format!("constant {}", constant.name),
    )?;
    let _ = writeln!(out, "{indent}{}: {ty},", constant.name);
    Ok(())
}

/// Emits a function entry inside a module table type: `name: (params) -> ret,`
fn emit_module_fn_entry(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    indent: &str,
    context: &str,
) -> Result<(), LuauDeclError> {
    emit_doc(out, f.doc, indent);
    if let Some(kind) = f.error_kind {
        let _ = writeln!(out, "{indent}-- Errors raise ({kind}).");
    }
    let mut params = Vec::new();
    for p in f.params {
        let ty = render_type(registry, p.ty, context)?;
        params.push(format!("{}: {ty}", p.name));
    }
    let ret = match f.return_type {
        TypeDescriptor::Unit => " -> ()".to_string(),
        ty => format!(" -> {}", render_type(registry, ty, context)?),
    };
    let _ = writeln!(
        out,
        "{indent}{}: ({}){ret},",
        f.name,
        params.join(", ")
    );
    Ok(())
}

/// Emits a dyn-dispatched generic function as an intersection type in a table.
fn emit_dyn_table_entry(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    instantiations: &[&[TypeDescriptor<'_>]],
    indent: &str,
    context: &str,
) -> Result<(), LuauDeclError> {
    if instantiations.is_empty() {
        return Err(LuauDeclError::GenericFunction {
            name: f.name.to_string(),
        });
    }
    emit_doc(out, f.doc, indent);
    let mut sigs = Vec::new();
    for args in instantiations {
        let subst = haphe::dispatch::GenericSubst {
            params: f.generic_params,
            args,
        };
        sigs.push(render_fun_sig(registry, f, context, Some(&subst))?);
    }
    let _ = writeln!(out, "{indent}{}: {},", f.name, sigs.join(" & "));
    Ok(())
}

/// Emits static generic function monomorphs as separate table entries.
fn emit_static_table_entries(
    out: &mut String,
    registry: &ValidatedRegistry<'_>,
    f: &FunctionDescriptor<'_>,
    instantiations: &[&[TypeDescriptor<'_>]],
    indent: &str,
    context: &str,
) -> Result<(), LuauDeclError> {
    if instantiations.is_empty() {
        return Err(LuauDeclError::GenericFunction {
            name: context.to_string(),
        });
    }
    for args in instantiations {
        let mangled = crate::binder::mangle_generic_name(f.name, args);
        let subst = haphe::dispatch::GenericSubst {
            params: f.generic_params,
            args,
        };
        emit_doc(out, f.doc, indent);
        let mut params = Vec::new();
        for p in f.params {
            let ty = render_subst(registry, p.ty, context, Some(&subst))?;
            params.push(format!("{}: {ty}", p.name));
        }
        let ret = match f.return_type {
            TypeDescriptor::Unit => " -> ()".to_string(),
            ty => format!(" -> {}", render_subst(registry, ty, context, Some(&subst))?),
        };
        let _ = writeln!(
            out,
            "{indent}{mangled}: ({}){ret},",
            params.join(", ")
        );
    }
    Ok(())
}

/// Renders a return type annotation: `: T` or empty for void.
fn render_return(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
) -> Result<String, LuauDeclError> {
    render_return_subst(registry, ty, context, None)
}

fn render_return_subst(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
    subst: Option<&haphe::dispatch::GenericSubst<'_>>,
) -> Result<String, LuauDeclError> {
    match ty {
        TypeDescriptor::Unit => Ok(String::new()),
        _ => Ok(format!(
            ": {}",
            render_subst(registry, ty, context, subst)?
        )),
    }
}

fn render_type(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
) -> Result<String, LuauDeclError> {
    render_subst(registry, ty, context, None)
}

fn render_subst(
    registry: &ValidatedRegistry<'_>,
    ty: &TypeDescriptor<'_>,
    context: &str,
    subst: Option<&haphe::dispatch::GenericSubst<'_>>,
) -> Result<String, LuauDeclError> {
    let ty = crate::peel_borrowed(ty);
    if let TypeDescriptor::GenericParam(name) = ty
        && let Some(sub) = subst
        && let Some(i) = sub.params.iter().position(|p| p.name == *name)
        && let Some(resolved) = sub.args.get(i)
    {
        return render_subst(registry, resolved, context, subst);
    }
    match ty {
        TypeDescriptor::Option(inner) => Ok(format!(
            "{}?",
            render_subst(registry, inner, context, subst)?
        )),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => Ok(format!(
            "{{{}}}",
            render_subst(registry, inner, context, subst)?
        )),
        TypeDescriptor::Map(k, v) => Ok(format!(
            "{{[{}]: {}}}",
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
) -> Result<String, LuauDeclError> {
    let ty = crate::peel_borrowed(ty);
    let err = |detail: &str| LuauDeclError::Unrepresentable {
        context: context.to_string(),
        detail: detail.to_string(),
    };
    Ok(match ty {
        TypeDescriptor::Primitive(p) => match p {
            PrimitiveType::Bool => "boolean".into(),
            // Luau has no integer subtype; all numerics are `number`.
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::F32
            | PrimitiveType::F64 => "number".into(),
            PrimitiveType::I128 | PrimitiveType::U128 => {
                return Err(err("128-bit integers exceed Luau's number width"));
            }
            PrimitiveType::Char => "string".into(),
            _ => return Err(err("unsupported primitive")),
        },
        TypeDescriptor::String | TypeDescriptor::Bytes => "string".into(),
        TypeDescriptor::Unit => "nil".into(),
        TypeDescriptor::Option(inner) => format!("{}?", render_impl(registry, inner, context)?),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => {
            format!("{{{}}}", render_impl(registry, inner, context)?)
        }
        TypeDescriptor::Map(k, v) => format!(
            "{{[{}]: {}}}",
            render_impl(registry, k, context)?,
            render_impl(registry, v, context)?
        ),
        TypeDescriptor::Tuple(elems) => {
            if elems.is_empty() {
                "nil".into()
            } else {
                "{any}".into()
            }
        }
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
                .collect::<Result<_, LuauDeclError>>()?;
            let ret = match return_type {
                TypeDescriptor::Unit => " -> ()".into(),
                ty => format!(" -> {}", render_impl(registry, ty, context)?),
            };
            format!("({}){ret}", parts.join(", "))
        }
        TypeDescriptor::Ref(id) | TypeDescriptor::Instance { id, .. } => {
            match registry.get_type(id) {
                Some(haphe::TypeKind::Struct(s)) => s.name.to_string(),
                Some(haphe::TypeKind::Enum(e)) => e.name.to_string(),
                Some(haphe::TypeKind::TypeAlias(a)) => a.name.to_string(),
                None => return Err(err("reference to an unregistered type")),
            }
        }
        TypeDescriptor::GenericParam(_)
        | TypeDescriptor::Stream(_)
        | TypeDescriptor::Future(_) => "any".into(),
        _ => return Err(err("unsupported type")),
    })
}
