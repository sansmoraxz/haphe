use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, FnBinder, ForeignCaller,
    ForeignError, ForeignErrorKind, ForeignHandle, ForeignInterfaceDescriptor, FromScript,
    IntoScript, PrimitiveType, Receiver, RuntimeBinder, ScriptBind, ScriptBindFn,
    ScriptConvertError, ScriptForeign, ScriptStruct, ScriptValue, ThreadSafety, TypeDescriptor,
    TypeKind, ValidatedRegistry,
};
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{
    Func, Instance, Linker, LinkerInstance, Resource, ResourceAny, ResourceType, Type, Val,
};
use wasmtime::{AsContextMut, Store};

use crate::host::{
    AnyBox, CowAny, EMethod, GuestResource, HostRep, HostResState, HostResource, HostTable,
    RawTable, ResourceEntry, ValueEntry, block_on, erase_resource, erase_value, trap_convert,
};
use crate::model::{Direction, Plan, ProjKind, Projected, projected_trait_members};
use crate::names::{NameMap, to_kebab};
use crate::{ConstantMode, WitGenError, WitGenerator};

/// Binds a haphe registry into a wasmtime component [`Linker`] as host
/// (provided-direction) definitions, mirroring the WIT document the same
/// [`WitGenerator`] configuration produces.
///
/// The foreign direction needs none of this machinery: to call a live
/// instance's foreign exports, use the free [`foreign_handle`] /
/// [`foreign_caller`] functions (and their `_in`/`_async` variants).
pub struct WasmBinder<T = ()> {
    config: WitGenerator,
    regs: Registrations,
    /// Live resource values, shared by every closure this binder defines.
    table: Arc<HostTable>,
    _marker: PhantomData<fn(T)>,
}

