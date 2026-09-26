use std::any::TypeId;
use std::collections::HashMap;
use std::marker::PhantomData;

use haphe::bridge::{FromScript, IntoScript, ScriptValue};
use haphe::{
    FnBinder, ScriptCallError, ScriptCallFuture, ScriptConvertError, ScriptCow, ScriptIter,
    TypeBinder,
};
use rhai::{Dynamic, EvalAltResult, NativeCallContext, Position};

use crate::RhaiBindError;
use crate::error::{call_error, convert_error};

#[cfg(feature = "sync")]
pub trait RhaiSendSync: Send + Sync {}
#[cfg(feature = "sync")]
impl<T: Send + Sync> RhaiSendSync for T {}

#[cfg(not(feature = "sync"))]
pub trait RhaiSendSync {}
#[cfg(not(feature = "sync"))]
impl<T> RhaiSendSync for T {}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Payload enum carried through Rhai's `Dynamic` as a distinct custom
/// type — never confused with a regular map.
#[derive(Clone, Debug)]
pub(crate) struct RhaiPayloadEnum {
    pub case: String,
    pub payload: Vec<Dynamic>,
}

pub(crate) fn script_to_dynamic(v: ScriptValue) -> Dynamic {
    match v {
        ScriptValue::Bool(b) => Dynamic::from(b),
        ScriptValue::I64(n) => Dynamic::from(n),
        ScriptValue::F64(n) => Dynamic::from(n),
        ScriptValue::String(s) => Dynamic::from(s),
        ScriptValue::Char(c) => Dynamic::from(c),
        ScriptValue::Bytes(b) => {
            let blob: rhai::Blob = b;
            Dynamic::from(blob)
        }
        ScriptValue::List(items) => {
            let arr: rhai::Array = items.into_iter().map(script_to_dynamic).collect();
            Dynamic::from(arr)
        }
        ScriptValue::Map(pairs) => {
            let mut map = rhai::Map::new();
            for (k, v) in pairs {
                map.insert(k.into(), script_to_dynamic(v));
            }
            Dynamic::from(map)
        }
        ScriptValue::Optional(Some(inner)) => script_to_dynamic(*inner),
        ScriptValue::Optional(None) | ScriptValue::Unit => Dynamic::UNIT,
        ScriptValue::UserData(ud) => Dynamic::from(ud),
        ScriptValue::Enum {
            case,
            discriminant,
            payload,
        } => {
            if payload.is_empty() {
                match discriminant {
                    Some(value) => Dynamic::from(value),
                    None => Dynamic::from(case),
                }
            } else {
                Dynamic::from(RhaiPayloadEnum {
                    case,
                    payload: payload.into_iter().map(script_to_dynamic).collect(),
                })
            }
        }
        _ => unreachable!("ScriptValue variant not representable in Rhai"),
    }
}

pub(crate) fn dynamic_to_script(v: Dynamic) -> Result<ScriptValue, Box<EvalAltResult>> {
    if v.is_unit() {
        return Ok(ScriptValue::Unit);
    }
    if v.is_bool() {
        return Ok(ScriptValue::Bool(v.as_bool().expect("checked")));
    }
    if v.is_int() {
        return Ok(ScriptValue::I64(v.as_int().expect("checked")));
    }
    if v.is_float() {
        return Ok(ScriptValue::F64(v.as_float().expect("checked")));
    }
    if v.is_char() {
        return Ok(ScriptValue::Char(v.as_char().expect("checked")));
    }
    if v.is_string() {
        return Ok(ScriptValue::String(
            v.into_immutable_string()
                .expect("checked")
                .into_owned(),
        ));
    }
    if v.is_blob() {
        return Ok(ScriptValue::Bytes(v.cast::<rhai::Blob>()));
    }
    if v.is_array() {
        let arr = v.cast::<rhai::Array>();
        let items: Result<Vec<_>, _> = arr.into_iter().map(dynamic_to_script).collect();
        return Ok(ScriptValue::List(items?));
    }
    if v.is_map() {
        let map = v.cast::<rhai::Map>();
        let mut pairs = Vec::with_capacity(map.len());
        for (k, v) in map {
            pairs.push((k.to_string(), dynamic_to_script(v)?));
        }
        return Ok(ScriptValue::Map(pairs));
    }
    if v.is::<RhaiPayloadEnum>() {
        let e = v.cast::<RhaiPayloadEnum>();
        let payload: Result<Vec<_>, _> =
            e.payload.into_iter().map(dynamic_to_script).collect();
        return Ok(ScriptValue::Enum {
            case: e.case,
            discriminant: None,
            payload: payload?,
        });
    }
    if v.is::<haphe::OpaqueUserData>() {
        return Ok(ScriptValue::UserData(v.cast::<haphe::OpaqueUserData>()));
    }
    Err(Box::new(EvalAltResult::ErrorRuntime(
        format!("cannot convert {} to script value", v.type_name()).into(),
        Position::NONE,
    )))
}


