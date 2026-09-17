//! WIT (WebAssembly Component Model Interface Types) binding generator for
//! [haphe](https://crates.io/crates/haphe).
//!
//! [`WitGenerator`] implements [`BindingGenerator`], turning a haphe type
//! registry into a `.wit` document: modules become interfaces, structs become
//! records or resources, enums become `enum`s or `variant`s, and a world
//! lists every interface. Provided (Rust-implemented) interfaces and foreign
//! (counterpart-implemented) interfaces map to `import`/`export` lines
//! according to the configured [`WorldPerspective`].
//!
//! Identifiers are implicitly converted to kebab-case because the WIT grammar
//! admits no other casing; see the crate README for details.
//!
//! With the `runtime` feature, the two directions split cleanly:
//! `WasmBinder` binds provided (Rust-implemented) interfaces into a wasmtime
//! `Linker`, while the free `foreign_handle`/`foreign_caller` functions (and
//! their `_in`/`_async` variants) connect to a live instantiated component's
//! foreign exports — no binder or registration machinery, just a store, an
//! instance, and a package address.

#[cfg(feature = "dyn-generics")]
mod dyn_gen;
/// Inert stand-ins without the `dyn-generics` feature: [`dyn_gen::is_dyn`]
/// is constantly `false`, so every dispatcher path is dead and none of the
/// emission machinery compiles.
#[cfg(not(feature = "dyn-generics"))]
mod dyn_gen {
    #![allow(dead_code)]

    use crate::WitGenError;
    use crate::emit::Printer;
    use crate::model::{Env, Plan};
    use haphe::FunctionDescriptor;

    pub(crate) fn is_dyn(_f: &FunctionDescriptor<'_>) -> bool {
        false
    }

    pub(crate) struct DispatcherSig {
        pub raw_name: String,
        pub marker: String,
    }

    impl DispatcherSig {
        pub fn param_list(&self) -> String {
            unreachable!("dyn-generics feature is off")
        }
        pub fn arrow(&self) -> String {
            unreachable!("dyn-generics feature is off")
        }
        pub fn keyword(&self) -> &'static str {
            unreachable!("dyn-generics feature is off")
        }
    }

    pub(crate) struct DynCx;

    impl DynCx {
        pub fn new() -> Self {
            DynCx
        }

        pub fn dispatcher(
            &mut self,
            _p: &mut Printer,
            _f: &FunctionDescriptor<'_>,
            _owner: Option<&str>,
            _plan: &Plan<'_>,
            _env: Option<&Env<'_, '_>>,
        ) -> Result<DispatcherSig, WitGenError> {
            unreachable!("dyn-generics feature is off")
        }
    }
}
mod emit;
#[cfg(feature = "runtime")]
mod host;
mod model;
pub mod names;
#[cfg(feature = "runtime")]
mod runtime;
mod types;

#[cfg(feature = "runtime")]
pub use host::GuestResource;
#[cfg(feature = "runtime")]
pub use runtime::{
    WasmBindError, WasmBinder, foreign_caller, foreign_caller_async, foreign_caller_async_in,
    foreign_caller_in, foreign_handle, foreign_handle_async, foreign_handle_async_in,
    foreign_handle_in,
};

use std::fmt;

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, EnumDescriptor, FunctionDescriptor,
    GeneratedFile, GeneratedOutput, Ownership, Receiver, StructDescriptor, TypeAliasDescriptor,
    TypeDescriptor, TypeKind, ValidatedRegistry, VariantKind,
};

use emit::Printer;
use model::{Direction, Env, Plan, ProjKind, Projected, projected_trait_members};
use names::{NameMap, to_kebab};
use types::{Pos, render_return, render_type};

/// Which side of the component boundary the generated world is targeted by.
///
/// A world's `import`/`export` lines are written from the perspective of the
/// component that targets it. The haphe program can sit on either side, so
/// the mapping of provided (Rust-implemented) and foreign (counterpart-
/// implemented) interfaces flips with the perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorldPerspective {
    /// The world is targeted by the haphe program's counterpart (default):
    /// provided interfaces are `import`s, foreign interfaces are `export`s.
    #[default]
    Peer,
    /// The world is targeted by the haphe program itself (compiled as a
    /// component): provided interfaces are `export`s, foreign interfaces are
    /// `import`s.
    Own,
}

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