/// Per-binder dispatch tables collected by the `register_*` calls.
#[derive(Default)]
struct Registrations {
    /// Resource entries keyed by TypeId string (concrete) or
    /// `"{id}|{mangled-args}"` (generic instances).
    resources: HashMap<String, Arc<ResourceEntry>>,
    /// Value entries (records) keyed by TypeId string.
    values: HashMap<String, Arc<ValueEntry>>,
    /// Free functions keyed by descriptor name + instantiation type args.
    fns: Vec<(
        (&'static str, &'static [TypeDescriptor<'static>]),
        ProvidedFn,
    )>,
    /// Async free functions, same keying; dispatched through `block_on`.
    async_fns: Vec<(
        (&'static str, &'static [TypeDescriptor<'static>]),
        AsyncProvidedFn,
    )>,
}

type ProvidedFn = fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;
type AsyncProvidedFn = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;

/// Collects `ScriptBindFn` registrations.
#[derive(Default)]
struct FnCollector {
    entries: Vec<(
        (&'static str, &'static [TypeDescriptor<'static>]),
        ProvidedFn,
    )>,
    async_entries: Vec<(
        (&'static str, &'static [TypeDescriptor<'static>]),
        AsyncProvidedFn,
    )>,
}

// `function_dyn`/`function_dyn_async` are deliberately NOT overridden: their
// core defaults delegate to `function`/`function_async`, so a dyn fn that
// somehow reached this collector would register its monomorphs statically.
// That path relies on check-before-bind — `WasmBinder::bind` runs the
// capability check (`dyn_generics: false`) through the facade, which rejects
// dyn functions with `DynGenericsUnsupported` before any collection happens.
impl FnBinder for FnCollector {
    type Error = std::convert::Infallible;

    fn function(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: ProvidedFn,
    ) -> Result<(), Self::Error> {
        self.entries.push(((name, type_args), f));
        Ok(())
    }

    fn function_async(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: AsyncProvidedFn,
    ) -> Result<(), Self::Error> {
        self.async_entries.push(((name, type_args), f));
        Ok(())
    }
}

fn collect_raw<U: ScriptBind + 'static>() -> RawTable<U> {
    let mut raw = RawTable::default();
    match <U as ScriptBind>::bind(&mut raw) {
        Ok(()) => raw,
        Err(never) => match never {},
    }
}

/// Registration key for a generic instance: erased id plus plan-independent
/// mangled args. Type arguments referencing registered types are not
/// supported at runtime registration (same limitation as the plain foreign
/// mangling).
fn instance_key(id: &str, args: &[TypeDescriptor<'_>]) -> Result<String, WitGenError> {
    let mut key = id.to_string();
    for a in args {
        key.push('|');
        key.push_str(&crate::model::mangle_plain_type(a)?);
    }
    Ok(key)
}

impl<T> WasmBinder<T> {
    /// Creates a binder from the same configuration used to generate the
    /// `.wit` document, so linker names always match the generated file.
    pub fn new(config: WitGenerator) -> Self {
        Self {
            config,
            regs: Registrations::default(),
            table: Arc::new(HostTable::new()),
            _marker: PhantomData,
        }
    }

    /// Registers a resource type for live dispatch: collects the derive's
    /// bridge fn pointers so constructors, field accessors, methods, and
    /// trait projections execute against real values (owned by the binder's
    /// host table). Unregistered types stay bound as descriptive trap stubs.
    ///
    /// `U: Send + Sync` because values live behind `Send + Sync` linker
    /// closures and cross as `UserData` when used as arguments; `Clone` is
    /// the binder's value-acquisition policy (like other backends).
    pub fn register_type<U>(&mut self) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + ScriptStruct + Clone + Send + Sync + 'static,
    {
        let id = <U as ScriptStruct>::DESCRIPTOR.id.as_str().to_string();
        let entry = erase_resource(<U as ScriptStruct>::DESCRIPTOR.name, collect_raw::<U>());
        self.insert_resource(id, entry)?;
        Ok(self)
    }

    /// Registers one concrete instantiation of a generic resource type
    /// (`generics` feature naming): `U` is the concrete Rust type
    /// (e.g. `Holder<f64>`), `type_args` its declared type arguments in
    /// order.
    pub fn register_type_instance<U>(
        &mut self,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + ScriptStruct + Clone + Send + Sync + 'static,
    {
        let id = instance_key(<U as ScriptStruct>::DESCRIPTOR.id.as_str(), type_args)?;
        let entry = erase_resource(<U as ScriptStruct>::DESCRIPTOR.name, collect_raw::<U>());
        self.insert_resource(id, entry)?;
        Ok(self)
    }

    fn insert_resource(&mut self, key: String, entry: ResourceEntry) -> Result<(), WasmBindError> {
        if self.regs.resources.contains_key(&key) {
            return Err(WasmBindError::DuplicateRegistration { name: key });
        }
        self.regs.resources.insert(key, Arc::new(entry));
        Ok(())
    }

    /// Registers a record (plain data struct) for live dispatch of its trait
    /// projections (`{type}-{op}` interface functions). The value
    /// round-trips through `FromScript`/`IntoScript`.
    pub fn register_record<U>(&mut self) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + ScriptStruct + FromScript + IntoScript + Clone + Send + 'static,
    {
        let key = <U as ScriptStruct>::DESCRIPTOR.id.as_str().to_string();
        if self.regs.values.contains_key(&key) {
            return Err(WasmBindError::DuplicateRegistration { name: key });
        }
        self.regs
            .values
            .insert(key, Arc::new(erase_value(collect_raw::<U>())));
        Ok(self)
    }

    /// Registers a unit enum whose methods should dispatch live: companion
    /// functions (`{enum}-{method}`) rebuild the receiver from the lowered
    /// enum value via `FromScript` and drive the method with value
    /// semantics. `&mut self` methods keep descriptive traps (a companion
    /// call has no way to hand the mutated value back).
    pub fn register_enum<U>(&mut self) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + haphe::ScriptEnum + FromScript + IntoScript + Clone + Send + 'static,
    {
        let key = <U as haphe::ScriptEnum>::DESCRIPTOR.id.as_str().to_string();
        if self.regs.values.contains_key(&key) {
            return Err(WasmBindError::DuplicateRegistration { name: key });
        }
        self.regs
            .values
            .insert(key, Arc::new(erase_value(collect_raw::<U>())));
        Ok(self)
    }

    /// Registers one concrete instantiation of a generic record for live
    /// dispatch of its trait projections (`{mangled-instance}-{op}`). `U` is
    /// the concrete Rust type, `type_args` its declared type arguments in
    /// order.
    pub fn register_record_instance<U>(
        &mut self,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + ScriptStruct + FromScript + IntoScript + Clone + Send + 'static,
    {
        let key = instance_key(<U as ScriptStruct>::DESCRIPTOR.id.as_str(), type_args)?;
        if self.regs.values.contains_key(&key) {
            return Err(WasmBindError::DuplicateRegistration { name: key });
        }
        self.regs
            .values
            .insert(key, Arc::new(erase_value(collect_raw::<U>())));
        Ok(self)
    }

    /// Registers one concrete instantiation of a generic unit enum for live
    /// companion dispatch (`{mangled-instance}-{method}`). Same value
    /// semantics as [`register_enum`](Self::register_enum).
    ///
    /// Provided for API symmetry: Rust cannot express a generic enum whose
    /// variants are all unit (the type parameter would be unused), so no
    /// deriving type can satisfy these bounds today — generic enum-instance
    /// companions in practice keep their descriptive stubs.
    pub fn register_enum_instance<U>(
        &mut self,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&mut Self, WasmBindError>
    where
        U: ScriptBind + haphe::ScriptEnum + FromScript + IntoScript + Clone + Send + 'static,
    {
        let key = instance_key(<U as haphe::ScriptEnum>::DESCRIPTOR.id.as_str(), type_args)?;
        if self.regs.values.contains_key(&key) {
            return Err(WasmBindError::DuplicateRegistration { name: key });
        }
        self.regs
            .values
            .insert(key, Arc::new(erase_value(collect_raw::<U>())));
        Ok(self)
    }

    /// Mints a host resource payload for a registered type: inserts `value`
    /// into this binder's host table and returns the `ScriptValue` a foreign
    /// (guest-implemented) call can take as a resource-typed argument. An
    /// `own` position transfers the entry to the guest (its drop reclaims
    /// it); borrows leave it in place — reclaim an untransferred value with
    /// the returned payload simply being dropped alongside the table at
    /// binder teardown.
    pub fn host_resource<U>(&self, value: U) -> Result<ScriptValue, WasmBindError>
    where
        U: ScriptStruct + Send + 'static,
    {
        let id = <U as ScriptStruct>::DESCRIPTOR.id.as_str();
        if !self.regs.resources.contains_key(id) {
            return Err(WasmBindError::UnregisteredType {
                name: id.to_string(),
            });
        }
        let type_name = <U as ScriptStruct>::DESCRIPTOR.name;
        let rep = self.table.insert(type_name, Box::new(value));
        Ok(ScriptValue::UserData(haphe::OpaqueUserData::new(
            HostResource {
                state: Mutex::new(HostResState::Unminted(rep)),
            },
        )))
    }

    /// Registers a free function (the hidden `#[script]` type) for live
    /// dispatch; generic functions register every declared instantiation's
    /// monomorphized wrapper.
    pub fn register_fn<F: ScriptBindFn>(&mut self) -> Result<&mut Self, WasmBindError> {
        let mut collector = FnCollector::default();
        match F::bind(&mut collector) {
            Ok(()) => {}
            Err(never) => match never {},
        }
        for (key, f) in collector.entries {
            if self.regs.fns.iter().any(|(k, _)| *k == key) {
                return Err(WasmBindError::DuplicateRegistration {
                    name: key.0.to_string(),
                });
            }
            self.regs.fns.push((key, f));
        }
        for (key, f) in collector.async_entries {
            if self.regs.async_fns.iter().any(|(k, _)| *k == key) {
                return Err(WasmBindError::DuplicateRegistration {
                    name: key.0.to_string(),
                });
            }
            self.regs.async_fns.push((key, f));
        }
        Ok(self)
    }
}

/// Errors produced while binding into a wasmtime linker.
#[derive(Debug)]
pub enum WasmBindError {
    /// The shared planning/naming pass failed (same errors as generation).
    Gen(WitGenError),
    /// A constant's IR value string could not be parsed as its declared type.
    UnsupportedConstant {
        /// The constant's name.
        name: String,
        /// Why the value could not be converted.
        detail: String,
    },
    /// wasmtime rejected a linker definition.
    Wasm(wasmtime::Error),
    /// The guest does not export the foreign interface's instance.
    MissingForeignInstance {
        /// The expected instance export name (`package/interface`).
        interface: String,
    },
    /// The guest's foreign-interface instance is missing a function.
    MissingForeignExport {
        /// The instance export name.
        interface: String,
        /// The missing (or non-function) export.
        function: String,
    },
    /// The same type, instantiation, or function was registered twice.
    DuplicateRegistration {
        /// The registration key (type id or function name).
        name: String,
    },
    /// A host resource payload was requested for a type never registered
    /// with this binder.
    UnregisteredType {
        /// The type id.
        name: String,
    },
}

impl fmt::Display for WasmBindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gen(e) => write!(f, "{e}"),
            Self::UnsupportedConstant { name, detail } => {
                write!(f, "constant `{name}` cannot be bound: {detail}")
            }
            Self::Wasm(e) => write!(f, "wasmtime linker error: {e}"),
            Self::MissingForeignInstance { interface } => {
                write!(f, "guest does not export instance `{interface}`")
            }
            Self::MissingForeignExport {
                interface,
                function,
            } => write!(
                f,
                "guest instance `{interface}` does not export function `{function}`"
            ),
            Self::DuplicateRegistration { name } => {
                write!(f, "`{name}` is already registered with this binder")
            }
            Self::UnregisteredType { name } => {
                write!(
                    f,
                    "`{name}` is not a registered resource type on this binder; \
                     call `register_type` first"
                )
            }
        }
    }
}

impl std::error::Error for WasmBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gen(e) => Some(e),
            _ => None,
        }
    }
}

impl From<WitGenError> for WasmBindError {
    fn from(e: WitGenError) -> Self {
        Self::Gen(e)
    }
}

impl From<wasmtime::Error> for WasmBindError {
    fn from(e: wasmtime::Error) -> Self {
        Self::Wasm(e)
    }
}

impl<T: 'static> RuntimeBinder for WasmBinder<T> {
    type Runtime = Linker<T>;
    type Error = WasmBindError;

    fn language_name(&self) -> &str {
        "wasm"
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Live binding stores values behind `Send + Sync` linker closures:
        // require `Send` declarations up front (text generation alone does
        // not, so only the runtime binder tightens this). `dyn_generics`
        // stays false via the generator's capabilities: guests name
        // monomorphs statically, so bare dyn is unrepresentable here.
        BindingGenerator::capabilities(&self.config)
            .with_required_thread_safety(Some(ThreadSafety::SEND))
    }

    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        linker: &mut Linker<T>,
    ) -> Result<(), WasmBindError> {
        crate::validate_package_name(&self.config.package)?;
        let plan = Plan::build(registry, &self.config.default_interface)?;
        let cx = DispatchCx::new(&self.table, &self.regs, registry);

        for iface in &plan.interfaces {
            // Foreign interfaces are guest exports: nothing to define in the
            // linker (see `foreign_handle` for the calling side).
            if iface.direction == Direction::Foreign {
                continue;
            }
            let empty = iface.type_ids.is_empty()
                && iface.instance_indices.is_empty()
                && iface.functions.is_empty()
                && (iface.constants.is_empty() || self.config.constants == ConstantMode::Skip);
            if empty {
                continue;
            }
            let instance_name = match &self.config.version {
                Some(v) => format!("{}/{}@{v}", self.config.package, iface.name),
                None => format!("{}/{}", self.config.package, iface.name),
            };
            let mut inst = linker.instance(&instance_name)?;
            let mut member_names = NameMap::new();

            for id in &iface.type_ids {
                match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
                    TypeKind::Struct(s) if plan.is_resource(id) => {
                        let res_name = plan.type_name(id).to_string();
                        match self.regs.resources.get(*id) {
                            Some(entry) => {
                                bind_resource_live(&mut inst, s, &res_name, entry.clone(), &cx)?
                            }
                            None => bind_resource_stub(&mut inst, s, &res_name, &self.table)?,
                        }
                    }
                    // Records, enums, variants, and aliases are type-only in
                    // WIT: nothing to define in a linker.
                    _ => {}
                }
            }

            // Monomorphized generic instantiations, same mangled names the
            // generator emits.
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Struct(s)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                    && plan.is_resource(planned.erased_id)
                {
                    let entry = instance_key(planned.erased_id, planned.args)
                        .ok()
                        .and_then(|k| self.regs.resources.get(&k).cloned());
                    match entry {
                        Some(entry) => {
                            bind_resource_live(&mut inst, s, &planned.wit_name, entry, &cx)?
                        }
                        None => bind_resource_stub(&mut inst, s, &planned.wit_name, &self.table)?,
                    }
                }
            }

            // Companion functions for enum methods, same names the generator
            // emits: live when the enum was registered (`register_enum`) and
            // the method has a value-dispatch channel, descriptive stubs
            // otherwise.
            for id in &iface.type_ids {
                if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(id)) {
                    let value = self.regs.values.get(*id).cloned();
                    for m in e.methods {
                        let name = member_names.insert(&format!("{}_{}", e.name, m.name))?;
                        match value.as_ref().filter(|v| v.methods.contains_key(m.name)) {
                            Some(v) => define_enum_companion(
                                &mut inst,
                                &name,
                                v.clone(),
                                m.name.to_string(),
                                &cx,
                            )?,
                            None => stub_func_msg(
                                &mut inst,
                                &name,
                                "enum companion method not registered; call `register_enum`",
                            )?,
                        }
                    }
                }
            }
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Enum(e)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                {
                    let value = instance_key(planned.erased_id, planned.args)
                        .ok()
                        .and_then(|k| self.regs.values.get(&k).cloned());
                    for m in e.methods {
                        let name =
                            member_names.insert(&format!("{}-{}", planned.wit_name, m.name))?;
                        match value.as_ref().filter(|v| v.methods.contains_key(m.name)) {
                            Some(v) => define_enum_companion(
                                &mut inst,
                                &name,
                                v.clone(),
                                m.name.to_string(),
                                &cx,
                            )?,
                            None => stub_func_msg(
                                &mut inst,
                                &name,
                                "enum companion method not registered; call \
                                 `register_enum_instance`",
                            )?,
                        }
                    }
                }
            }

            // Trait projections of records: live when the record was
            // registered (`register_record`), descriptive stubs otherwise.
            for id in &iface.type_ids {
                if let Some(TypeKind::Struct(s)) = registry.get_type(&haphe::TypeId::new(id))
                    && !plan.is_resource(id)
                {
                    let value = self.regs.values.get(*id).cloned();
                    for proj in projected_trait_members(s)? {
                        let raw = format!("{}_{}", s.name, proj.source_name);
                        let name =
                            member_names.insert_as(&raw, &format!("trait projection `{raw}`"))?;
                        match value
                            .as_ref()
                            .filter(|v| v.projections.contains_key(proj.source_name))
                        {
                            Some(v) => define_record_projection(
                                &mut inst,
                                &name,
                                v.clone(),
                                proj.source_name,
                                &cx,
                            )?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    }
                }
            }
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Struct(s)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                    && !plan.is_resource(planned.erased_id)
                {
                    let value = instance_key(planned.erased_id, planned.args)
                        .ok()
                        .and_then(|k| self.regs.values.get(&k).cloned());
                    for proj in projected_trait_members(s)? {
                        let raw = format!("{}-{}", planned.wit_name, proj.source_name);
                        let name =
                            member_names.insert_as(&raw, &format!("trait projection `{raw}`"))?;
                        match value
                            .as_ref()
                            .filter(|v| v.projections.contains_key(proj.source_name))
                        {
                            Some(v) => define_record_projection(
                                &mut inst,
                                &name,
                                v.clone(),
                                proj.source_name,
                                &cx,
                            )?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    }
                }
            }

            for func in iface.functions {
                if func.generic_params.is_empty() {
                    let name = member_names.insert(func.name)?;
                    if func.is_async {
                        match self.find_async_fn(func.name, &[]) {
                            Some(f) => define_value_fn_async(&mut inst, &name, f, &cx)?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    } else {
                        match self.find_fn(func.name, &[]) {
                            Some(f) => define_value_fn(&mut inst, &name, f, &cx)?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    }
                    continue;
                }
                // One definition per declared instantiation — fn-site and
                // registry-level sources unioned, same mangled names the
                // generator emits (`generics` feature; rejected in
                // `Plan::build` otherwise).
                for args in haphe::union_instantiations(func, iface.fn_instantiations) {
                    let mangled = plan.mangle_fn_instance(func.name, args, None)?;
                    let name = member_names.insert(&mangled)?;
                    if func.is_async {
                        match self.find_async_fn(func.name, args) {
                            Some(f) => define_value_fn_async(&mut inst, &name, f, &cx)?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    } else {
                        match self.find_fn(func.name, args) {
                            Some(f) => define_value_fn(&mut inst, &name, f, &cx)?,
                            None => stub_func(&mut inst, &name)?,
                        }
                    }
                }
            }

            if self.config.constants == ConstantMode::Getter {
                for c in iface.constants {
                    let name = member_names.insert(c.name)?;
                    let val = constant_to_val(c)?;
                    inst.func_new(&name, move |_store, _ty, _params, results| {
                        results[0] = val.clone();
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
}

impl<T> WasmBinder<T> {
    fn find_fn(&self, name: &str, args: &[TypeDescriptor<'_>]) -> Option<ProvidedFn> {
        self.regs
            .fns
            .iter()
            .find(|((n, a), _)| *n == name && *a == args)
            .map(|(_, f)| *f)
    }

    fn find_async_fn(&self, name: &str, args: &[TypeDescriptor<'_>]) -> Option<AsyncProvidedFn> {
        self.regs
            .async_fns
            .iter()
            .find(|((n, a), _)| *n == name && *a == args)
            .map(|(_, f)| *f)
    }
}

/// Splits a `namespace:name` or `namespace:name@version` address into its
/// package name and optional version, validating the name part.
fn split_address(package: &str) -> Result<(&str, Option<&str>), WasmBindError> {
    let (name, version) = match package.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (package, None),
    };
    crate::validate_package_name(name)?;
    if let Some(v) = version
        && (v.is_empty() || v.chars().any(|c| c.is_whitespace()))
    {
        return Err(WitGenError::InvalidPackageName(package.to_string()).into());
    }
    Ok((name, version))
}

/// Builds a [`ForeignCaller`] backed by the guest instance's export of
/// `descriptor`'s interface, resolving every function up front.
///
/// Connecting needs no binder or registration machinery — just a live
/// [`Store`] + instantiated component and the `package` address
/// (`"namespace:name"`, optionally versioned: `"namespace:name@1.2.3"`).
/// The instance export name mirrors the generated document:
/// `package/interface-name` (`@version` when the address carries one), with
/// function names in kebab-case. For a generic interface (`generics`
/// feature), `type_args` selects the monomorphized instance export; type
/// arguments referencing registered types need the registry-aware
/// [`foreign_caller_in`].
pub fn foreign_caller<T: 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &'static [TypeDescriptor<'static>],
) -> Result<Box<dyn ForeignCaller>, WasmBindError> {
    let inner = resolve_foreign(
        package,
        store,
        instance,
        descriptor,
        type_args,
        None,
        EnumNames::new(),
    )?;
    Ok(Box::new(WasmForeignCaller { inner }))
}

/// Like [`foreign_caller`], but resolves monomorphized names through the
/// registry's plan, so type arguments referencing registered types (records,
/// resources) work — and enum return values can be translated back to their
/// declared case names.
pub fn foreign_caller_in<T: 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &'static [TypeDescriptor<'static>],
    registry: &ValidatedRegistry<'_>,
) -> Result<Box<dyn ForeignCaller>, WasmBindError> {
    // The default-interface argument only names interface 0 in the plan and
    // never affects name mangling, so the generator's default suffices.
    let plan = Plan::build(registry, "types")?;
    let inner = resolve_foreign(
        package,
        store,
        instance,
        descriptor,
        type_args,
        Some(&plan),
        declared_enum_cases(registry),
    )?;
    Ok(Box::new(WasmForeignCaller { inner }))
}

/// Builds the foreign-trait handle `H`, dispatching into the guest
/// instance's export of the corresponding interface at the `package` address
/// (the monomorphized instance matching `H`'s type arguments, for a generic
/// interface). See [`foreign_caller`].
pub fn foreign_handle<H: ScriptForeign + ForeignHandle, T: 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
) -> Result<H, WasmBindError> {
    Ok(H::from_caller(foreign_caller(
        package,
        store,
        instance,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
    )?))
}

/// Like [`foreign_handle`], with registry-aware name resolution (see
/// [`foreign_caller_in`]).
pub fn foreign_handle_in<H: ScriptForeign + ForeignHandle, T: 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    registry: &ValidatedRegistry<'_>,
) -> Result<H, WasmBindError> {
    Ok(H::from_caller(foreign_caller_in(
        package,
        store,
        instance,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
        registry,
    )?))
}

/// Asynchronous variant of [`foreign_caller`]: dispatch goes through
/// `Func::call_async` (instances on such stores must be created with
/// `Linker::instantiate_async`). The returned caller's synchronous `call`
/// refuses with a descriptive error — use the handle's async methods.
pub fn foreign_caller_async<T: Send + 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &'static [TypeDescriptor<'static>],
) -> Result<Box<dyn ForeignCaller>, WasmBindError> {
    let inner = resolve_foreign(
        package,
        store,
        instance,
        descriptor,
        type_args,
        None,
        EnumNames::new(),
    )?;
    Ok(Box::new(AsyncWasmForeignCaller { inner }))
}

/// Builds the foreign-trait handle `H` with asynchronous dispatch (see
/// [`foreign_caller_async`]).
pub fn foreign_handle_async<H: ScriptForeign + ForeignHandle, T: Send + 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
) -> Result<H, WasmBindError> {
    Ok(H::from_caller(foreign_caller_async(
        package,
        store,
        instance,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
    )?))
}

/// Asynchronous variant of [`foreign_caller_in`]: registry-aware name
/// resolution (mangled generic-instance interfaces, declared enum-case
/// translation) with dispatch through `Func::call_async`. The returned
/// caller's synchronous `call` refuses with a descriptive error.
pub fn foreign_caller_async_in<T: Send + 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &'static [TypeDescriptor<'static>],
    registry: &ValidatedRegistry<'_>,
) -> Result<Box<dyn ForeignCaller>, WasmBindError> {
    // Same default-interface note as `foreign_caller_in`.
    let plan = Plan::build(registry, "types")?;
    let inner = resolve_foreign(
        package,
        store,
        instance,
        descriptor,
        type_args,
        Some(&plan),
        declared_enum_cases(registry),
    )?;
    Ok(Box::new(AsyncWasmForeignCaller { inner }))
}