fn lookup_arity(name: &str, method_arities: &HashMap<&str, usize>) -> usize {
    method_arities
        .get(name)
        .or_else(|| name.find("__").map(|i| &name[..i]).and_then(|base| method_arities.get(base)))
        .copied()
        .unwrap_or(0)
}

fn args_to_script(args: &mut [&mut Dynamic]) -> Result<Vec<ScriptValue>, Box<EvalAltResult>> {
    args.iter_mut()
        .map(|d| dynamic_to_script(std::mem::take(*d)))
        .collect()
}

// ---------------------------------------------------------------------------
// Generic name mangling
// ---------------------------------------------------------------------------

pub(crate) fn mangle_generic_name(
    base: &str,
    type_args: &[haphe::TypeDescriptor<'_>],
) -> String {
    let mut name = base.to_owned();
    for arg in type_args {
        name.push_str("__");
        write_type_slug(&mut name, arg);
    }
    name
}

fn write_type_slug(buf: &mut String, td: &haphe::TypeDescriptor<'_>) {
    use haphe::TypeDescriptor;
    match td {
        TypeDescriptor::Primitive(p) => {
            use std::fmt::Write;
            write!(buf, "{p:?}").expect("infallible");
            buf.make_ascii_lowercase();
        }
        TypeDescriptor::String => buf.push_str("string"),
        TypeDescriptor::Bytes => buf.push_str("bytes"),
        TypeDescriptor::Unit => buf.push_str("unit"),
        TypeDescriptor::Option(inner) => {
            buf.push_str("opt_");
            write_type_slug(buf, inner);
        }
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => {
            buf.push_str("list_");
            write_type_slug(buf, inner);
        }
        TypeDescriptor::Map(k, v) => {
            buf.push_str("map_");
            write_type_slug(buf, k);
            buf.push('_');
            write_type_slug(buf, v);
        }
        TypeDescriptor::Tuple(elems) => {
            buf.push_str("tuple");
            for e in *elems {
                buf.push('_');
                write_type_slug(buf, e);
            }
        }
        TypeDescriptor::Ref(id) | TypeDescriptor::Instance { id, .. } => {
            let short = id.as_str().rsplit("::").next().unwrap_or("ref");
            for c in short.chars() {
                if c.is_ascii_alphanumeric() {
                    buf.push(c.to_ascii_lowercase());
                } else {
                    buf.push('_');
                }
            }
        }
        TypeDescriptor::Borrowed { inner, .. } => write_type_slug(buf, inner),
        TypeDescriptor::Result(ok, err) => {
            buf.push_str("result_");
            write_type_slug(buf, ok);
            buf.push('_');
            write_type_slug(buf, err);
        }
        TypeDescriptor::GenericParam(name) => buf.push_str(name),
        TypeDescriptor::Callback { .. }
        | TypeDescriptor::Stream(_)
        | TypeDescriptor::Future(_) => buf.push_str("opaque"),
        _ => unreachable!("unhandled TypeDescriptor variant in name mangling"),
    }
}

// ---------------------------------------------------------------------------
// Operator name mapping
// ---------------------------------------------------------------------------

fn op_to_rhai_symbol(op: &str) -> Option<&'static str> {
    match op {
        "add" => Some("+"),
        "sub" => Some("-"),
        "mul" => Some("*"),
        "div" => Some("/"),
        "rem" | "mod" => Some("%"),
        "bitand" => Some("&"),
        "bitor" => Some("|"),
        "bitxor" => Some("^"),
        "shl" => Some("<<"),
        "shr" => Some(">>"),
        "pow" => Some("**"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// RhaiTypeBinder<T>
// ---------------------------------------------------------------------------

type FieldGetErased<T> = Box<dyn Fn(&T) -> ScriptValue + Send + Sync>;
type FieldSetErased<T> =
    Box<dyn Fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError> + Send + Sync>;

struct FieldEntry<T> {
    name: &'static str,
    getter: FieldGetErased<T>,
    setter: Option<FieldSetErased<T>>,
}

type MethodFn<T> =
    for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type MethodMutFn<T> = fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type CtorFn<T> = fn(&[ScriptValue]) -> Result<T, ScriptCallError>;
type AssocFn = fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type CallFn<T> = fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;
type IndexFn<T> = fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;
type NewIndexFn<T> = fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>;
type PropertyGetFn<T> = fn(&T) -> ScriptValue;
type PropertySetFn<T> = fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>;
type ArithSelfFn<T> = fn(T, T) -> T;
type ArithScalarFn<T> = fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>;
type NamedEntry<F> = (&'static str, F);

// ---------------------------------------------------------------------------
// Dyn dispatch types
// ---------------------------------------------------------------------------

#[cfg(feature = "generics")]
struct DynEntry<F> {
    descriptor: &'static haphe::FunctionDescriptor<'static>,
    type_args: &'static [haphe::TypeDescriptor<'static>],
    #[allow(dead_code, reason = "stored for future merged-substitution dyn dispatch on generic self types")]
    self_params: &'static [haphe::GenericParam<'static>],
    #[allow(dead_code, reason = "stored for future merged-substitution dyn dispatch on generic self types")]
    self_args: &'static [haphe::TypeDescriptor<'static>],
    wrapper: F,
}

pub(crate) struct RhaiTypeBinder<T> {
    fields: Vec<FieldEntry<T>>,
    methods: HashMap<&'static str, MethodFn<T>>,
    methods_mut: HashMap<&'static str, MethodMutFn<T>>,
    constructors: Vec<NamedEntry<CtorFn<T>>>,
    associated: Vec<NamedEntry<AssocFn>>,
    property_getters: Vec<NamedEntry<PropertyGetFn<T>>>,
    property_setters: Vec<NamedEntry<PropertySetFn<T>>>,
    tostring: Option<fn(&T) -> String>,
    concat: Option<fn(&T) -> String>,
    hash_fn: Option<fn(&T) -> u64>,
    debug_fn: Option<fn(&T) -> String>,
    eq_fn: Option<fn(&T, &T) -> bool>,
    lt_fn: Option<fn(&T, &T) -> bool>,
    le_fn: Option<fn(&T, &T) -> bool>,
    unm: Option<fn(&T) -> T>,
    bnot: Option<fn(&T) -> T>,
    arith_self: Vec<NamedEntry<ArithSelfFn<T>>>,
    arith_scalar: Vec<(
        &'static str,
        &'static haphe::TypeDescriptor<'static>,
        ArithScalarFn<T>,
    )>,
    iter_fn: Option<fn(T) -> ScriptIter>,
    len_fn: Option<fn(T) -> usize>,
    call_fn: Option<CallFn<T>>,
    index_fn: Option<IndexFn<T>>,
    newindex_fn: Option<NewIndexFn<T>>,
    #[cfg(feature = "generics")]
    dyn_methods: HashMap<&'static str, Vec<DynEntry<MethodFn<T>>>>,
    #[cfg(feature = "generics")]
    dyn_methods_mut: HashMap<&'static str, Vec<DynEntry<MethodMutFn<T>>>>,
    #[cfg(feature = "generics")]
    dyn_associated: HashMap<&'static str, Vec<DynEntry<AssocFn>>>,
    _phantom: PhantomData<T>,
}

impl<T: Clone + RhaiSendSync + 'static> RhaiTypeBinder<T> {
    pub(crate) fn new() -> Self {
        Self {
            fields: Vec::new(),
            methods: HashMap::new(),
            methods_mut: HashMap::new(),
            constructors: Vec::new(),
            associated: Vec::new(),
            property_getters: Vec::new(),
            property_setters: Vec::new(),
            tostring: None,
            concat: None,
            hash_fn: None,
            debug_fn: None,
            eq_fn: None,
            lt_fn: None,
            le_fn: None,
            unm: None,
            bnot: None,
            arith_self: Vec::new(),
            arith_scalar: Vec::new(),
            iter_fn: None,
            len_fn: None,
            call_fn: None,
            index_fn: None,
            newindex_fn: None,
            #[cfg(feature = "generics")]
            dyn_methods: HashMap::new(),
            #[cfg(feature = "generics")]
            dyn_methods_mut: HashMap::new(),
            #[cfg(feature = "generics")]
            dyn_associated: HashMap::new(),
            _phantom: PhantomData,
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one registration pass per kind; splitting would scatter related logic"
    )]
    #[allow(
        clippy::unnecessary_wraps,
        reason = "Result aligns with the RuntimeBinder contract; future validation may fail"
    )]
    pub(crate) fn register(
        self,
        engine: &mut rhai::Engine,
        type_name: &str,
        method_arities: &HashMap<&str, usize>,
    ) -> Result<(), RhaiBindError> {
        engine.register_type_with_name::<T>(type_name);

        let Self {
            fields,
            methods,
            methods_mut,
            constructors,
            associated,
            property_getters,
            property_setters,
            tostring,
            concat,
            hash_fn,
            debug_fn,
            eq_fn,
            lt_fn,
            le_fn,
            unm,
            bnot,
            arith_self,
            arith_scalar,
            iter_fn,
            len_fn,
            call_fn,
            index_fn,
            newindex_fn,
            #[cfg(feature = "generics")]
            dyn_methods,
            #[cfg(feature = "generics")]
            dyn_methods_mut,
            #[cfg(feature = "generics")]
            dyn_associated,
            _phantom: _,
        } = self;

        // ---- fields ----
        for entry in fields {
            let getter = entry.getter;
            engine.register_get(entry.name, move |obj: &mut T| -> Dynamic {
                script_to_dynamic(getter(obj))
            });
            if let Some(setter) = entry.setter {
                engine.register_set(entry.name, move |obj: &mut T, val: Dynamic| {
                    let sv = dynamic_to_script(val)?;
                    setter(obj, sv).map_err(convert_error)
                });
            }
        }

        // ---- computed properties ----
        for (name, getter) in &property_getters {
            let g = *getter;
            engine.register_get(*name, move |obj: &mut T| -> Dynamic {
                script_to_dynamic(g(obj))
            });
        }
        for (name, setter) in &property_setters {
            let s = *setter;
            engine.register_set(*name, move |obj: &mut T, val: Dynamic| {
                let sv = dynamic_to_script(val)?;
                s(obj, sv).map_err(convert_error)
            });
        }

        // ---- methods (non-mutating) ----
        for (name, bridge_fn) in &methods {
            let f = *bridge_fn;
            let n: &'static str = name;
            let arity = lookup_arity(n, method_arities);
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(1 + arity);
            arg_types.push(TypeId::of::<T>());
            arg_types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), arity));
            engine.register_raw_fn(
                n,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let obj: T = args[0].clone().cast::<T>();
                    let script_args = args_to_script(&mut args[1..])?;
                    let cow = ScriptCow::Owned(obj);
                    let result = f(cow, &script_args).map_err(call_error)?;
                    Ok(script_to_dynamic(result))
                },
            );
        }

        // ---- methods (mutating) ----
        for (name, bridge_fn) in &methods_mut {
            let f = *bridge_fn;
            let n: &'static str = name;
            let arity = lookup_arity(n, method_arities);
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(1 + arity);
            arg_types.push(TypeId::of::<T>());
            arg_types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), arity));
            engine.register_raw_fn(
                n,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let mut obj: T = std::mem::take(args[0]).cast::<T>();
                    let script_args = args_to_script(&mut args[1..])?;
                    let result = f(&mut obj, &script_args).map_err(call_error)?;
                    *args[0] = Dynamic::from(obj);
                    Ok(script_to_dynamic(result))
                },
            );
        }

        // ---- metamethods ----
        if let Some(f) = tostring {
            engine.register_fn("to_string", move |obj: &mut T| -> String { f(obj) });
        }
        if let Some(f) = debug_fn {
            engine.register_fn("to_debug", move |obj: &mut T| -> String { f(obj) });
        }
        if let Some(f) = hash_fn {
            engine.register_fn("hash", move |obj: &mut T| -> i64 { f(obj) as i64 });
        }
        if let Some(f) = eq_fn {
            engine.register_fn("==", move |a: &mut T, b: T| -> bool { f(a, &b) });
            engine.register_fn("!=", move |a: &mut T, b: T| -> bool { !f(a, &b) });
        }
        if let Some(f) = lt_fn {
            engine.register_fn("<", move |a: &mut T, b: T| -> bool { f(a, &b) });
        }
        if let Some(f) = le_fn {
            engine.register_fn("<=", move |a: &mut T, b: T| -> bool { f(a, &b) });
        }
        if let (Some(lt), Some(le)) = (lt_fn, le_fn) {
            engine.register_fn(">", move |a: &mut T, b: T| -> bool { lt(&b, a) });
            engine.register_fn(">=", move |a: &mut T, b: T| -> bool { le(&b, a) });
        }
        if let Some(f) = unm {
            engine.register_fn("-", move |obj: &mut T| -> T { f(obj) });
        }
        if let Some(f) = bnot {
            engine.register_fn("!", move |obj: &mut T| -> T { f(obj) });
        }
        if let Some(f) = concat {
            engine.register_fn("+", move |a: &mut T, b: &str| -> String {
                format!("{}{b}", f(a))
            });
        }

        for &(op, f) in &arith_self {
            if let Some(sym) = op_to_rhai_symbol(op) {
                engine.register_fn(sym, move |a: &mut T, b: T| -> T { f(a.clone(), b) });
            }
        }

        for &(op, _rhs_desc, f) in &arith_scalar {
            if let Some(sym) = op_to_rhai_symbol(op) {
                engine.register_raw_fn(
                    sym,
                    [TypeId::of::<T>(), TypeId::of::<Dynamic>()],
                    move |_ctx: NativeCallContext,
                          args: &mut [&mut Dynamic]|
                          -> Result<Dynamic, Box<EvalAltResult>> {
                        let lhs: T = args[0].clone().cast::<T>();
                        let rhs_sv = dynamic_to_script(std::mem::take(args[1]))?;
                        let result = f(lhs, &[rhs_sv]).map_err(convert_error)?;
                        Ok(Dynamic::from(result))
                    },
                );
            }
        }

        // ---- constructors ----
        // Rhai reserves `new` as a keyword, so constructors are registered
        // under the TYPE name (e.g. `Point(3.0, 4.0)`) which matches Rhai
        // convention.
        for (ctor_name, f) in constructors {
            let arity = method_arities.get(ctor_name).copied().unwrap_or(0);
            let arg_types: Vec<TypeId> =
                std::iter::repeat_n(TypeId::of::<Dynamic>(), arity).collect();
            engine.register_raw_fn(
                type_name,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let script_args = args_to_script(args)?;
                    let result = f(&script_args).map_err(call_error)?;
                    Ok(Dynamic::from(result))
                },
            );
        }

        // ---- associated functions ----
        for (name, f) in associated {
            let arity = lookup_arity(name, method_arities);
            let arg_types: Vec<TypeId> =
                std::iter::repeat_n(TypeId::of::<Dynamic>(), arity).collect();
            engine.register_raw_fn(
                name,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let script_args = args_to_script(args)?;
                    let result = f(&script_args).map_err(call_error)?;
                    Ok(script_to_dynamic(result))
                },
            );
        }

        // ---- dyn dispatch ----
        #[cfg(feature = "generics")]
        {
            register_dyn_methods(engine, dyn_methods);
            register_dyn_methods_mut(engine, dyn_methods_mut);
            register_dyn_associated(engine, dyn_associated);
        }

        // ---- iteration ----
        if let Some(iter_fn) = iter_fn {
            engine.register_fn("to_array", move |obj: &mut T| -> rhai::Array {
                iter_fn(obj.clone()).map(script_to_dynamic).collect()
            });
            if let Some(len_fn) = len_fn {
                engine.register_fn("len", move |obj: &mut T| -> i64 {
                    len_fn(obj.clone()) as i64
                });
            }
        }

        // ---- indexing ----
        if let Some(index_fn) = index_fn {
            engine.register_indexer_get(
                move |obj: &mut T, idx: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                    let sv_idx = dynamic_to_script(idx)?;
                    let result = index_fn(obj, &[sv_idx]).map_err(convert_error)?;
                    Ok(script_to_dynamic(result))
                },
            );
        }
        if let Some(newindex_fn) = newindex_fn {
            engine.register_indexer_set(
                move |obj: &mut T, idx: Dynamic, val: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let sv_idx = dynamic_to_script(idx)?;
                    let sv_val = dynamic_to_script(val)?;
                    newindex_fn(obj, &[sv_idx, sv_val]).map_err(convert_error)
                },
            );
        }

        // ---- call ----
        // Rhai reserves `call` for `FnPtr.call()`; callable userdata is
        // registered under `invoke` instead.
        if let Some(call_fn) = call_fn {
            let call_arity = method_arities.get("call").copied().unwrap_or(0);
            let mut arg_types: Vec<TypeId> = Vec::with_capacity(1 + call_arity);
            arg_types.push(TypeId::of::<T>());
            arg_types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), call_arity));
            engine.register_raw_fn(
                "invoke",
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let obj: T = args[0].clone().cast::<T>();
                    let call_args = args_to_script(&mut args[1..])?;
                    let result = call_fn(&obj, &call_args).map_err(convert_error)?;
                    Ok(script_to_dynamic(result))
                },
            );
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Dyn dispatch helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "generics")]
fn register_dyn_methods<T: Clone + RhaiSendSync + 'static>(
    engine: &mut rhai::Engine,
    dyn_methods: HashMap<&'static str, Vec<DynEntry<MethodFn<T>>>>,
) {
    use haphe::dispatch::{DynCandidate, Resolution, resolve_dyn_candidate};

    for (name, entries) in dyn_methods {
        let arity = entries.first().map_or(0, |e| e.descriptor.params.len());
        let mut arg_types: Vec<TypeId> = Vec::with_capacity(1 + arity);
        arg_types.push(TypeId::of::<T>());
        arg_types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), arity));
        let entries = std::sync::Arc::new(entries);
        engine.register_raw_fn(
            name,
            arg_types,
            move |_ctx: NativeCallContext,
                  args: &mut [&mut Dynamic]|
                  -> Result<Dynamic, Box<EvalAltResult>> {
                let obj: T = args[0].clone().cast::<T>();
                let script_args = args_to_script(&mut args[1..])?;

                let candidates: Vec<DynCandidate<'_>> = entries
                    .iter()
                    .map(|e| DynCandidate {
                        type_args: e.type_args,
                        descriptor: e.descriptor,
                    })
                    .collect();
                let order = match resolve_dyn_candidate(&script_args, &candidates) {
                    Resolution::Ranked(idx) => {
                        let mut o: Vec<usize> = (0..entries.len()).collect();
                        o.swap(0, idx);
                        o
                    }
                    Resolution::TryCallOrder => (0..entries.len()).collect(),
                };

                let mut last_err = None;
                for idx in order {
                    let f = entries[idx].wrapper;
                    let cow = ScriptCow::Owned(obj.clone());
                    match f(cow, &script_args) {
                        Ok(v) => return Ok(script_to_dynamic(v)),
                        Err(ScriptCallError::Convert(e)) => last_err = Some(e),
                        Err(e @ ScriptCallError::Callee { .. }) => {
                            return Err(call_error(e));
                        }
                    }
                }

                Err(Box::new(EvalAltResult::ErrorRuntime(
                    format!(
                        "no dyn candidate for `{name}` accepted the arguments{}",
                        last_err.map_or_else(String::new, |e| format!(": {e}"))
                    )
                    .into(),
                    Position::NONE,
                )))
            },
        );
    }
}

