//! WIT (WebAssembly Component Model Interface Types) binding generator for
//! [haphe](https://crates.io/crates/haphe).
//!
//! [`WitGenerator`] implements [`BindingGenerator`], turning a haphe type
//! registry into a `.wit` document: modules become interfaces, structs become
//! records or resources, enums become `enum`s or `variant`s, and a world
//! imports every interface (the host provides the API, guest components
//! consume it).
//!
//! Identifiers are implicitly converted to kebab-case because the WIT grammar
//! admits no other casing; see the crate README for details.

mod emit;
mod model;
pub mod names;
#[cfg(feature = "runtime")]
mod runtime;
mod types;

#[cfg(feature = "runtime")]
pub use runtime::{WasmBindError, WasmBinder};

use std::fmt;

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, EnumDescriptor, FunctionDescriptor,
    GeneratedFile, GeneratedOutput, Ownership, Receiver, StructDescriptor, TypeAliasDescriptor,
    TypeDescriptor, TypeKind, ValidatedRegistry, VariantKind,
};

use emit::Printer;
use model::Plan;
use names::{NameMap, to_kebab};
use types::{Pos, render_return, render_type};

/// How module constants are represented (WIT has no constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConstantMode {
    /// Emit each constant as a nullary getter function, with the value in a
    /// doc comment.
    #[default]
    Getter,
    /// Omit constants from the output.
    Skip,
}

/// Generates a WIT document with a world that imports every interface.
#[derive(Debug, Clone)]
pub struct WitGenerator {
    package: String,
    version: Option<String>,
    world: String,
    default_interface: String,
    constants: ConstantMode,
}

impl WitGenerator {
    /// Creates a generator for the given WIT package name (`namespace:name`).
    pub fn new(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            version: None,
            world: "host".to_string(),
            default_interface: "types".to_string(),
            constants: ConstantMode::default(),
        }
    }

    /// Sets the package version (`package ns:name@version;`).
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Sets the world name (default `host`).
    pub fn with_world(mut self, name: impl Into<String>) -> Self {
        self.world = name.into();
        self
    }

    /// Sets the interface that holds types not claimed by any module
    /// (default `types`).
    pub fn with_default_interface(mut self, name: impl Into<String>) -> Self {
        self.default_interface = name.into();
        self
    }

    /// Sets how module constants are represented.
    pub fn with_constant_mode(mut self, mode: ConstantMode) -> Self {
        self.constants = mode;
        self
    }
}

/// Errors produced while generating WIT.
#[derive(Debug)]
pub enum WitGenError {
    /// The package name is not `namespace:name` (kebab-case segments).
    InvalidPackageName(String),
    /// A registered type has no WIT equivalent (128-bit ints, empty tuples,
    /// nested unit).
    UnrepresentableType {
        /// The function, field, or constant where the type appears.
        context: String,
        /// Why the type cannot be represented.
        detail: String,
    },
    /// A function returns a borrowed resource handle, which WIT forbids.
    BorrowedResourceReturn {
        /// The resource's type id.
        type_id: String,
        /// The offending function.
        function: String,
    },
    /// Two distinct source identifiers map to the same kebab-case name.
    NameCollision {
        /// The shared kebab-case name.
        kebab: String,
        /// The first source identifier.
        first: String,
        /// The second source identifier.
        second: String,
    },
    /// The generated document failed WIT resolution (e.g. recursive value
    /// types, cyclic interface `use`s, empty enums or records). Caught at
    /// generation time by validating the output through `wit-parser`.
    InvalidWit {
        /// The wit-parser diagnostic.
        message: String,
    },
}

impl fmt::Display for WitGenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPackageName(name) => write!(
                f,
                "invalid WIT package name `{name}`: expected `namespace:name` in kebab-case"
            ),
            Self::UnrepresentableType { context, detail } => {
                write!(
                    f,
                    "type in `{context}` cannot be represented in WIT: {detail}"
                )
            }
            Self::BorrowedResourceReturn { type_id, function } => write!(
                f,
                "function `{function}` returns a borrowed `{type_id}` handle; WIT only allows owned handles in return position"
            ),
            Self::NameCollision {
                kebab,
                first,
                second,
            } => write!(
                f,
                "identifiers `{first}` and `{second}` both map to WIT name `{kebab}`"
            ),
            Self::InvalidWit { message } => {
                write!(f, "generated document is not valid WIT: {message}")
            }
        }
    }
}

impl std::error::Error for WitGenError {}