/// Builds the foreign-trait handle `H` with asynchronous dispatch and
/// registry-aware name resolution (see [`foreign_caller_async_in`]).
pub fn foreign_handle_async_in<H: ScriptForeign + ForeignHandle, T: Send + 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    registry: &ValidatedRegistry<'_>,
) -> Result<H, WasmBindError> {
    Ok(H::from_caller(foreign_caller_async_in(
        package,
        store,
        instance,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
        registry,
    )?))
}

/// Resolves the interface's instance export and every function into a
/// dispatch table keyed by (function name, type arguments).
fn resolve_foreign<T: 'static>(
    package: &str,
    store: Arc<Mutex<Store<T>>>,
    instance: &Instance,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &'static [TypeDescriptor<'static>],
    plan: Option<&Plan<'_>>,
    enum_cases: EnumNames,
) -> Result<CallerInner<T>, WasmBindError> {
    let (package, version) = split_address(package)?;
    let iface_name = if descriptor.generic_params.is_empty() {
        to_kebab(descriptor.name)
    } else if !cfg!(feature = "generics") || type_args.is_empty() {
        return Err(WitGenError::GenericForeignInterface {
            name: descriptor.name.to_string(),
        }
        .into());
    } else {
        match plan {
            Some(p) => p.mangle_instance(descriptor.id.as_str(), type_args, None)?,
            None => mangle_generic_name(descriptor.name, type_args)?,
        }
    };
    let instance_name = match version {
        Some(v) => format!("{package}/{iface_name}@{v}"),
        None => format!("{package}/{iface_name}"),
    };

    // Compute every export name up front and collision-check them the
    // same way the generator does: two descriptor functions must never
    // resolve to one guest export (e.g. a hand-written `echo_s64` next
    // to a monomorphized `echo<i64>`). Runs before any guest lookup.
    let mut export_names = NameMap::new();
    let mut planned: Vec<(ForeignKey, String)> = Vec::new();
    for f in descriptor.functions {
        if f.generic_params.is_empty() {
            planned.push(((f.name, &[] as _), export_names.insert(f.name)?));
            continue;
        }
        if !cfg!(feature = "generics") {
            return Err(WitGenError::GenericFunction {
                interface: iface_name.clone(),
                function: f.name.to_string(),
            }
            .into());
        }
        for args in f.instantiations {
            let mangled = match plan {
                Some(p) => p.mangle_fn_instance(f.name, args, None)?,
                None => format!("{}-{}", to_kebab(f.name), mangle_args(args)?),
            };
            planned.push(((f.name, args), export_names.insert(&mangled)?));
        }
    }

    let mut funcs: Vec<(ForeignKey, ForeignFn)> = Vec::new();
    {
        let mut store = store.lock().expect("store mutex poisoned");
        let iface_idx = instance
            .get_export_index(&mut *store, None, &instance_name)
            .ok_or_else(|| WasmBindError::MissingForeignInstance {
                interface: instance_name.clone(),
            })?;
        for (key, export_name) in planned {
            let missing = || WasmBindError::MissingForeignExport {
                interface: instance_name.clone(),
                function: export_name.clone(),
            };
            let (item, idx) = instance
                .get_export(&mut *store, Some(&iface_idx), &export_name)
                .ok_or_else(missing)?;
            let ComponentItem::ComponentFunc(fty) = item else {
                return Err(missing());
            };
            let func = instance.get_func(&mut *store, idx).ok_or_else(missing)?;
            funcs.push((
                key,
                ForeignFn {
                    func,
                    params: fty.params().map(|(_, t)| t).collect(),
                    result: fty.results().next(),
                },
            ));
        }
    }
    Ok(CallerInner {
        store,
        funcs,
        enum_cases,
        drops: Arc::new(Mutex::new(Vec::new())),
    })
}