#[cfg(feature = "generics")]
fn register_dyn_methods_mut<T: Clone + RhaiSendSync + 'static>(
    engine: &mut rhai::Engine,
    dyn_methods: HashMap<&'static str, Vec<DynEntry<MethodMutFn<T>>>>,
) {
    use haphe::dispatch::{DynCandidate, Resolution, resolve_dyn_candidate};

    for (name, entries) in dyn_methods {
        let arity = entries.first().map_or(0, |e| e.descriptor.params.len());
        let mut arg_types: Vec<TypeId> = Vec::with_capacity(1 + arity);
        arg_types.push(TypeId::of::<T>());
        arg_types.extend(std::iter::repeat_n(TypeId::of::<Dynamic>(), arity));
        let entries = std::sync::Arc::new(entries);
        engine.register_raw_fn(
            name,
            arg_types,
            move |_ctx: NativeCallContext,
                  args: &mut [&mut Dynamic]|
                  -> Result<Dynamic, Box<EvalAltResult>> {
                let mut obj: T = std::mem::take(args[0]).cast::<T>();
                let script_args = args_to_script(&mut args[1..])?;

                let candidates: Vec<DynCandidate<'_>> = entries
                    .iter()
                    .map(|e| DynCandidate {
                        type_args: e.type_args,
                        descriptor: e.descriptor,
                    })
                    .collect();
                let order = match resolve_dyn_candidate(&script_args, &candidates) {
                    Resolution::Ranked(idx) => {
                        let mut o: Vec<usize> = (0..entries.len()).collect();
                        o.swap(0, idx);
                        o
                    }
                    Resolution::TryCallOrder => (0..entries.len()).collect(),
                };

                let mut last_err = None;
                for idx in order {
                    let f = entries[idx].wrapper;
                    match f(&mut obj, &script_args) {
                        Ok(v) => {
                            *args[0] = Dynamic::from(obj);
                            return Ok(script_to_dynamic(v));
                        }
                        Err(ScriptCallError::Convert(e)) => last_err = Some(e),
                        Err(e @ ScriptCallError::Callee { .. }) => {
                            *args[0] = Dynamic::from(obj);
                            return Err(call_error(e));
                        }
                    }
                }

                *args[0] = Dynamic::from(obj);
                Err(Box::new(EvalAltResult::ErrorRuntime(
                    format!(
                        "no dyn candidate for `{name}` accepted the arguments{}",
                        last_err.map_or_else(String::new, |e| format!(": {e}"))
                    )
                    .into(),
                    Position::NONE,
                )))
            },
        );
    }
}