impl BindingGenerator for WitGenerator {
    type Error = WitGenError;

    fn language_name(&self) -> &'static str {
        "wit"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::ALL
            .with_callbacks(false)
            .with_generics(false)
            .with_properties(true)
            .with_type_aliases(true)
            .with_required_thread_safety(None)
    }

    fn generate(&self, registry: &ValidatedRegistry<'_>) -> Result<GeneratedOutput, Self::Error> {
        validate_package_name(&self.package)?;
        let plan = Plan::build(registry, &self.default_interface)?;

        let mut p = Printer::new();
        match &self.version {
            Some(v) => p.line(&format!("package {}@{v};", self.package)),
            None => p.line(&format!("package {};", self.package)),
        }

        let mut emitted_ifaces = Vec::new();
        for (index, iface) in plan.interfaces.iter().enumerate() {
            let empty = iface.type_ids.is_empty()
                && iface.functions.is_empty()
                && (iface.constants.is_empty() || self.constants == ConstantMode::Skip);
            if empty {
                continue;
            }
            emitted_ifaces.push(iface.name.clone());
            p.line("");
            self.emit_interface(&mut p, registry, &plan, index)?;
        }

        p.line("");
        p.open(&format!("world {}", to_kebab(&self.world)));
        for name in &emitted_ifaces {
            p.line(&format!("import {name};"));
        }
        p.close();

        let path = format!("wit/{}.wit", to_kebab(&self.world));
        let content = p.finish();

        // Validate through the reference WIT implementation so structural
        // rules the emitter cannot express locally (no recursive value types,
        // acyclic interface `use`s, non-empty enums/records, ...) become
        // generation-time errors instead of broken output.
        let mut resolve = wit_parser::Resolve::new();
        resolve
            .push_str(&path, &content)
            .map_err(|e| WitGenError::InvalidWit {
                message: format!("{e:?}"),
            })?;

        Ok(GeneratedOutput {
            files: vec![GeneratedFile {
                path,
                content: content.into_bytes(),
                encoding: Some("utf-8".to_string()),
            }],
        })
    }
}

impl WitGenerator {
    fn emit_interface(
        &self,
        p: &mut Printer,
        registry: &ValidatedRegistry<'_>,
        plan: &Plan<'_>,
        index: usize,
    ) -> Result<(), WitGenError> {
        let iface = &plan.interfaces[index];
        p.doc(iface.doc);
        p.open(&format!("interface {}", iface.name));

        for (owner, names) in plan.uses_for(registry, index) {
            p.line(&format!("use {owner}.{{{}}};", names.join(", ")));
        }

        let mut member_names = NameMap::new();

        for id in &iface.type_ids {
            match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
                TypeKind::Struct(s) => {
                    if plan.is_resource(id) {
                        emit_resource(p, s, plan)?;
                    } else {
                        emit_record(p, s, plan)?;
                    }
                }
                TypeKind::Enum(e) => emit_enum(p, e, plan, &mut member_names)?,
                TypeKind::TypeAlias(a) => emit_alias(p, a, plan, &mut member_names)?,
            }
        }

        // Companion functions for enum methods (WIT enums/variants have none).
        for id in &iface.type_ids {
            if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(id)) {
                let enum_name = plan.type_name(id);
                for m in e.methods {
                    let fn_name = member_names.insert(&format!("{}_{}", e.name, m.name))?;
                    emit_function(p, m, Some((enum_name, id)), &fn_name, plan)?;
                }
            }
        }

        for func in iface.functions {
            let name = member_names.insert(func.name)?;
            emit_function(p, func, None, &name, plan)?;
        }

        if self.constants == ConstantMode::Getter {
            for c in iface.constants {
                let name = member_names.insert(c.name)?;
                emit_constant(p, c, &name, plan)?;
            }
        }

        p.close();
        Ok(())
    }
}

fn validate_package_name(package: &str) -> Result<(), WitGenError> {
    let err = || WitGenError::InvalidPackageName(package.to_string());
    let (ns, name) = package.split_once(':').ok_or_else(err)?;
    for segment in [ns, name] {
        if segment.is_empty()
            || !segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || segment.starts_with('-')
            || segment.ends_with('-')
        {
            return Err(err());
        }
    }
    Ok(())
}