/// Joins plan-independent mangled fragments for a type-argument list.
fn mangle_args(args: &[TypeDescriptor<'_>]) -> Result<String, WitGenError> {
    let fragments: Vec<String> = args
        .iter()
        .map(crate::model::mangle_plain_type)
        .collect::<Result<_, _>>()?;
    Ok(fragments.join("-"))
}

/// Mangled interface name for a generic foreign instantiation, matching
/// [`Plan::mangle_instance`](crate::model::Plan).
fn mangle_generic_name(
    name: &str,
    type_args: &[TypeDescriptor<'_>],
) -> Result<String, WitGenError> {
    Ok(format!("{}-{}", to_kebab(name), mangle_args(type_args)?))
}

/// A pre-resolved guest export function with its reflected signature.
struct ForeignFn {
    func: Func,
    params: Vec<Type>,
    result: Option<Type>,
}

/// Dispatch key: descriptor function name plus the instantiation's type
/// arguments (empty for non-generic functions). Matching by descriptor
/// equality lets registered-type arguments dispatch without re-mangling.
type ForeignKey = (&'static str, &'static [TypeDescriptor<'static>]);

/// A declared enum case: exposed name plus the numeric discriminant of
/// numeric-repr enums (real bit values for flags).
#[derive(Clone, PartialEq)]
struct EnumCase {
    declared: String,
    discriminant: Option<i64>,
}

/// Guest (kebab) enum case name -> declared case; `None` marks a kebab
/// spelling shared by differently-declared variants (ambiguous).
type EnumNames = HashMap<String, Option<EnumCase>>;

/// Builds the guest-to-declared enum case translation from a registry's
/// enum descriptors.
fn declared_enum_cases(registry: &ValidatedRegistry<'_>) -> EnumNames {
    let mut map = EnumNames::new();
    for e in registry.enums() {
        for v in e.variants {
            let case = EnumCase {
                declared: v.name.to_string(),
                discriminant: v.discriminant,
            };
            map.entry(to_kebab(v.name))
                .and_modify(|existing| {
                    if existing.as_ref() != Some(&case) {
                        *existing = None;
                    }
                })
                .or_insert(Some(case));
        }
    }
    map
}

/// Shared state of the wasm-backed callers: the store, the pre-resolved
/// dispatch table, and the enum case translation (empty for callers built
/// without a registry).
struct CallerInner<T: 'static> {
    store: Arc<Mutex<Store<T>>>,
    funcs: Vec<(ForeignKey, ForeignFn)>,
    enum_cases: EnumNames,
    /// Guest resource handles whose host wrappers were dropped: drained
    /// (each `resource_drop`) at the start of every dispatch, since `Drop`
    /// has no store access. Handles queued after the last dispatch are
    /// reclaimed at store teardown.
    drops: Arc<Mutex<Vec<ResourceAny>>>,
}

/// A wasmtime error carried through [`ForeignErrorKind::Call`]
/// (`wasmtime::Error` itself does not implement `std::error::Error`).
#[derive(Debug)]
struct WasmCallError(String);

impl fmt::Display for WasmCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WasmCallError {}

fn call_error(function: &'static str, message: String) -> ForeignError {
    ForeignError {
        function,
        kind: ForeignErrorKind::Call(Box::new(WasmCallError(message))),
    }
}

fn convert_error(function: &'static str, e: ScriptConvertError) -> ForeignError {
    ForeignError {
        function,
        kind: ForeignErrorKind::Convert(e),
    }
}

impl<T: 'static> CallerInner<T> {
    fn find(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&ForeignFn, ForeignError> {
        if !type_args.is_empty() && !cfg!(feature = "generics") {
            return Err(call_error(
                function,
                "generic dispatch requires the `generics` feature of haphe-wit".to_string(),
            ));
        }
        self.funcs
            .iter()
            .find(|((name, args), _)| *name == function && *args == type_args)
            .map(|(_, f)| f)
            .ok_or_else(|| {
                call_error(
                    function,
                    "no declared instantiation matches these type arguments".to_string(),
                )
            })
    }

    /// Reclaims guest handles whose host wrappers were dropped since the
    /// last dispatch. Errors surface as `ForeignError`, never swallowed.
    fn drain_drops(
        &self,
        store: &mut Store<T>,
        function: &'static str,
    ) -> Result<(), ForeignError> {
        let pending: Vec<ResourceAny> =
            std::mem::take(&mut *self.drops.lock().expect("drop queue poisoned"));
        for any in pending {
            any.resource_drop(&mut *store)
                .map_err(|e| call_error(function, format!("dropping a guest resource: {e}")))?;
        }
        Ok(())
    }

    /// Lowers a resource-typed argument: `GuestResource` wrappers pass their
    /// handle (an `own` position TAKES it — reuse errors), `HostResource`
    /// wrappers re-enter as handles over the host table rep.
    fn lower_resource(
        _store: &mut Store<T>,
        v: &ScriptValue,
        borrow: bool,
    ) -> Result<Val, ScriptConvertError> {
        let err = |got: &'static str| ScriptConvertError {
            expected: "a resource handle payload",
            got,
        };
        let ScriptValue::UserData(ud) = v else {
            return Err(err(v.variant_name()));
        };
        if let Some(g) = ud.downcast_ref::<GuestResource>() {
            let mut handle = g.handle.lock().expect("guest handle poisoned");
            return if borrow {
                let any = handle.ok_or(err("an already-transferred guest handle"))?;
                Ok(Val::Resource(any))
            } else {
                let any = handle
                    .take()
                    .ok_or(err("an already-transferred guest handle"))?;
                Ok(Val::Resource(any))
            };
        }
        if let Some(h) = ud.downcast_ref::<HostResource>() {
            let mut state = h.state.lock().expect("host resource state poisoned");
            // Lazily mint ONE `own` handle; wasmtime borrows it in-call for
            // borrow positions, so the payload stays reusable until an
            // `own` position transfers it.
            if let HostResState::Unminted(rep) = *state {
                let any = Resource::<HostRep>::new_own(rep)
                    .try_into_resource_any(&mut *_store)
                    .map_err(|_| err("a mintable host resource handle"))?;
                *state = HostResState::Minted(any);
            }
            return match (&*state, borrow) {
                (HostResState::Minted(any), true) => Ok(Val::Resource(*any)),
                (HostResState::Minted(any), false) => {
                    let any = *any;
                    // Ownership moves to the guest; its drop runs the
                    // registered destructor, reclaiming the table entry.
                    *state = HostResState::Transferred;
                    Ok(Val::Resource(any))
                }
                (HostResState::Transferred, _) => Err(err("an already-transferred host resource")),
                (HostResState::Unminted(_), _) => unreachable!("minted above"),
            };
        }
        Err(err("an unrecognized userdata payload"))
    }

    fn lower_args(
        &self,
        store: &mut Store<T>,
        f: &ForeignFn,
        function: &'static str,
        args: &[ScriptValue],
    ) -> Result<Vec<Val>, ForeignError> {
        if args.len() != f.params.len() {
            return Err(call_error(
                function,
                format!(
                    "guest expects {} argument(s), got {}",
                    f.params.len(),
                    args.len()
                ),
            ));
        }
        let mut res = |v: &ScriptValue, borrow: bool| Self::lower_resource(&mut *store, v, borrow);
        args.iter()
            .zip(&f.params)
            .map(|(arg, ty)| script_to_val(arg, ty, &self.enum_cases, &mut res))
            .collect::<Result<_, _>>()
            .map_err(|e| convert_error(function, e))
    }

    fn lift(&self, v: &Val) -> Result<ScriptValue, ScriptConvertError> {
        let drops = Arc::downgrade(&self.drops);
        let mut res = |any: ResourceAny| {
            Ok(ScriptValue::UserData(haphe::OpaqueUserData::new(
                GuestResource {
                    handle: Mutex::new(Some(any)),
                    drops: drops.clone(),
                },
            )))
        };
        val_to_script(v, &self.enum_cases, &mut res)
    }

    fn lift_result(
        &self,
        f: &ForeignFn,
        function: &'static str,
        results: Vec<Val>,
    ) -> Result<ScriptValue, ForeignError> {
        match results.into_iter().next() {
            None => Ok(ScriptValue::Unit),
            // An `error_kind` return is wrapped in `result<T>` by the
            // generator: unwrap the payload, mapping the guest's error case
            // into the host error channel.
            Some(Val::Result(r)) if matches!(f.result, Some(Type::Result(_))) => match r {
                Ok(Some(v)) => self.lift(&v).map_err(|e| convert_error(function, e)),
                Ok(None) => Ok(ScriptValue::Unit),
                Err(payload) => {
                    let detail = match payload {
                        Some(v) => match self.lift(&v) {
                            Ok(ScriptValue::String(s)) => s,
                            Ok(other) => format!("{other:?}"),
                            Err(_) => "guest returned an error".to_string(),
                        },
                        None => "guest returned an error".to_string(),
                    };
                    Err(call_error(function, detail))
                }
            },
            Some(v) => self.lift(&v).map_err(|e| convert_error(function, e)),
        }
    }

    fn dispatch_sync(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        let f = self.find(function, type_args)?;
        let mut store = self.store.lock().expect("store mutex poisoned");
        self.drain_drops(&mut store, function)?;
        let vals = self.lower_args(&mut store, f, function, args)?;
        let mut results = vec![Val::Bool(false); usize::from(f.result.is_some())];
        f.func
            .call(&mut *store, &vals, &mut results)
            .map_err(|e| call_error(function, format!("{e:?}")))?;
        drop(store);
        self.lift_result(f, function, results)
    }
}

impl<T: Send + 'static> CallerInner<T> {
    // The guard is held across the await deliberately: the guest call needs
    // exclusive store access for its whole duration, and these futures are
    // not `Send`, so no executor can move them across threads mid-lock.
    #[allow(clippy::await_holding_lock)]
    async fn dispatch_async(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        let f = self.find(function, type_args)?;
        let mut store = self.store.lock().expect("store mutex poisoned");
        self.drain_drops(&mut store, function)?;
        let vals = self.lower_args(&mut store, f, function, args)?;
        let mut results = vec![Val::Bool(false); usize::from(f.result.is_some())];
        f.func
            .call_async(&mut *store, &vals, &mut results)
            .await
            .map_err(|e| call_error(function, format!("{e:?}")))?;
        drop(store);
        self.lift_result(f, function, results)
    }
}

/// [`ForeignCaller`] over a guest component instance's exports.
///
/// Dispatch is synchronous ([`Func::call`]); `call_async` delegates to it, so
/// async foreign methods work against synchronously-lifted guests on
/// non-async stores. For asynchronous dispatch use the async caller
/// instead.
struct WasmForeignCaller<T: 'static> {
    inner: CallerInner<T>,
}

impl<T: 'static> ForeignCaller for WasmForeignCaller<T> {
    fn call(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        self.inner.dispatch_sync(function, type_args, args)
    }

    fn call_async<'a>(
        &'a self,
        function: &'static str,
        type_args: &'a [TypeDescriptor<'static>],
        args: &'a [ScriptValue],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ScriptValue, ForeignError>> + 'a>>
    {
        Box::pin(std::future::ready(
            self.inner.dispatch_sync(function, type_args, args),
        ))
    }
}

/// [`ForeignCaller`] with genuinely asynchronous dispatch via
/// `Func::call_async`; the synchronous `call` refuses.
struct AsyncWasmForeignCaller<T: Send + 'static> {
    inner: CallerInner<T>,
}

impl<T: Send + 'static> ForeignCaller for AsyncWasmForeignCaller<T> {
    fn call(
        &self,
        function: &'static str,
        _type_args: &[TypeDescriptor<'static>],
        _args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        Err(call_error(
            function,
            "this caller dispatches asynchronously; call the trait's async methods".to_string(),
        ))
    }

    fn call_async<'a>(
        &'a self,
        function: &'static str,
        type_args: &'a [TypeDescriptor<'static>],
        args: &'a [ScriptValue],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ScriptValue, ForeignError>> + 'a>>
    {
        Box::pin(self.inner.dispatch_async(function, type_args, args))
    }
}

/// Lowers a resource-typed [`ScriptValue`] (a `UserData` payload) into a
/// handle [`Val`]; `borrow` distinguishes `borrow<T>` from `own<T>` positions.
type ResLower<'f> = &'f mut dyn FnMut(&ScriptValue, bool) -> Result<Val, ScriptConvertError>;
/// Lifts a resource handle back into a [`ScriptValue`] `UserData` payload.
type ResLift<'f> = &'f mut dyn FnMut(ResourceAny) -> Result<ScriptValue, ScriptConvertError>;