/// Generates a WIT document with a world listing every interface, provided
/// and foreign directions mapped per the configured [`WorldPerspective`].
#[derive(Debug, Clone)]
pub struct WitGenerator {
    package: String,
    version: Option<String>,
    world: String,
    default_interface: String,
    constants: ConstantMode,
    perspective: WorldPerspective,
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
            perspective: WorldPerspective::default(),
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

    /// Sets which side of the component boundary targets the generated world
    /// (default [`WorldPerspective::Peer`]).
    pub fn with_world_perspective(mut self, perspective: WorldPerspective) -> Self {
        self.perspective = perspective;
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
    /// A flags enum records a bit value that does not equal
    /// `1 << declaration_index`: WIT `flags` are position-based, so a
    /// gap-bearing or out-of-order mask cannot be represented faithfully.
    FlagsBitMismatch {
        /// The flags enum's exposed name.
        name: String,
        /// The offending flag.
        flag: String,
        /// The bit value WIT would assign (`1 << declaration_index`).
        expected: i64,
        /// The recorded bit value.
        actual: i64,
    },
    /// The generated document failed WIT resolution (e.g. recursive value
    /// types, cyclic interface `use`s, empty enums or records). Caught at
    /// generation time by validating the output through `wit-parser`.
    InvalidWit {
        /// The wit-parser diagnostic.
        message: String,
    },
    /// A type references a generic instantiation that was never recorded in
    /// the registry, so no monomorphized definition exists to emit.
    UnregisteredInstantiation {
        /// The instantiation's mangled WIT name.
        name: String,
    },
    /// A generic foreign interface; WIT dispatches by name and has no
    /// generics, so monomorphized interfaces need a naming extension that is
    /// not yet available.
    GenericForeignInterface {
        /// The interface's name.
        name: String,
    },
    /// A generic function; WIT dispatches by name and has no generics, so
    /// monomorphized functions need a naming extension that is not yet
    /// available.
    GenericFunction {
        /// The containing interface's WIT name.
        interface: String,
        /// The function's name.
        function: String,
    },
    /// A foreign function declaring `dyn` (erased) dispatch; the single
    /// variant-typed import it addresses exists only with the
    /// `dyn-generics` feature.
    DynForeignFunction {
        /// The containing interface's WIT name.
        interface: String,
        /// The function's name.
        function: String,
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
            Self::FlagsBitMismatch {
                name,
                flag,
                expected,
                actual,
            } => write!(
                f,
                "flags enum `{name}`: flag `{flag}` records bit value {actual}, but WIT flags \
                 are position-based and would assign {expected}; gap-bearing or reordered \
                 masks cannot be represented"
            ),
            Self::InvalidWit { message } => {
                write!(f, "generated document is not valid WIT: {message}")
            }
            Self::UnregisteredInstantiation { name } => write!(
                f,
                "generic instantiation `{name}` is referenced but not listed in the registry"
            ),
            Self::GenericForeignInterface { name } => write!(
                f,
                "foreign interface `{name}` is generic; WIT dispatches by name only and \
                 cannot distinguish instantiations"
            ),
            Self::GenericFunction {
                interface,
                function,
            } => write!(
                f,
                "function `{function}` in interface `{interface}` is generic; WIT dispatches \
                 by name only and cannot distinguish instantiations"
            ),
            Self::DynForeignFunction {
                interface,
                function,
            } => write!(
                f,
                "foreign function `{function}` in interface `{interface}` declares `dyn` \
                 (erased) dispatch; enable the `dyn-generics` feature of haphe-wit to \
                 address its single variant-typed import"
            ),
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
        // WIT guests always name a monomorph statically, so bare dyn
        // dispatch is representable only via the `dyn-generics` feature's
        // injected variant-based dispatchers — the capability reports true
        // exactly when that machinery is compiled in (see the README's
        // dyn-generics section). The runtime binder inherits this via
        // delegation, so both capability sites agree.
        BackendCapabilities::ALL
            .with_callbacks(false)
            .with_dyn_generics(cfg!(feature = "dyn-generics"))
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
                && iface.instance_indices.is_empty()
                && iface.functions.is_empty()
                && (iface.constants.is_empty() || self.constants == ConstantMode::Skip);
            if empty {
                continue;
            }
            emitted_ifaces.push((iface.name.clone(), iface.direction));
            p.line("");
            self.emit_interface(&mut p, registry, &plan, index)?;
        }

        p.line("");
        p.open(&format!("world {}", to_kebab(&self.world)));
        for (name, direction) in &emitted_ifaces {
            let keyword = match (self.perspective, direction) {
                (WorldPerspective::Peer, Direction::Provided)
                | (WorldPerspective::Own, Direction::Foreign) => "import",
                (WorldPerspective::Peer, Direction::Foreign)
                | (WorldPerspective::Own, Direction::Provided) => "export",
            };
            p.line(&format!("{keyword} {name};"));
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
        let base_env = iface
            .foreign_instance
            .map(|i| plan.env_for(registry, &plan.instances[i]));
        p.doc(iface.doc);
        if let Some(i) = iface.foreign_instance {
            let marker = plan.instance_marker(&plan.instances[i])?;
            p.doc(Some(&format!("haphe:generic-instance = {marker}")));
        }
        p.open(&format!("interface {}", iface.name));

        for (owner, names) in plan.uses_for(registry, index)? {
            let names: Vec<String> = names.into_iter().collect();
            p.line(&format!("use {owner}.{{{}}};", names.join(", ")));
        }

        let mut member_names = NameMap::new();
        let mut dyn_cx = dyn_gen::DynCx::new();

        for id in &iface.type_ids {
            match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
                TypeKind::Struct(s) => {
                    let name = plan.type_name(id).to_string();
                    if plan.is_resource(id) {
                        // Dispatcher variant types are interface-level, so
                        // they precede the resource block that uses them.
                        let dyn_sigs = dyn_method_sigs(&mut dyn_cx, p, s, &name, plan, None)?;
                        emit_resource(p, s, &name, plan, None, &dyn_sigs)?;
                    } else {
                        emit_record(p, s, &name, plan, None)?;
                    }
                }
                TypeKind::Enum(e) => {
                    let name = plan.type_name(id).to_string();
                    emit_enum(p, e, &name, plan, None, &mut member_names)?;
                }
                TypeKind::TypeAlias(a) => emit_alias(p, a, plan)?,
            }
        }

        // Monomorphized generic instantiations, each marked with a
        // deterministic `haphe:generic-instance` doc line.
        for &i in &iface.instance_indices {
            let inst = &plan.instances[i];
            let env = plan.env_for(registry, inst);
            let marker = plan.instance_marker(inst)?;
            p.doc(Some(&format!("haphe:generic-instance = {marker}")));
            match registry
                .get_type(&haphe::TypeId::new(inst.erased_id))
                .unwrap()
            {
                TypeKind::Struct(s) => {
                    if plan.is_resource(inst.erased_id) {
                        let dyn_sigs =
                            dyn_method_sigs(&mut dyn_cx, p, s, &inst.wit_name, plan, Some(&env))?;
                        emit_resource(p, s, &inst.wit_name, plan, Some(&env), &dyn_sigs)?;
                    } else {
                        emit_record(p, s, &inst.wit_name, plan, Some(&env))?;
                    }
                }
                TypeKind::Enum(e) => {
                    emit_enum(p, e, &inst.wit_name, plan, Some(&env), &mut member_names)?;
                }
                TypeKind::TypeAlias(_) => unreachable!("aliases cannot be generic"),
            }
        }

        // Companion functions for enum methods (WIT enums/variants have
        // none). Generic methods emit one mangled companion per declared
        // instantiation, like resource methods.
        for id in &iface.type_ids {
            if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(id)) {
                let enum_name = plan.type_name(id).to_string();
                for m in e.methods {
                    if m.generic_params.is_empty() {
                        let fn_name = member_names.insert(&format!("{}_{}", e.name, m.name))?;
                        emit_function(p, m, Some(&enum_name), &fn_name, plan, None)?;
                        continue;
                    }
                    for args in m.instantiations {
                        let mangled = plan.mangle_fn_instance(m.name, args, None)?;
                        let fn_name = member_names.insert(&format!("{}_{}", e.name, mangled))?;
                        let fenv = plan.fn_env(m, args, None);
                        emit_function(p, m, Some(&enum_name), &fn_name, plan, Some(&fenv))?;
                    }
                    if dyn_gen::is_dyn(m) {
                        let sig = dyn_cx.dispatcher(p, m, Some(&enum_name), plan, None)?;
                        let fn_name =
                            member_names.insert(&format!("{}_{}", e.name, sig.raw_name))?;
                        emit_companion_dispatcher(p, &sig, &fn_name, &enum_name);
                    }
                }
            }
        }
        for &i in &iface.instance_indices {
            let inst = &plan.instances[i];
            if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(inst.erased_id))
            {
                let env = plan.env_for(registry, inst);
                for m in e.methods {
                    if m.generic_params.is_empty() {
                        let fn_name =
                            member_names.insert(&format!("{}-{}", inst.wit_name, m.name))?;
                        emit_function(p, m, Some(&inst.wit_name), &fn_name, plan, Some(&env))?;
                        continue;
                    }
                    for args in m.instantiations {
                        let mangled = plan.mangle_fn_instance(m.name, args, Some(&env))?;
                        let fn_name =
                            member_names.insert(&format!("{}-{}", inst.wit_name, mangled))?;
                        let fenv = plan.fn_env(m, args, Some(&env));
                        emit_function(p, m, Some(&inst.wit_name), &fn_name, plan, Some(&fenv))?;
                    }
                    if dyn_gen::is_dyn(m) {
                        let sig =
                            dyn_cx.dispatcher(p, m, Some(&inst.wit_name), plan, Some(&env))?;
                        let fn_name =
                            member_names.insert(&format!("{}-{}", inst.wit_name, sig.raw_name))?;
                        emit_companion_dispatcher(p, &sig, &fn_name, &inst.wit_name);
                    }
                }
            }
        }

        // Trait projections of RECORDS (value types keep value semantics):
        // interface-level functions named `{type}-{member}`, taking `this`
        // by value; mutating projections return the updated record.
        for id in &iface.type_ids {
            if let Some(TypeKind::Struct(s)) = registry.get_type(&haphe::TypeId::new(id))
                && !plan.is_resource(id)
            {
                let rec_name = plan.type_name(id).to_string();
                for proj in projected_trait_members(s)? {
                    let raw = format!("{}_{}", s.name, proj.source_name);
                    let fn_name =
                        member_names.insert_as(&raw, &format!("trait projection `{raw}`"))?;
                    emit_projection(p, &proj, &fn_name, &rec_name, false, plan, None, s.name)?;
                }
            }
        }
        for &i in &iface.instance_indices {
            let inst = &plan.instances[i];
            if let Some(TypeKind::Struct(s)) =
                registry.get_type(&haphe::TypeId::new(inst.erased_id))
                && !plan.is_resource(inst.erased_id)
            {
                let env = plan.env_for(registry, inst);
                for proj in projected_trait_members(s)? {
                    let raw = format!("{}-{}", inst.wit_name, proj.source_name);
                    let fn_name =
                        member_names.insert_as(&raw, &format!("trait projection `{raw}`"))?;
                    emit_projection(
                        p,
                        &proj,
                        &fn_name,
                        &inst.wit_name,
                        false,
                        plan,
                        Some(&env),
                        s.name,
                    )?;
                }
            }
        }

        for func in iface.functions {
            if func.generic_params.is_empty() {
                let name = member_names.insert(func.name)?;
                emit_function(p, func, None, &name, plan, base_env.as_ref())?;
                continue;
            }
            // A `dyn` FOREIGN function declares erased addressing: the host
            // implements exactly ONE function under the plain name — no
            // monomorph imports, which would demand guest exports that do
            // not exist. Generic-typed positions cross as the shared case
            // variants (`dyn-generics` feature), so the case tag still tells
            // the host which instantiation was meant.
            if dyn_gen::is_dyn(func) && iface.direction == Direction::Foreign {
                let sig = dyn_cx.dispatcher(p, func, None, plan, base_env.as_ref())?;
                let name = member_names.insert(func.name)?;
                p.doc(Some(&format!("haphe:dyn-foreign = {}", func.name)));
                p.line(&format!(
                    "{name}: {}({}){};",
                    sig.keyword(),
                    sig.param_list(),
                    sig.arrow()
                ));
                continue;
            }
            // One deterministically-named monomorph per declared
            // instantiation — fn-site and registry-level sources unioned
            // (`generics` feature; rejected in `Plan::build` otherwise).
            for args in haphe::union_instantiations(func, iface.fn_instantiations) {
                let mangled = plan.mangle_fn_instance(func.name, args, base_env.as_ref())?;
                let name = member_names.insert(&mangled)?;
                let env = plan.fn_env(func, args, base_env.as_ref());
                let arg_names: Vec<String> = args
                    .iter()
                    .map(|a| plan.mangle_type(a, base_env.as_ref()))
                    .collect::<Result<_, _>>()?;
                p.doc(Some(&format!(
                    "haphe:generic-instance = {}<{}>",
                    func.name,
                    arg_names.join(", ")
                )));
                emit_function(p, func, None, &name, plan, Some(&env))?;
            }
            // A `dyn` PROVIDED function additionally synthesizes one
            // dispatcher next to its monomorphs (`dyn-generics` feature).
            if dyn_gen::is_dyn(func) && iface.direction == Direction::Provided {
                let sig = dyn_cx.dispatcher(p, func, None, plan, base_env.as_ref())?;
                let name = member_names.insert(&sig.raw_name)?;
                p.doc(Some(&sig.marker));
                p.line(&format!(
                    "{name}: {}({}){};",
                    sig.keyword(),
                    sig.param_list(),
                    sig.arrow()
                ));
            }
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

/// Prints one enum-companion dispatcher line: interface-level, `this` (the
/// enum value) prepended to the dispatcher's own parameters.
fn emit_companion_dispatcher(
    p: &mut Printer,
    sig: &dyn_gen::DispatcherSig,
    fn_name: &str,
    enum_name: &str,
) {
    p.doc(Some(&sig.marker));
    let params = sig.param_list();
    let sep = if params.is_empty() { "" } else { ", " };
    p.line(&format!(
        "{fn_name}: {}(this: {enum_name}{sep}{params}){};",
        sig.keyword(),
        sig.arrow()
    ));
}

/// Builds the dispatcher signatures for a resource's `dyn` methods, emitting
/// their variant type definitions at the CURRENT printer position (interface
/// level — call before opening the resource block). Returns
/// `(method name, signature)` pairs for [`emit_resource`] to print members
/// from; empty without the `dyn-generics` feature.
fn dyn_method_sigs(
    cx: &mut dyn_gen::DynCx,
    p: &mut Printer,
    s: &StructDescriptor<'_>,
    owner: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
) -> Result<Vec<(String, dyn_gen::DispatcherSig)>, WitGenError> {
    let mut out = Vec::new();
    for m in s.methods {
        if dyn_gen::is_dyn(m) {
            out.push((
                m.name.to_string(),
                cx.dispatcher(p, m, Some(owner), plan, env)?,
            ));
        }
    }
    Ok(out)
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
    wit_name: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
) -> Result<(), WitGenError> {
    p.doc(s.doc);
    p.open(&format!("record {wit_name}"));
    let mut field_names = NameMap::new();
    for field in s.fields {
        let context = format!("{}.{}", s.name, field.name);
        p.doc(field.doc);
        if field.readonly {
            p.doc(Some("Readonly in the source API."));
        }
        let name = field_names.insert(field.name)?;
        let ty = render_type(field.ty, Pos::Field, plan, env, &context)?;
        p.line(&format!("{name}: {ty},"));
    }
    p.close();
    Ok(())
}

fn emit_resource(
    p: &mut Printer,
    s: &StructDescriptor<'_>,
    wit_name: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    dyn_sigs: &[(String, dyn_gen::DispatcherSig)],
) -> Result<(), WitGenError> {
    p.doc(s.doc);
    p.open(&format!("resource {wit_name}"));
    let mut members = NameMap::new();

    for field in s.fields {
        let context = format!("{}.{}", s.name, field.name);
        let ty = render_type(field.ty, Pos::Return(Ownership::Owned), plan, env, &context)?;
        p.doc(field.doc);
        let getter = members.insert(field.name)?;
        p.line(&format!("{getter}: func() -> {ty};"));
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            let vty = render_type(field.ty, Pos::Param(Ownership::Owned), plan, env, &context)?;
            p.line(&format!("{setter}: func(value: {vty});"));
        }
    }

    for prop in s.properties {
        let context = format!("{}.{}", s.name, prop.name);
        let ty = render_type(prop.ty, Pos::Return(Ownership::Owned), plan, env, &context)?;
        p.doc(prop.doc);
        let getter = members.insert(prop.name)?;
        p.line(&format!("{getter}: func() -> {ty};"));
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            let vty = render_type(prop.ty, Pos::Param(Ownership::Owned), plan, env, &context)?;
            p.line(&format!("{setter}: func(value: {vty});"));
        }
    }

    // WIT `constructor` cannot be async: the first sync constructor gets the
    // slot, every other constructor becomes a static function.
    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        let context = format!("{}::{}", s.name, ctor.name);
        p.doc(ctor.doc);
        let params = render_params(ctor, plan, env, &context)?;
        if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            p.line(&format!("constructor({params});"));
        } else {
            let name = members.insert(ctor.name)?;
            let kw = fn_keyword(ctor);
            p.line(&format!("{name}: static {kw}({params}) -> {wit_name};"));
        }
    }

    for m in s.methods {
        let context = format!("{}::{}", s.name, m.name);
        if m.generic_params.is_empty() {
            let name = members.insert(m.name)?;
            emit_resource_method(p, m, &name, wit_name, plan, env, &context)?;
            continue;
        }
        // Generic methods: one member per declared instantiation, under the
        // same deterministic mangled name as generic free functions, with
        // the type parameters substituted (the monomorph set is
        // dispatch-mode neutral; `dyn` additionally declares a dispatcher
        // member under the `dyn-generics` feature).
        for args in m.instantiations {
            let mangled = plan.mangle_fn_instance(m.name, args, env)?;
            let name = members.insert(&mangled)?;
            let arg_names: Vec<String> = args
                .iter()
                .map(|a| plan.mangle_type(a, env))
                .collect::<Result<_, _>>()?;
            p.doc(Some(&format!(
                "haphe:generic-instance = {}<{}>",
                m.name,
                arg_names.join(", ")
            )));
            let fenv = plan.fn_env(m, args, env);
            emit_resource_method(p, m, &name, wit_name, plan, Some(&fenv), &context)?;
        }
        if dyn_gen::is_dyn(m)
            && let Some((_, sig)) = dyn_sigs.iter().find(|(n, _)| n == m.name)
        {
            let name = members.insert(&sig.raw_name)?;
            p.doc(Some(&sig.marker));
            let (params, ret, kw) = (sig.param_list(), sig.arrow(), sig.keyword());
            match m.receiver {
                Some(Receiver::Ref | Receiver::RefMut) => {
                    p.line(&format!("{name}: {kw}({params}){ret};"));
                }
                Some(Receiver::Owned) => {
                    let sep = if params.is_empty() { "" } else { ", " };
                    p.line(&format!(
                        "{name}: static {kw}(this: {wit_name}{sep}{params}){ret};"
                    ));
                }
                None => {
                    p.line(&format!("{name}: static {kw}({params}){ret};"));
                }
            }
        }
    }

    // Trait projections: every declared trait impl surfaces as a named
    // member (interusability). Same NameMap as user members, so a
    // user-declared `add`/`at`/`call`/... collides descriptively.
    for proj in projected_trait_members(s)? {
        let name = members.insert_as(
            proj.source_name,
            &format!("trait projection `{}`", proj.source_name),
        )?;
        emit_projection(p, &proj, &name, wit_name, true, plan, env, s.name)?;
    }

    p.close();
    Ok(())
}