fn emit_record(
    p: &mut Printer,
    s: &StructDescriptor<'_>,
    plan: &Plan<'_>,
) -> Result<(), WitGenError> {
    p.doc(s.doc);
    p.open(&format!("record {}", plan.type_name(s.id.as_str())));
    let mut field_names = NameMap::new();
    for field in s.fields {
        let context = format!("{}.{}", s.name, field.name);
        p.doc(field.doc);
        if field.readonly {
            p.doc(Some("Readonly in the source API."));
        }
        let name = field_names.insert(field.name)?;
        let ty = render_type(field.ty, Pos::Field, plan, &context)?;
        p.line(&format!("{name}: {ty},"));
    }
    p.close();
    Ok(())
}

fn emit_resource(
    p: &mut Printer,
    s: &StructDescriptor<'_>,
    plan: &Plan<'_>,
) -> Result<(), WitGenError> {
    let res_name = plan.type_name(s.id.as_str()).to_string();
    p.doc(s.doc);
    p.open(&format!("resource {res_name}"));
    let mut members = NameMap::new();

    for field in s.fields {
        let context = format!("{}.{}", s.name, field.name);
        let ty = render_type(field.ty, Pos::Return(Ownership::Owned), plan, &context)?;
        p.doc(field.doc);
        let getter = members.insert(field.name)?;
        p.line(&format!("{getter}: func() -> {ty};"));
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            let vty = render_type(field.ty, Pos::Param(Ownership::Owned), plan, &context)?;
            p.line(&format!("{setter}: func(value: {vty});"));
        }
    }

    for prop in s.properties {
        let context = format!("{}.{}", s.name, prop.name);
        let ty = render_type(prop.ty, Pos::Return(Ownership::Owned), plan, &context)?;
        p.doc(prop.doc);
        let getter = members.insert(prop.name)?;
        p.line(&format!("{getter}: func() -> {ty};"));
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            let vty = render_type(prop.ty, Pos::Param(Ownership::Owned), plan, &context)?;
            p.line(&format!("{setter}: func(value: {vty});"));
        }
    }

    // WIT `constructor` cannot be async: the first sync constructor gets the
    // slot, every other constructor becomes a static function.
    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        let context = format!("{}::{}", s.name, ctor.name);
        p.doc(ctor.doc);
        let params = render_params(ctor, plan, &context)?;
        if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            p.line(&format!("constructor({params});"));
        } else {
            let name = members.insert(ctor.name)?;
            let kw = fn_keyword(ctor);
            p.line(&format!("{name}: static {kw}({params}) -> {res_name};"));
        }
    }

    for m in s.methods {
        let context = format!("{}::{}", s.name, m.name);
        p.doc(m.doc);
        if let Some(kind) = m.error_kind {
            p.doc(Some(&format!("Errors: {kind}")));
        }
        let name = members.insert(m.name)?;
        let params = render_params(m, plan, &context)?;
        let ret = render_fn_return(m, plan, &context)?;
        let kw = fn_keyword(m);
        match m.receiver {
            Some(Receiver::Ref | Receiver::RefMut) => {
                p.line(&format!("{name}: {kw}({params}){ret};"));
            }
            Some(Receiver::Owned) => {
                let sep = if params.is_empty() { "" } else { ", " };
                p.line(&format!(
                    "{name}: static {kw}(this: {res_name}{sep}{params}){ret};"
                ));
            }
            None => {
                p.line(&format!("{name}: static {kw}({params}){ret};"));
            }
        }
    }

    p.close();
    Ok(())
}