/// Converts a [`ScriptValue`] argument into the [`Val`] shape the guest's
/// reflected parameter type expects. Resource-typed positions are delegated
/// to `res` (the caller's store-aware handle plumbing).
fn script_to_val(
    v: &ScriptValue,
    ty: &Type,
    enums: &EnumNames,
    res: ResLower<'_>,
) -> Result<Val, ScriptConvertError> {
    let err = |expected: &'static str| ScriptConvertError {
        expected,
        got: v.variant_name(),
    };
    let int = |expected: &'static str| match v {
        ScriptValue::I64(n) => Ok(*n),
        _ => Err(err(expected)),
    };
    Ok(match ty {
        Type::Bool => match v {
            ScriptValue::Bool(b) => Val::Bool(*b),
            _ => return Err(err("bool")),
        },
        Type::S8 => Val::S8(int("s8")?.try_into().map_err(|_| err("s8"))?),
        Type::S16 => Val::S16(int("s16")?.try_into().map_err(|_| err("s16"))?),
        Type::S32 => Val::S32(int("s32")?.try_into().map_err(|_| err("s32"))?),
        Type::S64 => Val::S64(int("s64")?),
        Type::U8 => Val::U8(int("u8")?.try_into().map_err(|_| err("u8"))?),
        Type::U16 => Val::U16(int("u16")?.try_into().map_err(|_| err("u16"))?),
        Type::U32 => Val::U32(int("u32")?.try_into().map_err(|_| err("u32"))?),
        Type::U64 => Val::U64(int("u64")?.try_into().map_err(|_| err("u64"))?),
        Type::Float32 => match v {
            ScriptValue::F64(n) => Val::Float32(*n as f32),
            ScriptValue::I64(n) => Val::Float32(*n as f32),
            _ => return Err(err("f32")),
        },
        Type::Float64 => match v {
            ScriptValue::F64(n) => Val::Float64(*n),
            ScriptValue::I64(n) => Val::Float64(*n as f64),
            _ => return Err(err("f64")),
        },
        Type::Char => match v {
            ScriptValue::Char(c) => Val::Char(*c),
            _ => return Err(err("char")),
        },
        Type::String => match v {
            ScriptValue::String(s) => Val::String(s.clone()),
            _ => return Err(err("string")),
        },
        Type::List(l) => match v {
            ScriptValue::List(items) => Val::List(
                items
                    .iter()
                    .map(|item| script_to_val(item, &l.ty(), enums, &mut *res))
                    .collect::<Result<_, _>>()?,
            ),
            ScriptValue::Bytes(bytes) if matches!(l.ty(), Type::U8) => {
                Val::List(bytes.iter().map(|b| Val::U8(*b)).collect())
            }
            // Core `Map` descriptors render as `list<tuple<k, v>>` in the
            // generated document: accept maps as pair lists.
            ScriptValue::Map(pairs) => {
                let Type::Tuple(t) = l.ty() else {
                    return Err(err("list"));
                };
                let mut kv = t.types();
                let (Some(key_ty), Some(value_ty), None) = (kv.next(), kv.next(), kv.next()) else {
                    return Err(err("list"));
                };
                Val::List(
                    pairs
                        .iter()
                        .map(|(k, value)| {
                            Ok(Val::Tuple(vec![
                                script_to_val(
                                    &ScriptValue::String(k.clone()),
                                    &key_ty,
                                    enums,
                                    &mut *res,
                                )?,
                                script_to_val(value, &value_ty, enums, &mut *res)?,
                            ]))
                        })
                        .collect::<Result<_, ScriptConvertError>>()?,
                )
            }
            _ => return Err(err("list")),
        },
        Type::FixedLengthList(l) => match v {
            ScriptValue::List(items) if items.len() == l.len() as usize => Val::List(
                items
                    .iter()
                    .map(|item| script_to_val(item, &l.ty(), enums, &mut *res))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(err("fixed-length list")),
        },
        Type::Option(o) => match v {
            ScriptValue::Optional(None) | ScriptValue::Unit => Val::Option(None),
            ScriptValue::Optional(Some(inner)) => Val::Option(Some(Box::new(script_to_val(
                inner,
                &o.ty(),
                enums,
                &mut *res,
            )?))),
            other_v => Val::Option(Some(Box::new(script_to_val(
                other_v,
                &o.ty(),
                enums,
                &mut *res,
            )?))),
        },
        Type::Record(r) => match v {
            ScriptValue::Map(pairs) => {
                let mut fields = Vec::with_capacity(r.fields().len());
                for field in r.fields() {
                    let value = pairs
                        .iter()
                        .find(|(k, _)| k == field.name)
                        .map(|(_, value)| value)
                        .ok_or_else(|| err("record with all fields"))?;
                    fields.push((
                        field.name.to_string(),
                        script_to_val(value, &field.ty, enums, &mut *res)?,
                    ));
                }
                if pairs.len() != fields.len() {
                    return Err(err("record without extra fields"));
                }
                Val::Record(fields)
            }
            _ => return Err(err("record")),
        },
        Type::Tuple(t) => match v {
            ScriptValue::List(items) if items.len() == t.types().len() => Val::Tuple(
                items
                    .iter()
                    .zip(t.types())
                    .map(|(item, ty)| script_to_val(item, &ty, enums, &mut *res))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(err("tuple")),
        },
        Type::Flags(fl) => match v {
            // Flags cross as a list of set flag names (mirroring the lifting
            // shape); unknown names error descriptively.
            ScriptValue::List(items) => {
                let mut names = Vec::with_capacity(items.len());
                for item in items {
                    let ScriptValue::String(name) = item else {
                        return Err(err("a list of flag-name strings"));
                    };
                    if !fl.names().any(|n| n == name) {
                        return Err(err("a declared flag name"));
                    }
                    names.push(name.clone());
                }
                Val::Flags(names)
            }
            _ => return Err(err("a list of flag-name strings")),
        },
        Type::Variant(var) => match v {
            // Variants cross as a single-pair map { case: payload }
            // (mirroring the lifting shape); Unit payload means no payload.
            ScriptValue::Map(pairs) if pairs.len() == 1 => {
                let (case, payload) = &pairs[0];
                let Some(case_decl) = var.cases().find(|c| c.name == case) else {
                    return Err(err("a declared variant case"));
                };
                let lowered = match (&case_decl.ty, payload) {
                    (None, _) => None,
                    (Some(ty), p) => Some(Box::new(script_to_val(p, ty, enums, &mut *res)?)),
                };
                Val::Variant(case.clone(), lowered)
            }
            _ => return Err(err("a single-case variant map")),
        },
        Type::Map(m) => match v {
            ScriptValue::Map(pairs) => Val::Map(
                pairs
                    .iter()
                    .map(|(k, value)| {
                        Ok((
                            script_to_val(
                                &ScriptValue::String(k.clone()),
                                &m.key(),
                                enums,
                                &mut *res,
                            )?,
                            script_to_val(value, &m.value(), enums, &mut *res)?,
                        ))
                    })
                    .collect::<Result<_, ScriptConvertError>>()?,
            ),
            _ => return Err(err("map")),
        },
        Type::Enum(e) => {
            // Numeric enums also accept a plain integer, matched against the
            // recorded discriminants of this guest enum's cases.
            if let ScriptValue::I64(n) = v {
                let matched = e.names().find(|name| {
                    matches!(
                        enums.get(*name),
                        Some(Some(case)) if case.discriminant == Some(*n)
                    )
                });
                return match matched {
                    Some(name) => Ok(Val::Enum(name.to_string())),
                    None => Err(err("a declared enum case discriminant")),
                };
            }
            let case = match v {
                ScriptValue::Enum { case, .. } => case.as_str(),
                ScriptValue::String(s) => s.as_str(),
                _ => return Err(err("enum case")),
            };
            // The declared case maps to the guest case deterministically via
            // this crate's kebab conversion; an exact guest spelling is also
            // accepted.
            let kebab = to_kebab(case);
            let name = e
                .names()
                .find(|n| *n == case || *n == kebab)
                .ok_or_else(|| err("a declared enum case"))?;
            Val::Enum(name.to_string())
        }
        Type::Own(_) => res(v, false)?,
        Type::Borrow(_) => res(v, true)?,
        _ => return Err(err("a wasm-representable value")),
    })
}

/// Converts a guest return [`Val`] back into a [`ScriptValue`].
///
/// Enum cases are translated from the guest's spelling back to the declared
/// case name through `enum_cases`; callers built without a registry have an
/// empty map, so enum returns error there — build the caller with the
/// registry-aware constructor instead. Resource handles are delegated to
/// `res` (the caller's store-aware handle plumbing).
fn val_to_script(
    v: &Val,
    enum_cases: &EnumNames,
    res: ResLift<'_>,
) -> Result<ScriptValue, ScriptConvertError> {
    let err = ScriptConvertError {
        expected: "a script-representable value",
        got: "an unsupported wasm value",
    };
    Ok(match v {
        Val::Bool(b) => ScriptValue::Bool(*b),
        Val::S8(n) => ScriptValue::I64((*n).into()),
        Val::S16(n) => ScriptValue::I64((*n).into()),
        Val::S32(n) => ScriptValue::I64((*n).into()),
        Val::S64(n) => ScriptValue::I64(*n),
        Val::U8(n) => ScriptValue::I64((*n).into()),
        Val::U16(n) => ScriptValue::I64((*n).into()),
        Val::U32(n) => ScriptValue::I64((*n).into()),
        Val::U64(n) => ScriptValue::I64(i64::try_from(*n).map_err(|_| err)?),
        Val::Float32(n) => ScriptValue::F64((*n).into()),
        Val::Float64(n) => ScriptValue::F64(*n),
        Val::Char(c) => ScriptValue::Char(*c),
        Val::String(s) => ScriptValue::String(s.clone()),
        Val::List(items) | Val::Tuple(items) => ScriptValue::List(
            items
                .iter()
                .map(|item| val_to_script(item, enum_cases, &mut *res))
                .collect::<Result<_, _>>()?,
        ),
        Val::Option(opt) => ScriptValue::Optional(match opt {
            Some(inner) => Some(Box::new(val_to_script(inner, enum_cases, &mut *res)?)),
            None => None,
        }),
        Val::Record(fields) => ScriptValue::Map(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), val_to_script(v, enum_cases, &mut *res)?)))
                .collect::<Result<_, ScriptConvertError>>()?,
        ),
        Val::Map(pairs) => ScriptValue::Map(
            pairs
                .iter()
                .map(|(k, v)| match k {
                    Val::String(s) => Ok((s.clone(), val_to_script(v, enum_cases, &mut *res)?)),
                    _ => Err(err.clone()),
                })
                .collect::<Result<_, ScriptConvertError>>()?,
        ),
        Val::Enum(name) => match enum_cases.get(name) {
            Some(Some(case)) => ScriptValue::Enum {
                case: case.declared.clone(),
                discriminant: case.discriminant,
            },
            Some(None) => {
                return Err(ScriptConvertError {
                    expected: "an unambiguous enum case",
                    got: "a case name shared by multiple declared variants",
                });
            }
            None => {
                return Err(ScriptConvertError {
                    expected: "a registry-translated enum case (use the registry-aware caller)",
                    got: "an enum case with no registry translation",
                });
            }
        },
        Val::Flags(names) => ScriptValue::List(
            names
                .iter()
                .map(|n| ScriptValue::String(n.clone()))
                .collect(),
        ),
        Val::Variant(case, payload) => ScriptValue::Map(vec![(
            case.clone(),
            match payload {
                Some(p) => val_to_script(p, enum_cases, &mut *res)?,
                None => ScriptValue::Unit,
            },
        )]),
        Val::Resource(any) => res(*any)?,
        _ => return Err(err),
    })
}

// ---------------------------------------------------------------------------
// Provided-direction live dispatch
// ---------------------------------------------------------------------------

/// Shared context captured by every dispatch closure: the live-value table,
/// the registered resource entries by type name (for handle <-> value
/// conversion), and the enum case translation.
#[derive(Clone)]
struct DispatchCx {
    table: Arc<HostTable>,
    by_type_name: Arc<HashMap<&'static str, Arc<ResourceEntry>>>,
    cases: Arc<EnumNames>,
}