/// Renders one resource member line for a method (plain, or one generic
/// monomorph under its mangled `name` with the instantiation's `env`).
fn emit_resource_method(
    p: &mut Printer,
    m: &FunctionDescriptor<'_>,
    name: &str,
    wit_name: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<(), WitGenError> {
    p.doc(m.doc);
    if let Some(kind) = m.error_kind {
        p.doc(Some(&format!("Errors: {kind}")));
    }
    let params = render_params(m, plan, env, context)?;
    let ret = render_fn_return(m, plan, env, context)?;
    let kw = fn_keyword(m);
    match m.receiver {
        Some(Receiver::Ref | Receiver::RefMut) => {
            p.line(&format!("{name}: {kw}({params}){ret};"));
        }
        Some(Receiver::Owned) => {
            let sep = if params.is_empty() { "" } else { ", " };
            p.line(&format!(
                "{name}: static {kw}(this: {wit_name}{sep}{params}){ret};"
            ));
        }
        None => {
            p.line(&format!("{name}: static {kw}({params}){ret};"));
        }
    }
    Ok(())
}

/// Renders one trait projection. `receiver` is the WIT self type name; for
/// resources (`as_resource`) members are methods/statics on the resource,
/// for records they are interface-level functions taking `this` by value
/// (mutating projections then return the updated record).
#[allow(clippy::too_many_arguments)]
fn emit_projection(
    p: &mut Printer,
    proj: &Projected<'_>,
    name: &str,
    receiver: &str,
    as_resource: bool,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<(), WitGenError> {
    let context = format!("{context}::{name}");
    // Receiver rendering differs: resources dispatch on the handle
    // implicitly (methods) or via `this` (statics); records take `this` by
    // value as the first parameter.
    let this_param = |empty: bool| {
        if as_resource {
            String::new()
        } else if empty {
            format!("this: {receiver}")
        } else {
            format!("this: {receiver}, ")
        }
    };
    let rhs_self = if as_resource {
        format!("borrow<{receiver}>")
    } else {
        receiver.to_string()
    };
    let ret_ty = |ty: &TypeDescriptor<'_>| -> Result<String, WitGenError> {
        render_type(ty, Pos::Return(Ownership::Owned), plan, env, &context)
    };
    let param_ty = |ty: &TypeDescriptor<'_>| -> Result<String, WitGenError> {
        render_type(ty, Pos::Param(Ownership::Owned), plan, env, &context)
    };
    match &proj.kind {
        ProjKind::ArithSelf { output, .. } => {
            let out = ret_ty(output)?;
            p.line(&format!(
                "{name}: func({}rhs: {rhs_self}) -> {out};",
                this_param(false)
            ));
        }
        ProjKind::ArithScalar { rhs, output, .. } => {
            let rhs = param_ty(rhs)?;
            let out = ret_ty(output)?;
            p.line(&format!(
                "{name}: func({}rhs: {rhs}) -> {out};",
                this_param(false)
            ));
        }
        ProjKind::Neg { output } | ProjKind::BNot { output } => {
            let out = ret_ty(output)?;
            p.line(&format!("{name}: func({}) -> {out};", this_param(true)));
        }
        ProjKind::Eq | ProjKind::Lt | ProjKind::Le => {
            p.line(&format!(
                "{name}: func({}other: {rhs_self}) -> bool;",
                this_param(false)
            ));
        }
        ProjKind::ToString | ProjKind::DebugString => {
            p.line(&format!("{name}: func({}) -> string;", this_param(true)));
        }
        ProjKind::Hash => {
            p.line(&format!("{name}: func({}) -> u64;", this_param(true)));
        }
        ProjKind::Call {
            args,
            output,
            is_async,
        } => {
            let mut params: Vec<String> = Vec::new();
            if !as_resource {
                params.push(format!("this: {receiver}"));
            }
            for (i, a) in args.iter().enumerate() {
                params.push(format!("a{i}: {}", param_ty(a)?));
            }
            let kw = if *is_async { "async func" } else { "func" };
            let ret = match output {
                TypeDescriptor::Unit => String::new(),
                other => format!(" -> {}", ret_ty(other)?),
            };
            p.line(&format!("{name}: {kw}({}){ret};", params.join(", ")));
        }
        ProjKind::IndexGet { index, output } => {
            let idx = param_ty(index)?;
            let out = ret_ty(output)?;
            p.line(&format!(
                "{name}: func({}index: {idx}) -> {out};",
                this_param(false)
            ));
        }
        ProjKind::IndexSet { index, output } => {
            let idx = param_ty(index)?;
            let val = param_ty(output)?;
            // Value semantics for records: the updated record comes back.
            let ret = if as_resource {
                String::new()
            } else {
                format!(" -> {receiver}")
            };
            p.line(&format!(
                "{name}: func({}index: {idx}, value: {val}){ret};",
                this_param(false)
            ));
        }
        ProjKind::Items { item } => {
            p.doc(Some(
                "Eager snapshot of the iteration: materializes every item at \
                 call time (lazy iteration is not WIT-native).",
            ));
            let item = ret_ty(item)?;
            p.line(&format!(
                "{name}: func({}) -> list<{item}>;",
                this_param(true)
            ));
        }
        ProjKind::Length => {
            p.line(&format!("{name}: func({}) -> u64;", this_param(true)));
        }
        ProjKind::Default => {
            if as_resource {
                p.line(&format!("{name}: static func() -> {receiver};"));
            } else {
                p.line(&format!("{name}: func() -> {receiver};"));
            }
        }
    }
    Ok(())
}