#[cfg(feature = "generics")]
fn register_dyn_associated(
    engine: &mut rhai::Engine,
    dyn_assoc: HashMap<&'static str, Vec<DynEntry<AssocFn>>>,
) {
    use haphe::dispatch::{DynCandidate, Resolution, resolve_dyn_candidate};

    for (name, entries) in dyn_assoc {
        let arity = entries.first().map_or(0, |e| e.descriptor.params.len());
        let arg_types: Vec<TypeId> =
            std::iter::repeat_n(TypeId::of::<Dynamic>(), arity).collect();
        let entries = std::sync::Arc::new(entries);
        engine.register_raw_fn(
            name,
            arg_types,
            move |_ctx: NativeCallContext,
                  args: &mut [&mut Dynamic]|
                  -> Result<Dynamic, Box<EvalAltResult>> {
                let script_args = args_to_script(args)?;

                let candidates: Vec<DynCandidate<'_>> = entries
                    .iter()
                    .map(|e| DynCandidate {
                        type_args: e.type_args,
                        descriptor: e.descriptor,
                    })
                    .collect();
                let order = match resolve_dyn_candidate(&script_args, &candidates) {
                    Resolution::Ranked(idx) => {
                        let mut o: Vec<usize> = (0..entries.len()).collect();
                        o.swap(0, idx);
                        o
                    }
                    Resolution::TryCallOrder => (0..entries.len()).collect(),
                };

                let mut last_err = None;
                for idx in order {
                    let f = entries[idx].wrapper;
                    match f(&script_args) {
                        Ok(v) => return Ok(script_to_dynamic(v)),
                        Err(ScriptCallError::Convert(e)) => last_err = Some(e),
                        Err(e @ ScriptCallError::Callee { .. }) => {
                            return Err(call_error(e));
                        }
                    }
                }

                Err(Box::new(EvalAltResult::ErrorRuntime(
                    format!(
                        "no dyn candidate for `{name}` accepted the arguments{}",
                        last_err.map_or_else(String::new, |e| format!(": {e}"))
                    )
                    .into(),
                    Position::NONE,
                )))
            },
        );
    }
}