impl DispatchCx {
    fn new(table: &Arc<HostTable>, regs: &Registrations, registry: &ValidatedRegistry<'_>) -> Self {
        let mut by_type_name = HashMap::new();
        for entry in regs.resources.values() {
            by_type_name.insert(entry.type_name, entry.clone());
        }
        Self {
            table: table.clone(),
            by_type_name: Arc::new(by_type_name),
            cases: Arc::new(declared_enum_cases(registry)),
        }
    }

    fn entry_for(&self, type_name: &str) -> Result<&Arc<ResourceEntry>, ScriptConvertError> {
        self.by_type_name.get(type_name).ok_or(ScriptConvertError {
            expected: "a registered resource type",
            got: "an unregistered resource value",
        })
    }

    /// Lifts one guest [`Val`] into a [`ScriptValue`], turning resource
    /// handles into `UserData` values: `own` handles consume the table entry
    /// (ownership transferred to the host), `borrow` handles clone out.
    fn lift<S: AsContextMut>(
        &self,
        store: &mut S,
        v: &Val,
    ) -> Result<ScriptValue, ScriptConvertError> {
        let mut res = |any: ResourceAny| self.lift_resource(store, any);
        val_to_script(v, &self.cases, &mut res)
    }

    fn lift_resource<S: AsContextMut>(
        &self,
        store: &mut S,
        any: ResourceAny,
    ) -> Result<ScriptValue, ScriptConvertError> {
        let err = |got: &'static str| ScriptConvertError {
            expected: "a live host resource handle",
            got,
        };
        let owned = any.owned();
        let resource: Resource<HostRep> = any
            .try_into_resource(&mut *store)
            .map_err(|_| err("a foreign or stale resource handle"))?;
        let rep = resource.rep();
        if owned {
            let entry = self
                .table
                .remove(rep)
                .ok_or(err("an already-consumed resource handle"))?;
            Ok((self.entry_for(entry.type_name)?.to_userdata)(
                &*entry.value,
            ))
        } else {
            let guard = self.table.lock();
            let entry = guard.get(&rep).ok_or(err("a stale resource handle"))?;
            Ok((self.entry_for(entry.type_name)?.to_userdata)(
                &*entry.value,
            ))
        }
    }

    /// Lowers one [`ScriptValue`] into the guest's expected [`Val`] shape,
    /// turning resource-typed `UserData` values into fresh owned handles
    /// (their values enter the table).
    fn lower<S: AsContextMut>(
        &self,
        store: &mut S,
        v: &ScriptValue,
        ty: &Type,
    ) -> Result<Val, ScriptConvertError> {
        let mut res = |v: &ScriptValue, borrow: bool| self.lower_resource(store, v, borrow);
        script_to_val(v, ty, &self.cases, &mut res)
    }

    fn lower_resource<S: AsContextMut>(
        &self,
        store: &mut S,
        v: &ScriptValue,
        borrow: bool,
    ) -> Result<Val, ScriptConvertError> {
        let err = |got: &'static str| ScriptConvertError {
            expected: "a host resource value",
            got,
        };
        let ScriptValue::UserData(_) = v else {
            return Err(err(v.variant_name()));
        };
        let _ = borrow;
        // A concrete registered value becomes a fresh table entry with an
        // owned handle.
        for entry in self.by_type_name.values() {
            if let Some(value) = (entry.from_userdata)(v) {
                let rep = self.table.insert(entry.type_name, value);
                return Resource::<HostRep>::new_own(rep)
                    .try_into_resource_any(&mut *store)
                    .map(Val::Resource)
                    .map_err(|_| err("a host resource that could not enter this store"));
            }
        }
        Err(err("a userdata value of an unregistered type"))
    }

    /// Inserts a freshly produced value and hands back its owned handle.
    fn own_handle<S: AsContextMut>(
        &self,
        store: &mut S,
        type_name: &'static str,
        value: AnyBox,
    ) -> wasmtime::Result<Val> {
        let rep = self.table.insert(type_name, value);
        Ok(Val::Resource(
            Resource::<HostRep>::new_own(rep).try_into_resource_any(&mut *store)?,
        ))
    }
}

/// Resolves the leading `self` handle parameter to its table rep.
fn self_rep<S: AsContextMut>(store: &mut S, params: &[Val], name: &str) -> wasmtime::Result<u32> {
    let Some(Val::Resource(any)) = params.first() else {
        return Err(wasmtime::Error::msg(format!(
            "haphe-wit: `{name}`: expected a resource handle receiver"
        )));
    };
    let resource: Resource<HostRep> = any.try_into_resource(&mut *store)?;
    Ok(resource.rep())
}

/// Lifts the non-receiver arguments.
fn lift_args<S: AsContextMut>(
    cx: &DispatchCx,
    store: &mut S,
    params: &[Val],
    skip: usize,
    name: &str,
) -> wasmtime::Result<Vec<ScriptValue>> {
    params[skip..]
        .iter()
        .map(|v| cx.lift(store, v).map_err(|e| trap_convert(name, e)))
        .collect()
}

/// Lowers an optional single result.
fn lower_result<S: AsContextMut, I: Iterator<Item = Type>>(
    cx: &DispatchCx,
    store: &mut S,
    mut result_types: I,
    out: ScriptValue,
    results: &mut [Val],
    name: &str,
) -> wasmtime::Result<()> {
    if let Some(rty) = result_types.next() {
        results[0] = cx
            .lower(store, &out, &rty)
            .map_err(|e| trap_convert(name, e))?;
    }
    Ok(())
}

fn trap(name: &str, detail: &str) -> wasmtime::Error {
    wasmtime::Error::msg(format!("haphe-wit: `{name}`: {detail}"))
}

/// Defines a live free-function (or companion-shaped) dispatch.
fn define_value_fn<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
    f: ProvidedFn,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let cx = cx.clone();
    let msg_name = name.to_string();
    inst.func_new(name, move |mut store, fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg_name)?;
        let out = f(&args).map_err(|e| trap_convert(&msg_name, e))?;
        lower_result(&cx, &mut store, fty.results(), out, results, &msg_name)
    })?;
    Ok(())
}

/// Defines a live `async` free function: the boxed future borrows the lifted
/// argument slice and is driven to completion on this thread ([`block_on`] —
/// same trade-offs as async methods: the calling fiber blocks, and
/// tokio-reactor futures need a multi-thread runtime).
fn define_value_fn_async<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
    f: AsyncProvidedFn,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let cx = cx.clone();
    let msg_name = name.to_string();
    inst.func_new(name, move |mut store, fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg_name)?;
        let out = block_on(f(&args)).map_err(|e| trap_convert(&msg_name, e))?;
        lower_result(&cx, &mut store, fty.results(), out, results, &msg_name)
    })?;
    Ok(())
}

/// Defines a live record trait projection (`{type}-{op}`): the receiver and
/// arguments round-trip through `ScriptValue`.
fn define_record_projection<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
    entry: Arc<ValueEntry>,
    member: &'static str,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let cx = cx.clone();
    let msg_name = name.to_string();
    inst.func_new(name, move |mut store, fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg_name)?;
        let proj = entry
            .projections
            .get(member)
            .ok_or_else(|| trap(&msg_name, "trait projection has no dispatch channel"))?;
        let out = proj(&args).map_err(|e| trap_convert(&msg_name, e))?;
        lower_result(&cx, &mut store, fty.results(), out, results, &msg_name)
    })?;
    Ok(())
}

/// Dispatches one enum companion function: `args[0]` is the lowered enum
/// value (the receiver), the rest are the method's parameters.
fn define_enum_companion<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
    entry: Arc<ValueEntry>,
    method: String,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let cx = cx.clone();
    let msg_name = name.to_string();
    inst.func_new(name, move |mut store, fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg_name)?;
        let m = entry
            .methods
            .get(method.as_str())
            .ok_or_else(|| trap(&msg_name, "enum method has no dispatch channel"))?;
        let (this, rest) = args
            .split_first()
            .ok_or_else(|| trap(&msg_name, "companion call is missing its receiver"))?;
        let out = m(this.clone(), rest).map_err(|e| trap_convert(&msg_name, e))?;
        lower_result(&cx, &mut store, fty.results(), out, results, &msg_name)
    })?;
    Ok(())
}

/// Registers the full live surface of one resource: constructor(s), field
/// accessors, methods (sync and async), and trait projections, dispatching
/// through the erased entry with values owned by the host table. The
/// destructor removes the entry.
fn bind_resource_live<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    s: &haphe::StructDescriptor<'_>,
    res: &str,
    entry: Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let dtor_table = cx.table.clone();
    inst.resource(res, ResourceType::host::<HostRep>(), move |_, rep| {
        dtor_table.remove(rep);
        Ok(())
    })?;
    let mut members = NameMap::new();

    for field in s.fields {
        let getter = members.insert(field.name)?;
        define_field_get(inst, res, &getter, field.name, &entry, cx)?;
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            define_field_set(inst, res, &setter, field.name, &entry, cx)?;
        }
    }
    for prop in s.properties {
        let getter = members.insert(prop.name)?;
        if entry.props.contains_key(prop.name) {
            define_prop_get(inst, res, &getter, prop.name, &entry, cx)?;
        } else {
            // Whitelist-gated out at the macro layer: declared, described,
            // but not bridgeable.
            stub_func_msg(
                inst,
                &format!("[method]{res}.{getter}"),
                "declared but not bridgeable (its script type lacks value conversions)",
            )?;
        }
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            if entry.props.contains_key(prop.name) {
                define_prop_set(inst, res, &setter, prop.name, &entry, cx)?;
            } else {
                stub_func_msg(
                    inst,
                    &format!("[method]{res}.{setter}"),
                    "declared but not bridgeable (its script type lacks value conversions)",
                )?;
            }
        }
    }

    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        let (linker_name, ctor_key) = if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            (format!("[constructor]{res}"), ctor.name.to_string())
        } else {
            let name = members.insert(ctor.name)?;
            (format!("[static]{res}.{name}"), ctor.name.to_string())
        };
        if ctor.is_async {
            define_ctor_async(inst, &linker_name, ctor_key, &entry, cx)?;
            continue;
        }
        define_ctor(inst, &linker_name, ctor_key, &entry, cx)?;
    }

    for m in s.methods {
        let name = members.insert(m.name)?;
        let prefixed = match m.receiver {
            Some(Receiver::Ref | Receiver::RefMut) => format!("[method]{res}.{name}"),
            Some(Receiver::Owned) | None => format!("[static]{res}.{name}"),
        };
        if m.receiver.is_none() {
            // Receiver-less associated functions have no dispatch channel
            // through `TypeBinder` that carries no value; descriptive trap.
            stub_func_msg(
                inst,
                &prefixed,
                "associated functions without a receiver are not yet bridged",
            )?;
            continue;
        }
        if !entry.methods.contains_key(m.name) {
            // Descriptor-only (e.g. signature types without value
            // conversions): declared, described, but not bridgeable.
            stub_func_msg(
                inst,
                &prefixed,
                "declared but not bridgeable (its signature types lack value conversions)",
            )?;
            continue;
        }
        define_method(
            inst,
            &prefixed,
            m.name.to_string(),
            m.receiver.expect("checked above"),
            &entry,
            cx,
        )?;
    }

    for proj in projected_trait_members(s)? {
        let name = members.insert_as(
            proj.source_name,
            &format!("trait projection `{}`", proj.source_name),
        )?;
        define_projection(inst, res, &name, proj, &entry, cx)?;
    }

    Ok(())
}