fn emit_enum(
    p: &mut Printer,
    e: &EnumDescriptor<'_>,
    wit_name: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    names: &mut NameMap,
) -> Result<(), WitGenError> {
    let unit_only = e
        .variants
        .iter()
        .all(|v| matches!(v.kind, VariantKind::Unit));

    // Struct variants get a synthesized record emitted first.
    if !unit_only {
        for v in e.variants {
            if let VariantKind::Struct(fields) = v.kind {
                let rec_name = names.insert(&format!("{wit_name}-{}", v.name))?;
                p.doc(Some(&format!("Payload of `{}.{}`.", e.name, v.name)));
                p.open(&format!("record {rec_name}"));
                let mut field_names = NameMap::new();
                for field in fields {
                    let context = format!("{}.{}.{}", e.name, v.name, field.name);
                    p.doc(field.doc);
                    let fname = field_names.insert(field.name)?;
                    let ty = render_type(field.ty, Pos::Field, plan, env, &context)?;
                    p.line(&format!("{fname}: {ty},"));
                }
                p.close();
            }
        }
    }

    p.doc(e.doc);
    let mut case_names = NameMap::new();
    if e.is_flags {
        // Validated upstream: flags enums have only unit variants. WIT
        // `flags` assign bits by declaration order, so a recorded real bit
        // value must equal `1 << index` — anything else would silently
        // misrepresent the mask. (No recorded value = positional by
        // definition, accepted.)
        p.open(&format!("flags {wit_name}"));
        for (i, v) in e.variants.iter().enumerate() {
            if let Some(actual) = v.discriminant {
                let expected = 1i64.checked_shl(i as u32).unwrap_or(0);
                if actual != expected {
                    return Err(WitGenError::FlagsBitMismatch {
                        name: e.name.to_string(),
                        flag: v.name.to_string(),
                        expected,
                        actual,
                    });
                }
            }
            p.doc(v.doc);
            p.line(&format!("{},", case_names.insert(v.name)?));
        }
    } else if unit_only {
        // WIT enums are name-based with no numeric-repr slot; an enum with a
        // Rust `#[repr]` integer type still emits by case name, and the
        // exact-type propagation is noted for readers.
        if let Some(repr) = e.repr {
            p.doc(Some(&format!(
                "Numeric representation in the source API: {repr:?}."
            )));
        }
        p.open(&format!("enum {wit_name}"));
        for v in e.variants {
            p.doc(v.doc);
            p.line(&format!("{},", case_names.insert(v.name)?));
        }
    } else {
        p.open(&format!("variant {wit_name}"));
        for v in e.variants {
            p.doc(v.doc);
            let case = case_names.insert(v.name)?;
            match v.kind {
                VariantKind::Unit => p.line(&format!("{case},")),
                VariantKind::Tuple(elems) => {
                    let context = format!("{}.{}", e.name, v.name);
                    let payload = if elems.len() == 1 {
                        render_type(&elems[0], Pos::Field, plan, env, &context)?
                    } else {
                        render_type(
                            &TypeDescriptor::Tuple(elems),
                            Pos::Field,
                            plan,
                            env,
                            &context,
                        )?
                    };
                    p.line(&format!("{case}({payload}),"));
                }
                VariantKind::Struct(_) => {
                    p.line(&format!(
                        "{case}({}),",
                        to_kebab(&format!("{wit_name}-{}", v.name))
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
) -> Result<(), WitGenError> {
    let context = format!("type alias {}", a.name);
    p.doc(a.doc);
    let ty = render_type(a.inner, Pos::Field, plan, None, &context)?;
    p.line(&format!("type {} = {ty};", plan.type_name(a.id.as_str())));
    Ok(())
}

fn emit_function(
    p: &mut Printer,
    f: &FunctionDescriptor<'_>,
    enum_receiver: Option<&str>,
    name: &str,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
) -> Result<(), WitGenError> {
    let context = f.name.to_string();
    p.doc(f.doc);

    let mut params = Vec::new();
    if let Some(enum_name) = enum_receiver {
        params.push(format!("this: {enum_name}"));
    }
    let mut param_names = NameMap::new();
    for param in f.params {
        let pname = param_names.insert(param.name)?;
        let ty = render_type(param.ty, Pos::Param(param.ownership), plan, env, &context)?;
        params.push(format!("{pname}: {ty}"));
    }

    if let Some(kind) = f.error_kind {
        p.doc(Some(&format!("Errors: {kind}")));
    }
    let arrow = render_fn_return(f, plan, env, &context)?;
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
    let ty = render_type(c.ty, Pos::Return(Ownership::Owned), plan, None, &context)?;
    p.line(&format!("{name}: func() -> {ty};"));
    Ok(())
}

fn render_params(
    f: &FunctionDescriptor<'_>,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<String, WitGenError> {
    let mut names = NameMap::new();
    let mut out = Vec::new();
    for param in f.params {
        let name = names.insert(param.name)?;
        let ty = render_type(param.ty, Pos::Param(param.ownership), plan, env, context)?;
        out.push(format!("{name}: {ty}"));
    }
    Ok(out.join(", "))
}

fn render_fn_return(
    f: &FunctionDescriptor<'_>,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<String, WitGenError> {
    let mut ret = render_return(f.return_type, f.return_ownership, plan, env, context)?;
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