// ---------------------------------------------------------------------------
// TypeBinder<T> impl
// ---------------------------------------------------------------------------

impl<T: Clone + RhaiSendSync + 'static> TypeBinder<T> for RhaiTypeBinder<T> {
    type Error = RhaiBindError;

    fn field<V: IntoScript + FromScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        getter: fn(&T) -> V,
        setter: Option<fn(&mut T, V)>,
    ) -> Result<(), Self::Error> {
        let get_fn: FieldGetErased<T> =
            Box::new(move |obj: &T| IntoScript::into_script(getter(obj)));
        let set_fn: Option<FieldSetErased<T>> = setter.map(|s| {
            Box::new(move |obj: &mut T, val: ScriptValue| {
                let v = V::from_script(val)?;
                s(obj, v);
                Ok(())
            }) as FieldSetErased<T>
        });
        self.fields.push(FieldEntry {
            name,
            getter: get_fn,
            setter: set_fn,
        });
        Ok(())
    }

    fn field_get<V: IntoScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        getter: fn(&T) -> V,
    ) -> Result<(), Self::Error> {
        let get_fn: FieldGetErased<T> =
            Box::new(move |obj: &T| IntoScript::into_script(getter(obj)));
        self.fields.push(FieldEntry {
            name,
            getter: get_fn,
            setter: None,
        });
        Ok(())
    }

    fn method(
        &mut self,
        name: &'static str,
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.methods.insert(name, f);
        Ok(())
    }

    fn method_mut(
        &mut self,
        name: &'static str,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.methods_mut.insert(name, f);
        Ok(())
    }

    fn method_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncMethod { name })
    }

    fn method_async_mut(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncMethod { name })
    }

    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.constructors.push((name, f));
        Ok(())
    }

    fn constructor_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(
            &'a [ScriptValue],
        ) -> haphe::ScriptCtorFuture<'a, T>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncMethod { name })
    }

    fn associated(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.associated.push((name, f));
        Ok(())
    }

    fn associated_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncFunction { name })
    }

    fn property_get(
        &mut self,
        name: &'static str,
        f: fn(&T) -> ScriptValue,
    ) -> Result<(), Self::Error> {
        self.property_getters.push((name, f));
        Ok(())
    }

    fn property_set(
        &mut self,
        name: &'static str,
        f: fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.property_setters.push((name, f));
        Ok(())
    }

    fn property_get_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(ScriptCow<'a, T>) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncProperty { name })
    }

    fn property_set_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(&'a mut T, ScriptValue) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncProperty { name })
    }

    fn meta_tostring(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error> {
        self.tostring = Some(f);
        Ok(())
    }

    fn meta_concat(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error> {
        self.concat = Some(f);
        Ok(())
    }

    fn meta_hash(&mut self, f: fn(&T) -> u64) -> Result<(), Self::Error> {
        self.hash_fn = Some(f);
        Ok(())
    }

    fn meta_debug(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error> {
        self.debug_fn = Some(f);
        Ok(())
    }

    fn meta_eq(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.eq_fn = Some(f);
        Ok(())
    }

    fn meta_lt(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.lt_fn = Some(f);
        Ok(())
    }

    fn meta_le(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.le_fn = Some(f);
        Ok(())
    }

    fn meta_unm(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error> {
        self.unm = Some(f);
        Ok(())
    }

    fn meta_bnot(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error> {
        self.bnot = Some(f);
        Ok(())
    }

    fn meta_arith_self(&mut self, op: &'static str, f: fn(T, T) -> T) -> Result<(), Self::Error> {
        self.arith_self.push((op, f));
        Ok(())
    }

    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        rhs: &'static haphe::TypeDescriptor<'static>,
        f: fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.arith_scalar.push((op, rhs, f));
        Ok(())
    }

    fn meta_iter(&mut self, f: fn(T) -> ScriptIter) -> Result<(), Self::Error> {
        self.iter_fn = Some(f);
        Ok(())
    }

    fn meta_len(&mut self, f: fn(T) -> usize) -> Result<(), Self::Error> {
        self.len_fn = Some(f);
        Ok(())
    }

    fn meta_call(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.call_fn = Some(f);
        Ok(())
    }

    fn meta_call_async(
        &mut self,
        _f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncCall)
    }

    fn meta_index(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.index_fn = Some(f);
        Ok(())
    }

    fn meta_newindex(
        &mut self,
        f: fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.newindex_fn = Some(f);
        Ok(())
    }

    #[cfg(feature = "generics")]
    fn method_generic(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        let mangled = mangle_generic_name(name, type_args);
        let leaked: &'static str = Box::leak(mangled.into_boxed_str());
        self.methods.insert(leaked, f);
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn method_generic(
        &mut self,
        name: &'static str,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::GenericFunction { name })
    }

    #[cfg(feature = "generics")]
    fn method_generic_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        let mangled = mangle_generic_name(name, type_args);
        let leaked: &'static str = Box::leak(mangled.into_boxed_str());
        self.methods_mut.insert(leaked, f);
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn method_generic_mut(
        &mut self,
        name: &'static str,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::GenericFunction { name })
    }

    #[cfg(feature = "generics")]
    fn associated_generic(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        let mangled = mangle_generic_name(name, type_args);
        let leaked: &'static str = Box::leak(mangled.into_boxed_str());
        self.associated.push((leaked, f));
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn associated_generic(
        &mut self,
        name: &'static str,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::GenericFunction { name })
    }

    #[cfg(feature = "generics")]
    fn method_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.dyn_methods
            .entry(descriptor.name)
            .or_default()
            .push(DynEntry {
                descriptor,
                type_args,
                self_params: self_inst.params,
                self_args: self_inst.args,
                wrapper: f,
            });
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn method_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _self_inst: haphe::SelfInstantiation,
        _f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedDynFunction {
            name: descriptor.name,
            reason: "enable the `generics` feature for dyn dispatch",
        })
    }

    #[cfg(feature = "generics")]
    fn method_dyn_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.dyn_methods_mut
            .entry(descriptor.name)
            .or_default()
            .push(DynEntry {
                descriptor,
                type_args,
                self_params: self_inst.params,
                self_args: self_inst.args,
                wrapper: f,
            });
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn method_dyn_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _self_inst: haphe::SelfInstantiation,
        _f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedDynFunction {
            name: descriptor.name,
            reason: "enable the `generics` feature for dyn dispatch",
        })
    }

    #[cfg(feature = "generics")]
    fn associated_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.dyn_associated
            .entry(descriptor.name)
            .or_default()
            .push(DynEntry {
                descriptor,
                type_args,
                self_params: self_inst.params,
                self_args: self_inst.args,
                wrapper: f,
            });
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn associated_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _self_inst: haphe::SelfInstantiation,
        _f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedDynFunction {
            name: descriptor.name,
            reason: "enable the `generics` feature for dyn dispatch",
        })
    }
}

