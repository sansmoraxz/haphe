use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, ForeignCaller, ForeignError,
    ForeignErrorKind, ForeignHandle, ForeignInterfaceDescriptor, PrimitiveType, Receiver,
    RuntimeBinder, ScriptConvertError, ScriptForeign, ScriptValue, TypeDescriptor, TypeKind,
    ValidatedRegistry,
};
use wasmtime::Store;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Func, Instance, Linker, LinkerInstance, ResourceType, Type, Val};

use crate::model::{Direction, Plan};
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
    _marker: PhantomData<fn(T)>,
}

impl<T> WasmBinder<T> {
    /// Creates a binder from the same configuration used to generate the
    /// `.wit` document, so linker names always match the generated file.
    pub fn new(config: WitGenerator) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

/// Placeholder host representation for resources until real dispatch lands.
struct StubResource;

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
        BindingGenerator::capabilities(&self.config)
    }

    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        linker: &mut Linker<T>,
    ) -> Result<(), WasmBindError> {
        crate::validate_package_name(&self.config.package)?;
        let plan = Plan::build(registry, &self.config.default_interface)?;

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
                        bind_resource(&mut inst, s, plan.type_name(id))?;
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
                    bind_resource(&mut inst, s, &planned.wit_name)?;
                }
            }

            // Companion functions for enum methods, same names the generator
            // emits.
            for id in &iface.type_ids {
                if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(id)) {
                    for m in e.methods {
                        let name = member_names.insert(&format!("{}_{}", e.name, m.name))?;
                        stub_func(&mut inst, &name)?;
                    }
                }
            }
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Enum(e)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                {
                    for m in e.methods {
                        let name =
                            member_names.insert(&format!("{}-{}", planned.wit_name, m.name))?;
                        stub_func(&mut inst, &name)?;
                    }
                }
            }

            for func in iface.functions {
                if func.generic_params.is_empty() {
                    let name = member_names.insert(func.name)?;
                    stub_func(&mut inst, &name)?;
                    continue;
                }
                // One stub per declared instantiation — fn-site and
                // registry-level sources unioned, same mangled names the
                // generator emits (`generics` feature; rejected in
                // `Plan::build` otherwise).
                for args in haphe::union_instantiations(func, iface.fn_instantiations) {
                    let mangled = plan.mangle_fn_instance(func.name, args, None)?;
                    let name = member_names.insert(&mangled)?;
                    stub_func(&mut inst, &name)?;
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

/// Guest (kebab) enum case name -> declared case name; `None` marks a kebab
/// spelling shared by differently-declared variants (ambiguous).
type EnumNames = HashMap<String, Option<String>>;

/// Builds the guest-to-declared enum case translation from a registry's
/// enum descriptors.
fn declared_enum_cases(registry: &ValidatedRegistry<'_>) -> EnumNames {
    let mut map = EnumNames::new();
    for e in registry.enums() {
        for v in e.variants {
            let declared = v.name.to_string();
            map.entry(to_kebab(v.name))
                .and_modify(|existing| {
                    if existing.as_deref() != Some(declared.as_str()) {
                        *existing = None;
                    }
                })
                .or_insert(Some(declared));
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

    fn lower_args(
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
        args.iter()
            .zip(&f.params)
            .map(|(arg, ty)| script_to_val(arg, ty))
            .collect::<Result<_, _>>()
            .map_err(|e| convert_error(function, e))
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
                Ok(Some(v)) => {
                    val_to_script(&v, &self.enum_cases).map_err(|e| convert_error(function, e))
                }
                Ok(None) => Ok(ScriptValue::Unit),
                Err(payload) => {
                    let detail = match payload {
                        Some(v) => match val_to_script(&v, &self.enum_cases) {
                            Ok(ScriptValue::String(s)) => s,
                            Ok(other) => format!("{other:?}"),
                            Err(_) => "guest returned an error".to_string(),
                        },
                        None => "guest returned an error".to_string(),
                    };
                    Err(call_error(function, detail))
                }
            },
            Some(v) => val_to_script(&v, &self.enum_cases).map_err(|e| convert_error(function, e)),
        }
    }

    fn dispatch_sync(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        let f = self.find(function, type_args)?;
        let vals = Self::lower_args(f, function, args)?;
        let mut store = self.store.lock().expect("store mutex poisoned");
        let mut results = vec![Val::Bool(false); usize::from(f.result.is_some())];
        f.func
            .call(&mut *store, &vals, &mut results)
            .map_err(|e| call_error(function, e.to_string()))?;
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
        let vals = Self::lower_args(f, function, args)?;
        let mut store = self.store.lock().expect("store mutex poisoned");
        let mut results = vec![Val::Bool(false); usize::from(f.result.is_some())];
        f.func
            .call_async(&mut *store, &vals, &mut results)
            .await
            .map_err(|e| call_error(function, e.to_string()))?;
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

/// Converts a [`ScriptValue`] argument into the [`Val`] shape the guest's
/// reflected parameter type expects. Records, variants, resources, and other
/// composite guest types are not supported yet.
fn script_to_val(v: &ScriptValue, ty: &Type) -> Result<Val, ScriptConvertError> {
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
                    .map(|item| script_to_val(item, &l.ty()))
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
                                script_to_val(&ScriptValue::String(k.clone()), &key_ty)?,
                                script_to_val(value, &value_ty)?,
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
                    .map(|item| script_to_val(item, &l.ty()))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(err("fixed-length list")),
        },
        Type::Option(o) => match v {
            ScriptValue::Optional(None) | ScriptValue::Unit => Val::Option(None),
            ScriptValue::Optional(Some(inner)) => {
                Val::Option(Some(Box::new(script_to_val(inner, &o.ty())?)))
            }
            other_v => Val::Option(Some(Box::new(script_to_val(other_v, &o.ty())?))),
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
                    fields.push((field.name.to_string(), script_to_val(value, &field.ty)?));
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
                    .map(|(item, ty)| script_to_val(item, &ty))
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err(err("tuple")),
        },
        Type::Map(m) => match v {
            ScriptValue::Map(pairs) => Val::Map(
                pairs
                    .iter()
                    .map(|(k, value)| {
                        Ok((
                            script_to_val(&ScriptValue::String(k.clone()), &m.key())?,
                            script_to_val(value, &m.value())?,
                        ))
                    })
                    .collect::<Result<_, ScriptConvertError>>()?,
            ),
            _ => return Err(err("map")),
        },
        Type::Enum(e) => {
            let case = match v {
                ScriptValue::Enum { case } => case.as_str(),
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
        _ => return Err(err("a wasm-representable value")),
    })
}

/// Converts a guest return [`Val`] back into a [`ScriptValue`].
///
/// Enum cases are translated from the guest's spelling back to the declared
/// case name through `enum_cases`; callers built without a registry have an
/// empty map, so enum returns error there — build the caller with the
/// registry-aware constructor instead. Resources are not supported yet.
fn val_to_script(v: &Val, enum_cases: &EnumNames) -> Result<ScriptValue, ScriptConvertError> {
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
                .map(|item| val_to_script(item, enum_cases))
                .collect::<Result<_, _>>()?,
        ),
        Val::Option(opt) => ScriptValue::Optional(match opt {
            Some(inner) => Some(Box::new(val_to_script(inner, enum_cases)?)),
            None => None,
        }),
        Val::Record(fields) => ScriptValue::Map(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), val_to_script(v, enum_cases)?)))
                .collect::<Result<_, ScriptConvertError>>()?,
        ),
        Val::Map(pairs) => ScriptValue::Map(
            pairs
                .iter()
                .map(|(k, v)| match k {
                    Val::String(s) => Ok((s.clone(), val_to_script(v, enum_cases)?)),
                    _ => Err(err.clone()),
                })
                .collect::<Result<_, ScriptConvertError>>()?,
        ),
        Val::Enum(name) => match enum_cases.get(name) {
            Some(Some(declared)) => ScriptValue::Enum {
                case: declared.clone(),
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
                Some(p) => val_to_script(p, enum_cases)?,
                None => ScriptValue::Unit,
            },
        )]),
        _ => return Err(err),
    })
}