fn define_field_get<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    res: &str,
    getter: &str,
    field: &str,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let linker_name = format!("[method]{res}.{getter}");
    let (entry, cx, field) = (entry.clone(), cx.clone(), field.to_string());
    let msg = linker_name.clone();
    inst.func_new(&linker_name, move |mut store, fty, params, results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let out = {
            let guard = cx.table.lock();
            let e = guard
                .get(&rep)
                .ok_or_else(|| trap(&msg, "stale resource handle"))?;
            check_type(&msg, e.type_name, entry.type_name)?;
            let acc = entry
                .fields
                .get(field.as_str())
                .ok_or_else(|| trap(&msg, "field has no bridge accessor"))?;
            (acc.get)(&*e.value).map_err(|e| trap_convert(&msg, e))?
        };
        lower_result(&cx, &mut store, fty.results(), out, results, &msg)
    })?;
    Ok(())
}

fn define_field_set<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    res: &str,
    setter: &str,
    field: &str,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let linker_name = format!("[method]{res}.{setter}");
    let (entry, cx, field) = (entry.clone(), cx.clone(), field.to_string());
    let msg = linker_name.clone();
    inst.func_new(&linker_name, move |mut store, _fty, params, _results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let args = lift_args(&cx, &mut store, params, 1, &msg)?;
        let value = args
            .into_iter()
            .next()
            .ok_or_else(|| trap(&msg, "missing setter value"))?;
        let mut guard = cx.table.lock();
        let e = guard
            .get_mut(&rep)
            .ok_or_else(|| trap(&msg, "stale resource handle"))?;
        check_type(&msg, e.type_name, entry.type_name)?;
        let acc = entry
            .fields
            .get(field.as_str())
            .ok_or_else(|| trap(&msg, "field has no bridge accessor"))?;
        let set = acc
            .set
            .as_ref()
            .ok_or_else(|| trap(&msg, "field is readonly"))?;
        set(&mut *e.value, value).map_err(|e| trap_convert(&msg, e))?;
        Ok(())
    })?;
    Ok(())
}

fn define_prop_get<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    res: &str,
    getter: &str,
    prop: &str,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let linker_name = format!("[method]{res}.{getter}");
    let (entry, cx, prop) = (entry.clone(), cx.clone(), prop.to_string());
    let msg = linker_name.clone();
    inst.func_new(&linker_name, move |mut store, fty, params, results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let out = {
            let guard = cx.table.lock();
            let e = guard
                .get(&rep)
                .ok_or_else(|| trap(&msg, "stale resource handle"))?;
            check_type(&msg, e.type_name, entry.type_name)?;
            let acc = entry
                .props
                .get(prop.as_str())
                .ok_or_else(|| trap(&msg, "property has no bridge accessor"))?;
            if let Some(get) = &acc.get {
                get(&*e.value).map_err(|e| trap_convert(&msg, e))?
            } else if let Some(get) = &acc.get_async {
                // Driven on-thread; the table lock is held across awaits
                // (same re-entrancy caveat as async methods).
                block_on(get(CowAny::Borrowed(&*e.value))).map_err(|e| trap_convert(&msg, e))?
            } else {
                return Err(trap(&msg, "property has no getter channel"));
            }
        };
        lower_result(&cx, &mut store, fty.results(), out, results, &msg)
    })?;
    Ok(())
}

fn define_prop_set<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    res: &str,
    setter: &str,
    prop: &str,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let linker_name = format!("[method]{res}.{setter}");
    let (entry, cx, prop) = (entry.clone(), cx.clone(), prop.to_string());
    let msg = linker_name.clone();
    inst.func_new(&linker_name, move |mut store, _fty, params, _results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let args = lift_args(&cx, &mut store, params, 1, &msg)?;
        let value = args
            .into_iter()
            .next()
            .ok_or_else(|| trap(&msg, "missing setter value"))?;
        let mut guard = cx.table.lock();
        let e = guard
            .get_mut(&rep)
            .ok_or_else(|| trap(&msg, "stale resource handle"))?;
        check_type(&msg, e.type_name, entry.type_name)?;
        let acc = entry
            .props
            .get(prop.as_str())
            .ok_or_else(|| trap(&msg, "property has no bridge accessor"))?;
        if let Some(set) = &acc.set {
            set(&mut *e.value, value).map_err(|e| trap_convert(&msg, e))?;
        } else if let Some(set) = &acc.set_async {
            // In-place mutation under the held lock (method_async_mut
            // precedent); same re-entrancy caveat.
            block_on(set(&mut *e.value, value)).map_err(|e| trap_convert(&msg, e))?;
        } else {
            return Err(trap(&msg, "property is readonly"));
        }
        Ok(())
    })?;
    Ok(())
}

fn define_ctor_async<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    linker_name: &str,
    ctor_key: String,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let (entry, cx) = (entry.clone(), cx.clone());
    let msg = linker_name.to_string();
    inst.func_new(linker_name, move |mut store, _fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg)?;
        let ctor = entry
            .ctors_async
            .get(ctor_key.as_str())
            .ok_or_else(|| trap(&msg, "constructor has no bridge channel"))?;
        let value = block_on(ctor(&args)).map_err(|e| trap_convert(&msg, e))?;
        results[0] = cx.own_handle(&mut store, entry.type_name, value)?;
        Ok(())
    })?;
    Ok(())
}

fn check_type(name: &str, actual: &str, expected: &str) -> wasmtime::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(trap(
            name,
            &format!("handle is a `{actual}`, this member belongs to `{expected}`"),
        ))
    }
}

fn define_ctor<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    linker_name: &str,
    ctor_key: String,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let (entry, cx) = (entry.clone(), cx.clone());
    let msg = linker_name.to_string();
    inst.func_new(linker_name, move |mut store, _fty, params, results| {
        let args = lift_args(&cx, &mut store, params, 0, &msg)?;
        let ctor = entry
            .ctors
            .get(ctor_key.as_str())
            .ok_or_else(|| trap(&msg, "constructor has no bridge channel"))?;
        let value = ctor(&args).map_err(|e| trap_convert(&msg, e))?;
        results[0] = cx.own_handle(&mut store, entry.type_name, value)?;
        Ok(())
    })?;
    Ok(())
}