fn emit_enum(
    p: &mut Printer,
    e: &EnumDescriptor<'_>,
    plan: &Plan<'_>,
    names: &mut NameMap,
) -> Result<(), WitGenError> {
    let enum_name = plan.type_name(e.id.as_str()).to_string();
    let unit_only = e
        .variants
        .iter()
        .all(|v| matches!(v.kind, VariantKind::Unit));

    // Struct variants get a synthesized record emitted first.
    if !unit_only {
        for v in e.variants {
            if let VariantKind::Struct(fields) = v.kind {
                let rec_name = names.insert(&format!("{}_{}", e.name, v.name))?;
                p.doc(Some(&format!("Payload of `{}.{}`.", e.name, v.name)));
                p.open(&format!("record {rec_name}"));
                let mut field_names = NameMap::new();
                for field in fields {
                    let context = format!("{}.{}.{}", e.name, v.name, field.name);
                    p.doc(field.doc);
                    let fname = field_names.insert(field.name)?;
                    let ty = render_type(field.ty, Pos::Field, plan, &context)?;
                    p.line(&format!("{fname}: {ty},"));
                }
                p.close();
            }
        }
    }

    p.doc(e.doc);
    let mut case_names = NameMap::new();
    if unit_only {
        p.open(&format!("enum {enum_name}"));
        for v in e.variants {
            p.doc(v.doc);
            p.line(&format!("{},", case_names.insert(v.name)?));
        }
    } else {
        p.open(&format!("variant {enum_name}"));
        for v in e.variants {
            p.doc(v.doc);
            let case = case_names.insert(v.name)?;
            match v.kind {
                VariantKind::Unit => p.line(&format!("{case},")),
                VariantKind::Tuple(elems) => {
                    let context = format!("{}.{}", e.name, v.name);
                    let payload = if elems.len() == 1 {
                        render_type(&elems[0], Pos::Field, plan, &context)?
                    } else {
                        render_type(&TypeDescriptor::Tuple(elems), Pos::Field, plan, &context)?
                    };
                    p.line(&format!("{case}({payload}),"));
                }
                VariantKind::Struct(_) => {
                    p.line(&format!(
                        "{case}({}),",
                        to_kebab(&format!("{}_{}", e.name, v.name))
                    ));
                }
            }
        }
    }
    p.close();
    Ok(())
}

fn emit_alias(
    p: &mut Printer,
    a: &TypeAliasDescriptor<'_>,
    plan: &Plan<'_>,
    _names: &mut NameMap,
) -> Result<(), WitGenError> {
    let context = format!("type alias {}", a.name);
    p.doc(a.doc);
    let ty = render_type(a.inner, Pos::Field, plan, &context)?;
    p.line(&format!("type {} = {ty};", plan.type_name(a.id.as_str())));
    Ok(())
}

fn emit_function(
    p: &mut Printer,
    f: &FunctionDescriptor<'_>,
    enum_receiver: Option<(&str, &str)>,
    name: &str,
    plan: &Plan<'_>,
) -> Result<(), WitGenError> {
    let context = f.name.to_string();
    p.doc(f.doc);

    let mut params = Vec::new();
    if let Some((enum_name, _)) = enum_receiver {
        params.push(format!("this: {enum_name}"));
    }
    let mut param_names = NameMap::new();
    for param in f.params {
        let pname = param_names.insert(param.name)?;
        let ty = render_type(param.ty, Pos::Param(param.ownership), plan, &context)?;
        params.push(format!("{pname}: {ty}"));
    }

    if let Some(kind) = f.error_kind {
        p.doc(Some(&format!("Errors: {kind}")));
    }
    let arrow = render_fn_return(f, plan, &context)?;
    let kw = fn_keyword(f);
    p.line(&format!("{name}: {kw}({}){arrow};", params.join(", ")));
    Ok(())
}

fn fn_keyword(f: &FunctionDescriptor<'_>) -> &'static str {
    if f.is_async { "async func" } else { "func" }
}

fn emit_constant(
    p: &mut Printer,
    c: &ConstantDescriptor<'_>,
    name: &str,
    plan: &Plan<'_>,
) -> Result<(), WitGenError> {
    let context = format!("constant {}", c.name);
    p.doc(c.doc);
    p.doc(Some(&format!("Constant value: {}", c.value)));
    let ty = render_type(c.ty, Pos::Return(Ownership::Owned), plan, &context)?;
    p.line(&format!("{name}: func() -> {ty};"));
    Ok(())
}

fn render_params(
    f: &FunctionDescriptor<'_>,
    plan: &Plan<'_>,
    context: &str,
) -> Result<String, WitGenError> {
    let mut names = NameMap::new();
    let mut out = Vec::new();
    for param in f.params {
        let name = names.insert(param.name)?;
        let ty = render_type(param.ty, Pos::Param(param.ownership), plan, context)?;
        out.push(format!("{name}: {ty}"));
    }
    Ok(out.join(", "))
}

fn render_fn_return(
    f: &FunctionDescriptor<'_>,
    plan: &Plan<'_>,
    context: &str,
) -> Result<String, WitGenError> {
    let mut ret = render_return(f.return_type, f.return_ownership, plan, context)?;
    if f.error_kind.is_some() && !matches!(f.return_type, TypeDescriptor::Result(_, _)) {
        ret = Some(match ret {
            Some(t) => format!("result<{t}>"),
            None => "result".to_string(),
        });
    }
    Ok(match ret {
        Some(t) => format!(" -> {t}"),
        None => String::new(),
    })
}