/// Registers a resource placeholder plus trap-stubs for its constructor,
/// field/property accessors, and methods, using the same member names the
/// generator emits (canonical-ABI prefixed).
fn bind_resource<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    s: &haphe::StructDescriptor<'_>,
    res: &str,
) -> Result<(), WasmBindError> {
    inst.resource(res, ResourceType::host::<StubResource>(), |_, _| Ok(()))?;
    let mut members = NameMap::new();

    for field in s.fields {
        let getter = members.insert(field.name)?;
        stub_func(inst, &format!("[method]{res}.{getter}"))?;
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            stub_func(inst, &format!("[method]{res}.{setter}"))?;
        }
    }
    for prop in s.properties {
        let getter = members.insert(prop.name)?;
        stub_func(inst, &format!("[method]{res}.{getter}"))?;
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            stub_func(inst, &format!("[method]{res}.{setter}"))?;
        }
    }

    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            stub_func(inst, &format!("[constructor]{res}"))?;
        } else {
            let name = members.insert(ctor.name)?;
            stub_func(inst, &format!("[static]{res}.{name}"))?;
        }
    }

    for m in s.methods {
        let name = members.insert(m.name)?;
        let prefixed = match m.receiver {
            Some(Receiver::Ref | Receiver::RefMut) => format!("[method]{res}.{name}"),
            Some(Receiver::Owned) | None => format!("[static]{res}.{name}"),
        };
        stub_func(inst, &prefixed)?;
    }
    Ok(())
}

fn stub_func<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
) -> Result<(), WasmBindError> {
    let msg = format!("haphe-wit: `{name}` not yet implemented");
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
    match c.ty {
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