fn define_method<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    linker_name: &str,
    method: String,
    receiver: Receiver,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    let (entry, cx) = (entry.clone(), cx.clone());
    let msg = linker_name.to_string();
    inst.func_new(linker_name, move |mut store, fty, params, results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let args = lift_args(&cx, &mut store, params, 1, &msg)?;
        let m = entry
            .methods
            .get(method.as_str())
            .ok_or_else(|| trap(&msg, "method has no bridge channel"))?;
        let out = match (m, receiver) {
            // Owned receiver: consume the entry (stale later use traps).
            (EMethod::Cow(f), Receiver::Owned) => {
                let e = cx
                    .table
                    .remove(rep)
                    .ok_or_else(|| trap(&msg, "stale or already-consumed resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                f(CowAny::Owned(e.value), &args).map_err(|e| trap_convert(&msg, e))?
            }
            (EMethod::Cow(f), _) => {
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                f(CowAny::Borrowed(&*e.value), &args).map_err(|e| trap_convert(&msg, e))?
            }
            (EMethod::Mut(f), _) => {
                let mut guard = cx.table.lock();
                let e = guard
                    .get_mut(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                f(&mut *e.value, &args).map_err(|e| trap_convert(&msg, e))?
            }
            // Async dispatch: wasmtime's dynamic host functions are
            // synchronous, so the bridge future is driven to completion on
            // this thread. The table lock is held across the awaits for
            // borrowed receivers — re-entrant calls into the same binder's
            // resources from inside the future would deadlock (documented).
            (EMethod::AsyncCow(f), Receiver::Owned) => {
                let e = cx
                    .table
                    .remove(rep)
                    .ok_or_else(|| trap(&msg, "stale or already-consumed resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                block_on(f(CowAny::Owned(e.value), &args)).map_err(|e| trap_convert(&msg, e))?
            }
            (EMethod::AsyncCow(f), _) => {
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                block_on(f(CowAny::Borrowed(&*e.value), &args))
                    .map_err(|e| trap_convert(&msg, e))?
            }
            (EMethod::AsyncMut(f), _) => {
                let mut guard = cx.table.lock();
                let e = guard
                    .get_mut(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                block_on(f(&mut *e.value, &args)).map_err(|e| trap_convert(&msg, e))?
            }
        };
        lower_result(&cx, &mut store, fty.results(), out, results, &msg)
    })?;
    Ok(())
}

/// Defines one live trait projection on a resource.
fn define_projection<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    res: &str,
    name: &str,
    proj: Projected<'_>,
    entry: &Arc<ResourceEntry>,
    cx: &DispatchCx,
) -> Result<(), WasmBindError> {
    // `default` is a static constructor; everything else dispatches on the
    // receiver handle.
    if matches!(proj.kind, ProjKind::Default) {
        return define_ctor(
            inst,
            &format!("[static]{res}.{name}"),
            "default".to_string(),
            entry,
            cx,
        );
    }
    let linker_name = format!("[method]{res}.{name}");
    let (entry, cx) = (entry.clone(), cx.clone());
    let msg = linker_name.clone();
    let kind = ProjDispatch::from(&proj.kind);
    inst.func_new(&linker_name, move |mut store, fty, params, results| {
        let rep = self_rep(&mut store, params, &msg)?;
        let missing = || trap(&msg, "declared trait impl has no dispatch channel");
        let out: ScriptValue = match kind {
            ProjDispatch::ArithSelf(op) => {
                // Both operands clone out of the table (value acquisition).
                let rhs_rep = match params.get(1) {
                    Some(Val::Resource(any)) => {
                        let r: Resource<HostRep> = any.try_into_resource(&mut store)?;
                        r.rep()
                    }
                    _ => return Err(trap(&msg, "expected a resource rhs")),
                };
                let f = entry.metas.arith_self.get(op).ok_or_else(missing)?;
                let (a, b) = {
                    let guard = cx.table.lock();
                    let ea = guard
                        .get(&rep)
                        .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                    check_type(&msg, ea.type_name, entry.type_name)?;
                    let eb = guard
                        .get(&rhs_rep)
                        .ok_or_else(|| trap(&msg, "stale rhs resource handle"))?;
                    check_type(&msg, eb.type_name, entry.type_name)?;
                    ((entry.clone_any)(&*ea.value), (entry.clone_any)(&*eb.value))
                };
                let value = f(a, b);
                results[0] = cx.own_handle(&mut store, entry.type_name, value)?;
                return Ok(());
            }
            ProjDispatch::ArithScalar(op) => {
                let args = lift_args(&cx, &mut store, params, 1, &msg)?;
                let f = entry.metas.arith_scalar.get(op).ok_or_else(missing)?;
                let a = {
                    let guard = cx.table.lock();
                    let e = guard
                        .get(&rep)
                        .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                    check_type(&msg, e.type_name, entry.type_name)?;
                    (entry.clone_any)(&*e.value)
                };
                let value = f(a, &args).map_err(|e| trap_convert(&msg, e))?;
                results[0] = cx.own_handle(&mut store, entry.type_name, value)?;
                return Ok(());
            }
            ProjDispatch::Unary(bnot) => {
                let f = if bnot {
                    entry.metas.bnot.as_ref()
                } else {
                    entry.metas.unm.as_ref()
                }
                .ok_or_else(missing)?;
                let value = {
                    let guard = cx.table.lock();
                    let e = guard
                        .get(&rep)
                        .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                    check_type(&msg, e.type_name, entry.type_name)?;
                    f(&*e.value)
                };
                results[0] = cx.own_handle(&mut store, entry.type_name, value)?;
                return Ok(());
            }
            ProjDispatch::Cmp(which) => {
                let other_rep = match params.get(1) {
                    Some(Val::Resource(any)) => {
                        let r: Resource<HostRep> = any.try_into_resource(&mut store)?;
                        r.rep()
                    }
                    _ => return Err(trap(&msg, "expected a resource operand")),
                };
                let f = match which {
                    0 => entry.metas.eq.as_ref(),
                    1 => entry.metas.lt.as_ref(),
                    _ => entry.metas.le.as_ref(),
                }
                .ok_or_else(missing)?;
                let guard = cx.table.lock();
                let ea = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, ea.type_name, entry.type_name)?;
                let eb = guard
                    .get(&other_rep)
                    .ok_or_else(|| trap(&msg, "stale operand resource handle"))?;
                check_type(&msg, eb.type_name, entry.type_name)?;
                ScriptValue::Bool(f(&*ea.value, &*eb.value))
            }
            ProjDispatch::Str(debug) => {
                let f = if debug {
                    entry.metas.debug.as_ref()
                } else {
                    entry.metas.tostring.as_ref()
                }
                .ok_or_else(missing)?;
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                ScriptValue::String(f(&*e.value))
            }
            ProjDispatch::Hash => {
                let f = entry.metas.hash.as_ref().ok_or_else(missing)?;
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                // WIT `u64` result: lowered directly below.
                let h = f(&*e.value);
                drop(guard);
                results[0] = Val::U64(h);
                return Ok(());
            }
            ProjDispatch::Call { is_async } => {
                let args = lift_args(&cx, &mut store, params, 1, &msg)?;
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                if is_async {
                    let f = entry.metas.call_async.as_ref().ok_or_else(missing)?;
                    block_on(f(CowAny::Borrowed(&*e.value), &args))
                        .map_err(|e| trap_convert(&msg, e))?
                } else {
                    let f = entry.metas.call.as_ref().ok_or_else(missing)?;
                    f(&*e.value, &args).map_err(|e| trap_convert(&msg, e))?
                }
            }
            ProjDispatch::IndexGet => {
                let args = lift_args(&cx, &mut store, params, 1, &msg)?;
                let f = entry.metas.index.as_ref().ok_or_else(missing)?;
                let guard = cx.table.lock();
                let e = guard
                    .get(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                f(&*e.value, &args).map_err(|e| trap_convert(&msg, e))?
            }
            ProjDispatch::IndexSet => {
                let args = lift_args(&cx, &mut store, params, 1, &msg)?;
                let f = entry.metas.newindex.as_ref().ok_or_else(missing)?;
                let mut guard = cx.table.lock();
                let e = guard
                    .get_mut(&rep)
                    .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                check_type(&msg, e.type_name, entry.type_name)?;
                f(&mut *e.value, &args).map_err(|e| trap_convert(&msg, e))?;
                return Ok(());
            }
            ProjDispatch::Items => {
                let f = entry.metas.iter.as_ref().ok_or_else(missing)?;
                let value = {
                    let guard = cx.table.lock();
                    let e = guard
                        .get(&rep)
                        .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                    check_type(&msg, e.type_name, entry.type_name)?;
                    (entry.clone_any)(&*e.value)
                };
                ScriptValue::List(f(value).collect())
            }
            ProjDispatch::Length => {
                let f = entry.metas.len.as_ref().ok_or_else(missing)?;
                let value = {
                    let guard = cx.table.lock();
                    let e = guard
                        .get(&rep)
                        .ok_or_else(|| trap(&msg, "stale resource handle"))?;
                    check_type(&msg, e.type_name, entry.type_name)?;
                    (entry.clone_any)(&*e.value)
                };
                results[0] = Val::U64(f(value) as u64);
                return Ok(());
            }
            ProjDispatch::Default => unreachable!("handled above"),
        };
        lower_result(&cx, &mut store, fty.results(), out, results, &msg)
    })?;
    Ok(())
}

/// `'static` dispatch selector derived from a borrowed [`ProjKind`].
#[derive(Clone, Copy)]
enum ProjDispatch {
    ArithSelf(&'static str),
    ArithScalar(&'static str),
    Unary(bool),
    Cmp(u8),
    Str(bool),
    Hash,
    Call { is_async: bool },
    IndexGet,
    IndexSet,
    Items,
    Length,
    Default,
}

impl ProjDispatch {
    fn from(kind: &ProjKind<'_>) -> Self {
        match kind {
            ProjKind::ArithSelf { op, .. } => Self::ArithSelf(op),
            ProjKind::ArithScalar { op, .. } => Self::ArithScalar(op),
            ProjKind::Neg { .. } => Self::Unary(false),
            ProjKind::BNot { .. } => Self::Unary(true),
            ProjKind::Eq => Self::Cmp(0),
            ProjKind::Lt => Self::Cmp(1),
            ProjKind::Le => Self::Cmp(2),
            ProjKind::ToString => Self::Str(false),
            ProjKind::DebugString => Self::Str(true),
            ProjKind::Hash => Self::Hash,
            ProjKind::Call { is_async, .. } => Self::Call {
                is_async: *is_async,
            },
            ProjKind::IndexGet { .. } => Self::IndexGet,
            ProjKind::IndexSet { .. } => Self::IndexSet,
            ProjKind::Items { .. } => Self::Items,
            ProjKind::Length => Self::Length,
            ProjKind::Default => Self::Default,
        }
    }
}

/// Registers a resource surface as descriptive trap stubs (the type was
/// never registered with `register_type`), using the same member names the
/// generator emits (canonical-ABI prefixed). The destructor still removes
/// table entries so mixed registered/unregistered surfaces stay sound.
fn bind_resource_stub<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    s: &haphe::StructDescriptor<'_>,
    res: &str,
    table: &Arc<HostTable>,
) -> Result<(), WasmBindError> {
    let dtor_table = table.clone();
    inst.resource(res, ResourceType::host::<HostRep>(), move |_, rep| {
        dtor_table.remove(rep);
        Ok(())
    })?;
    let mut members = NameMap::new();
    let unregistered = "declared but not registered; call `register_type` on the binder";

    for field in s.fields {
        let getter = members.insert(field.name)?;
        stub_func_msg(inst, &format!("[method]{res}.{getter}"), unregistered)?;
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            stub_func_msg(inst, &format!("[method]{res}.{setter}"), unregistered)?;
        }
    }
    for prop in s.properties {
        let getter = members.insert(prop.name)?;
        stub_func_msg(inst, &format!("[method]{res}.{getter}"), unregistered)?;
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            stub_func_msg(inst, &format!("[method]{res}.{setter}"), unregistered)?;
        }
    }

    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            stub_func_msg(inst, &format!("[constructor]{res}"), unregistered)?;
        } else {
            let name = members.insert(ctor.name)?;
            stub_func_msg(inst, &format!("[static]{res}.{name}"), unregistered)?;
        }
    }

    for m in s.methods {
        let name = members.insert(m.name)?;
        let prefixed = match m.receiver {
            Some(Receiver::Ref | Receiver::RefMut) => format!("[method]{res}.{name}"),
            Some(Receiver::Owned) | None => format!("[static]{res}.{name}"),
        };
        stub_func_msg(inst, &prefixed, unregistered)?;
    }

    for proj in projected_trait_members(s)? {
        let name = members.insert_as(
            proj.source_name,
            &format!("trait projection `{}`", proj.source_name),
        )?;
        let prefixed = if matches!(proj.kind, ProjKind::Default) {
            format!("[static]{res}.{name}")
        } else {
            format!("[method]{res}.{name}")
        };
        stub_func_msg(inst, &prefixed, unregistered)?;
    }
    Ok(())
}

fn stub_func<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
) -> Result<(), WasmBindError> {
    stub_func_msg(
        inst,
        name,
        "not registered; call the binder's `register_*` method",
    )
}

fn stub_func_msg<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
    detail: &str,
) -> Result<(), WasmBindError> {
    let msg = format!("haphe-wit: `{name}`: {detail}");
    inst.func_new(name, move |_store, _ty, _params, _results| {
        Err(wasmtime::Error::msg(msg.clone()))
    })?;
    Ok(())
}

/// Parses a constant's IR string value into a component [`Val`]
/// or returns an error if the value cannot be represented in the declared type.
fn constant_to_val(c: &ConstantDescriptor<'_>) -> Result<Val, WasmBindError> {
    let err = |detail: &str| WasmBindError::UnsupportedConstant {
        name: c.name.to_string(),
        detail: detail.to_string(),
    };
    let v = c.value;
    match crate::types::peel_borrowed(c.ty) {
        TypeDescriptor::String => Ok(Val::String(v.to_string())),
        TypeDescriptor::Primitive(p) => match p {
            PrimitiveType::Bool => match v {
                "true" => Ok(Val::Bool(true)),
                "false" => Ok(Val::Bool(false)),
                _ => Err(err("not a bool")),
            },
            PrimitiveType::I8 => v.parse().map(Val::S8).map_err(|_| err("not an s8")),
            PrimitiveType::I16 => v.parse().map(Val::S16).map_err(|_| err("not an s16")),
            PrimitiveType::I32 => v.parse().map(Val::S32).map_err(|_| err("not an s32")),
            PrimitiveType::I64 => v.parse().map(Val::S64).map_err(|_| err("not an s64")),
            PrimitiveType::U8 => v.parse().map(Val::U8).map_err(|_| err("not a u8")),
            PrimitiveType::U16 => v.parse().map(Val::U16).map_err(|_| err("not a u16")),
            PrimitiveType::U32 => v.parse().map(Val::U32).map_err(|_| err("not a u32")),
            PrimitiveType::U64 => v.parse().map(Val::U64).map_err(|_| err("not a u64")),
            PrimitiveType::F32 => v.parse().map(Val::Float32).map_err(|_| err("not an f32")),
            PrimitiveType::F64 => v.parse().map(Val::Float64).map_err(|_| err("not an f64")),
            PrimitiveType::Char => v.parse().map(Val::Char).map_err(|_| err("not a char")),
            _ => Err(err("unsupported primitive constant type")),
        },
        _ => Err(err("only primitive and string constants can be bound")),
    }
}