// ---------------------------------------------------------------------------
// RhaiFnBinder
// ---------------------------------------------------------------------------

pub(crate) struct RhaiFnBinder {
    functions: Vec<NamedEntry<AssocFn>>,
    #[cfg(feature = "generics")]
    dyn_functions: HashMap<&'static str, Vec<DynEntry<AssocFn>>>,
}

impl RhaiFnBinder {
    pub(crate) fn new() -> Self {
        Self {
            functions: Vec::new(),
            #[cfg(feature = "generics")]
            dyn_functions: HashMap::new(),
        }
    }

    pub(crate) fn apply(self, engine: &mut rhai::Engine, arity: usize) {
        for (name, f) in self.functions {
            let arg_types: Vec<TypeId> =
                std::iter::repeat_n(TypeId::of::<Dynamic>(), arity).collect();
            engine.register_raw_fn(
                name,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    let script_args = args_to_script(args)?;
                    let result = f(&script_args).map_err(call_error)?;
                    Ok(script_to_dynamic(result))
                },
            );
        }
        #[cfg(feature = "generics")]
        for (name, entries) in self.dyn_functions {
            let fn_arity = entries.first().map_or(arity, |e| e.descriptor.params.len());
            let arg_types: Vec<TypeId> =
                std::iter::repeat_n(TypeId::of::<Dynamic>(), fn_arity).collect();
            let entries = std::sync::Arc::new(entries);
            engine.register_raw_fn(
                name,
                arg_types,
                move |_ctx: NativeCallContext,
                      args: &mut [&mut Dynamic]|
                      -> Result<Dynamic, Box<EvalAltResult>> {
                    use haphe::dispatch::{DynCandidate, Resolution, resolve_dyn_candidate};
                    let script_args = args_to_script(args)?;
                    let candidates: Vec<DynCandidate<'_>> = entries
                        .iter()
                        .map(|e| DynCandidate {
                            type_args: e.type_args,
                            descriptor: e.descriptor,
                        })
                        .collect();
                    let order = match resolve_dyn_candidate(&script_args, &candidates) {
                        Resolution::Ranked(idx) => {
                            let mut o: Vec<usize> = (0..entries.len()).collect();
                            o.swap(0, idx);
                            o
                        }
                        Resolution::TryCallOrder => (0..entries.len()).collect(),
                    };
                    let mut last_err = None;
                    for idx in order {
                        let f = entries[idx].wrapper;
                        match f(&script_args) {
                            Ok(v) => return Ok(script_to_dynamic(v)),
                            Err(ScriptCallError::Convert(e)) => last_err = Some(e),
                            Err(e @ ScriptCallError::Callee { .. }) => {
                                return Err(call_error(e));
                            }
                        }
                    }
                    Err(Box::new(EvalAltResult::ErrorRuntime(
                        format!(
                            "no dyn candidate for `{name}` accepted the arguments{}",
                            last_err.map_or_else(String::new, |e| format!(": {e}"))
                        )
                        .into(),
                        Position::NONE,
                    )))
                },
            );
        }
    }
}

impl FnBinder for RhaiFnBinder {
    type Error = RhaiBindError;

    fn function(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        if type_args.is_empty() {
            self.functions.push((name, f));
        } else {
            let mangled = mangle_generic_name(name, type_args);
            let leaked: &'static str = Box::leak(mangled.into_boxed_str());
            self.functions.push((leaked, f));
        }
        Ok(())
    }

    fn function_async(
        &mut self,
        name: &'static str,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _f: for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedAsyncFunction { name })
    }

    #[cfg(feature = "generics")]
    fn function_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.dyn_functions
            .entry(descriptor.name)
            .or_default()
            .push(DynEntry {
                descriptor,
                type_args,
                self_params: &[],
                self_args: &[],
                wrapper: f,
            });
        Ok(())
    }

    #[cfg(not(feature = "generics"))]
    fn function_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        _type_args: &'static [haphe::TypeDescriptor<'static>],
        _f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), Self::Error> {
        Err(RhaiBindError::UnsupportedDynFunction {
            name: descriptor.name,
            reason: "enable the `generics` feature for dyn dispatch",
        })
    }
}
