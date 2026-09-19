//! `TypeBinder<T>` implementation for mlua.
//!
//! Collects field/method/metamethod registrations from `ScriptBind::bind`,
//! then applies them to a Lua state by creating UserData + FromLua impls
//! at registration time.

use std::sync::Arc;

use haphe::bridge::{FromScript, IntoScript, ScriptValue};
use haphe::{FnBinder, ScriptCallFuture, ScriptCow, ScriptIter, TypeBinder};
use mlua::{Lua, MetaMethod, UserDataFields, UserDataMethods};

use crate::LuaBindError;

/// How the stepper presents core's plain value stream to Lua's generic-for.
///
/// Core's `ScriptIter` yields single values; pairing is this backend's
/// decision, made statically from the type's declared iterator item type
/// (never guessed from runtime shapes).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum IterPairing {
    /// 1-based `(i, item)` control pairs.
    #[default]
    Enumerate,
    /// The declared item is a 2-tuple: each yielded 2-element list is
    /// unpacked to `(k, v)`.
    KeyValue,
}

/// Builds the per-iteration stepper closure Lua's generic-for drives: it
/// owns the iterator, pulls one item per call, and returns nil-terminated
/// control pairs.
fn make_stepper(lua: &Lua, iter: ScriptIter, pairing: IterPairing) -> mlua::Result<mlua::Function> {
    // Without `send`, the closure holds the lazy iterator and advances it
    // per step — lazy end to end.
    #[cfg(not(feature = "send"))]
    let state = std::cell::RefCell::new(iter.enumerate());
    // Under `send`, mlua closures must be `Send` while `ScriptIter` is
    // deliberately not: this backend's send-mode policy is to buffer the
    // iteration up front.
    #[cfg(feature = "send")]
    let state = std::sync::Mutex::new(iter.collect::<Vec<_>>().into_iter().enumerate());

    lua.create_function(move |lua, _: mlua::MultiValue| {
        #[cfg(not(feature = "send"))]
        let mut it = state.borrow_mut();
        #[cfg(feature = "send")]
        let mut it = state.lock().expect("iteration stepper poisoned");
        match it.next() {
            None => Ok((mlua::Value::Nil, mlua::Value::Nil)),
            Some((i, item)) => match pairing {
                IterPairing::Enumerate => Ok((
                    mlua::Value::Integer((i as i64 + 1) as mlua::Integer),
                    script_to_lua(lua, item)?,
                )),
                IterPairing::KeyValue => match item {
                    ScriptValue::List(mut kv) if kv.len() == 2 => {
                        let v = kv.pop().expect("len checked");
                        let k = kv.pop().expect("len checked");
                        Ok((script_to_lua(lua, k)?, script_to_lua(lua, v)?))
                    }
                    other => Err(mlua::Error::runtime(format!(
                        "declared (key, value) iterator item did not cross as a \
                         2-element list (got {other:?})"
                    ))),
                },
            },
        }
    })
}

/// Rejects operators the configured Lua version cannot represent, per the
/// bridge contract (never silently drop a registration).
///
/// The bitwise metamethods (`__band`, `__bor`, `__bxor`, `__bnot`, `__shl`,
/// `__shr`) exist only on Lua 5.3+ — not on 5.1/5.2/LuaJIT, and not on Luau,
/// which has no bitwise operators. `__pow` exists everywhere.
fn check_op_supported(op: &'static str) -> Result<(), LuaBindError> {
    #[cfg(not(any(feature = "lua55", feature = "lua54", feature = "lua53")))]
    if matches!(op, "bitand" | "bitor" | "bitxor" | "bnot" | "shl" | "shr") {
        return Err(LuaBindError::UnsupportedOperator { op });
    }
    // `__idiv` (`//`) exists on Lua 5.3+ and Luau (mlua gates the
    // MetaMethod accordingly).
    #[cfg(not(any(
        feature = "lua55",
        feature = "lua54",
        feature = "lua53",
        feature = "luau"
    )))]
    if op == "idiv" {
        return Err(LuaBindError::UnsupportedOperator { op });
    }
    let _ = op;
    Ok(())
}

/// Converts a [`ScriptValue`] to an [`mlua::Value`].
pub(crate) fn script_to_lua(lua: &Lua, v: ScriptValue) -> mlua::Result<mlua::Value> {
    match v {
        ScriptValue::Unit => Ok(mlua::Value::Nil),
        ScriptValue::Bool(b) => Ok(mlua::Value::Boolean(b)),
        ScriptValue::I64(n) => Ok(mlua::Value::Integer(n)),
        ScriptValue::F64(n) => Ok(mlua::Value::Number(n)),
        ScriptValue::String(s) => Ok(mlua::Value::String(lua.create_string(&s)?)),
        ScriptValue::Bytes(b) => Ok(mlua::Value::String(lua.create_string(&b)?)),
        ScriptValue::Char(c) => {
            let mut buf = [0u8; 4];
            Ok(mlua::Value::String(
                lua.create_string(c.encode_utf8(&mut buf) as &str)?,
            ))
        }
        ScriptValue::List(items) => {
            let table = lua.create_table()?;
            for (i, item) in items.into_iter().enumerate() {
                table.set(i + 1, script_to_lua(lua, item)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
        ScriptValue::Map(pairs) => {
            let table = lua.create_table()?;
            for (k, v) in pairs {
                table.set(k, script_to_lua(lua, v)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
        ScriptValue::Optional(None) => Ok(mlua::Value::Nil),
        ScriptValue::Optional(Some(inner)) => script_to_lua(lua, *inner),
        ScriptValue::UserData(ud) => lua.create_any_userdata(ud).map(mlua::Value::UserData),
        // Unit-enum cases: NUMERIC enums (a Rust `#[repr]` integer type)
        // carry their discriminant and cross as Lua integers; string enums
        // travel as their case-name string — the declared exposed names,
        // passed through verbatim and matched exactly. A case WITH a payload
        // crosses as a case table: `{ case = "Name", <values at [1..n]> }`
        // (tuple values positionally; struct fields in declaration order).
        ScriptValue::Enum {
            case,
            discriminant,
            payload,
        } => {
            if payload.is_empty() {
                match discriminant {
                    Some(value) => Ok(mlua::Value::Integer(value)),
                    None => Ok(mlua::Value::String(lua.create_string(&case)?)),
                }
            } else {
                let table = lua.create_table()?;
                table.raw_set("case", lua.create_string(&case)?)?;
                for (i, item) in payload.into_iter().enumerate() {
                    table.raw_set(i + 1, script_to_lua(lua, item)?)?;
                }
                Ok(mlua::Value::Table(table))
            }
        }
        _ => Err(mlua::Error::runtime("unsupported ScriptValue variant")),
    }
}

/// Converts an [`mlua::Value`] to a [`ScriptValue`].
pub(crate) fn lua_to_script(v: &mlua::Value) -> mlua::Result<ScriptValue> {
    match v {
        mlua::Value::Nil => Ok(ScriptValue::Unit),
        mlua::Value::Boolean(b) => Ok(ScriptValue::Bool(*b)),
        mlua::Value::Integer(n) => Ok(ScriptValue::I64(*n)),
        mlua::Value::Number(n) => Ok(ScriptValue::F64(*n)),
        mlua::Value::String(s) => Ok(ScriptValue::String(s.to_str()?.to_owned())),
        mlua::Value::Table(t) => lua_table_to_script(t),
        mlua::Value::UserData(ud) => {
            if let Ok(opaque) = ud.borrow::<haphe::OpaqueUserData>() {
                Ok(ScriptValue::UserData(opaque.clone()))
            } else {
                Err(mlua::Error::runtime(
                    "cannot convert foreign userdata to script value",
                ))
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "cannot convert {} to script value",
            other.type_name()
        ))),
    }
}

/// Recognizes the enum case-table shape — a `case` string field plus ONLY
/// the contiguous positional values `[1..n]` — and converts it to
/// [`ScriptValue::Enum`]. Any other key means an ordinary table (`None`).
fn enum_table_to_script(t: &mlua::Table) -> mlua::Result<Option<ScriptValue>> {
    let mlua::Value::String(case) = t.raw_get::<mlua::Value>("case")? else {
        return Ok(None);
    };
    let len = t.raw_len();
    for pair in t.pairs::<mlua::Value, mlua::Value>() {
        let (key, _) = pair?;
        let positional = matches!(key, mlua::Value::Integer(i) if i >= 1 && (i as usize) <= len);
        let case_key = matches!(&key, mlua::Value::String(s) if s.as_bytes().as_ref() == b"case");
        if !positional && !case_key {
            return Ok(None);
        }
    }
    let mut payload = Vec::with_capacity(len);
    for i in 1..=len {
        let val: mlua::Value = t.raw_get(i)?;
        payload.push(lua_to_script(&val)?);
    }
    Ok(Some(ScriptValue::Enum {
        case: case.to_str()?.to_owned(),
        discriminant: None,
        payload,
    }))
}

fn lua_table_to_script(t: &mlua::Table) -> mlua::Result<ScriptValue> {
    if let Some(v) = enum_table_to_script(t)? {
        return Ok(v);
    }
    let len = t.raw_len();
    if len > 0 {
        let mut list = Vec::with_capacity(len);
        for i in 1..=len {
            let val: mlua::Value = t.get(i)?;
            list.push(lua_to_script(&val)?);
        }
        Ok(ScriptValue::List(list))
    } else {
        let mut pairs = Vec::new();
        for pair in t.pairs::<mlua::Value, mlua::Value>() {
            let (k, v) = pair?;
            let key = match &k {
                mlua::Value::String(s) => s.to_str()?.to_owned(),
                _ => return Err(mlua::Error::runtime("map keys must be strings")),
            };
            pairs.push((key, lua_to_script(&v)?));
        }
        Ok(ScriptValue::Map(pairs))
    }
}

// ---------------------------------------------------------------------------
// Collected registrations — type-erased closures
// ---------------------------------------------------------------------------

type FieldGetFn<T> = Arc<dyn Fn(&T, &Lua) -> mlua::Result<mlua::Value> + Send + Sync + 'static>;
type FieldSetFn<T> =
    Arc<dyn Fn(&mut T, mlua::Value, &Lua) -> mlua::Result<()> + Send + Sync + 'static>;

// Method/constructor fn pointers using ScriptValue — no generics needed.
type ScriptMethodRef<T> = fn(&T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>;
type CowMethodFn<T> =
    for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
type AsyncCowFn<T> = for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>;
type AsyncMutFn<T> = for<'a> fn(&'a mut T, &'a [ScriptValue]) -> ScriptCallFuture<'a>;
type ScriptMethodMut<T> = fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
type ScriptCtorFn<T> = fn(&[ScriptValue]) -> Result<T, haphe::ScriptCallError>;
type PropGetFn<T> = fn(&T) -> ScriptValue;
type PropSetFn<T> = fn(&mut T, ScriptValue) -> Result<(), haphe::ScriptConvertError>;
#[cfg(all(feature = "async", not(feature = "send")))]
type AsyncCtorFn<T> = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>;
type ScriptArithSelf<T> = fn(T, T) -> T;
type ScriptArithScalar<T> = fn(T, &[ScriptValue]) -> Result<T, haphe::ScriptConvertError>;
type ScriptNewIndex<T> = fn(&mut T, &[ScriptValue]) -> Result<(), haphe::ScriptConvertError>;
/// One scalar overload: the declared rhs type plus its monomorphized wrapper.
type ScalarOverload<T> = (
    &'static haphe::TypeDescriptor<'static>,
    ScriptArithScalar<T>,
);

/// Mangled per-monomorph name for a STATICALLY dispatched generic method:
/// the exposed name, a double underscore, then each type argument's
/// identifier-safe rendering joined by single underscores —
/// `first_of__i64`, `first_of__string`, `pick__i64_bool`. Lua dispatches by
/// name only, so every declared instantiation gets its own metatable entry
/// under this name (`obj:first_of__i64(...)`); decl stubs use the same
/// spelling.
pub(crate) fn mangle_generic_name(name: &str, type_args: &[haphe::TypeDescriptor<'_>]) -> String {
    let args: Vec<String> = type_args.iter().map(mangle_ty).collect();
    format!("{name}__{}", args.join("_"))
}

/// Identifier-safe rendering of one concrete type argument for
/// [`mangle_generic_name`]. Lowercase, `[a-z0-9_]` only.
fn mangle_ty(ty: &haphe::TypeDescriptor<'_>) -> String {
    use haphe::TypeDescriptor as T;
    match crate::peel_borrowed(ty) {
        T::Primitive(p) => format!("{p:?}").to_lowercase(),
        T::String => "string".into(),
        T::Bytes => "bytes".into(),
        T::Unit => "unit".into(),
        T::Option(inner) => format!("opt_{}", mangle_ty(inner)),
        T::List(inner) | T::Array(inner, _) => format!("list_{}", mangle_ty(inner)),
        T::Map(k, v) => format!("map_{}_{}", mangle_ty(k), mangle_ty(v)),
        T::Tuple(elems) => {
            let parts: Vec<String> = elems.iter().map(mangle_ty).collect();
            format!("tuple_{}", parts.join("_"))
        }
        T::Ref(id) | T::Instance { id, .. } => id
            .as_str()
            .rsplit("::")
            .next()
            .unwrap_or("ref")
            .to_lowercase(),
        other => format!("{other:?}")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect(),
    }
}

struct FieldReg<T: 'static> {
    name: &'static str,
    getter: FieldGetFn<T>,
    setter: Option<FieldSetFn<T>>,
}

struct MethodReg<T: 'static> {
    name: &'static str,
    f: CowMethodFn<T>,
}

struct MutMethodReg<T: 'static> {
    name: &'static str,
    f: ScriptMethodMut<T>,
}

struct CtorReg<T: 'static> {
    name: &'static str,
    f: ScriptCtorFn<T>,
}

enum ArithEntry<T: 'static> {
    SelfOp(&'static str, ScriptArithSelf<T>),
    Scalar(&'static str, ScalarOverload<T>),
}

/// Lua's overload-ranking policy: whether a Lua value's natural shape is an
/// exact match for a declared scalar operand type. Integers are *not* exact
/// matches for float operands (although they convert), so an integer rhs
/// prefers an integer overload regardless of declaration order.
fn rhs_matches_exactly(rhs: &haphe::TypeDescriptor<'_>, value: &mlua::Value) -> bool {
    use haphe::{PrimitiveType as P, TypeDescriptor as Td};
    match value {
        mlua::Value::Integer(_) => matches!(
            rhs,
            Td::Primitive(P::I8 | P::I16 | P::I32 | P::I64 | P::U8 | P::U16 | P::U32 | P::U64)
        ),
        mlua::Value::Number(_) => matches!(rhs, Td::Primitive(P::F32 | P::F64)),
        mlua::Value::Boolean(_) => matches!(rhs, Td::Primitive(P::Bool)),
        mlua::Value::String(_) => matches!(rhs, Td::String | Td::Primitive(P::Char)),
        _ => false,
    }
}

/// Collects registrations from `ScriptBind::bind`, then applies them to Lua.
pub(crate) struct LuaTypeBinder<T: 'static> {
    fields: Vec<FieldReg<T>>,
    methods: Vec<MethodReg<T>>,
    mut_methods: Vec<MutMethodReg<T>>,
    constructors: Vec<CtorReg<T>>,
    tostring: Option<fn(&T) -> String>,
    concat: Option<fn(&T) -> String>,
    eq: Option<fn(&T, &T) -> bool>,
    lt: Option<fn(&T, &T) -> bool>,
    le: Option<fn(&T, &T) -> bool>,
    unm: Option<fn(&T) -> T>,
    bnot: Option<fn(&T) -> T>,
    hash: Option<fn(&T) -> u64>,
    debug: Option<fn(&T) -> String>,
    ariths: Vec<ArithEntry<T>>,
    iter: Option<fn(T) -> ScriptIter>,
    len: Option<fn(T) -> usize>,
    index: Option<ScriptMethodRef<T>>,
    newindex: Option<ScriptNewIndex<T>>,
    call: Option<ScriptMethodRef<T>>,
    call_async: Option<AsyncCowFn<T>>,
    prop_gets: Vec<(&'static str, PropGetFn<T>)>,
    prop_sets: Vec<(&'static str, PropSetFn<T>)>,
    #[cfg(all(feature = "async", not(feature = "send")))]
    async_ctors: Vec<(&'static str, AsyncCtorFn<T>)>,
    #[cfg(all(feature = "async", not(feature = "send")))]
    async_methods: Vec<(&'static str, AsyncCowFn<T>)>,
    #[cfg(all(feature = "async", not(feature = "send")))]
    async_mut_methods: Vec<(&'static str, AsyncMutFn<T>)>,
    #[cfg(feature = "generics")]
    dyn_methods: Vec<(&'static str, Vec<DynFn<CowMethodFn<T>>>)>,
    #[cfg(feature = "generics")]
    dyn_mut_methods: Vec<(&'static str, Vec<DynFn<ScriptMethodMut<T>>>)>,
    /// Static generic monomorphs, keyed by mangled per-instantiation name.
    #[cfg(feature = "generics")]
    generic_methods: Vec<(String, CowMethodFn<T>)>,
    #[cfg(feature = "generics")]
    generic_mut_methods: Vec<(String, ScriptMethodMut<T>)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    generic_async_methods: Vec<(String, AsyncCowFn<T>)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    generic_async_mut_methods: Vec<(String, AsyncMutFn<T>)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    dyn_async_methods: Vec<(&'static str, Vec<DynFn<AsyncCowFn<T>>>)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    dyn_async_mut_methods: Vec<(&'static str, Vec<DynFn<AsyncMutFn<T>>>)>,
    pairing: IterPairing,
    idiv_fallback: bool,
}

impl<T: 'static + Clone + mlua::MaybeSend + mlua::MaybeSync> LuaTypeBinder<T> {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            methods: Vec::new(),
            mut_methods: Vec::new(),
            constructors: Vec::new(),
            tostring: None,
            concat: None,
            eq: None,
            lt: None,
            le: None,
            unm: None,
            bnot: None,
            hash: None,
            debug: None,
            ariths: Vec::new(),
            iter: None,
            len: None,
            index: None,
            newindex: None,
            call: None,
            call_async: None,
            prop_gets: Vec::new(),
            prop_sets: Vec::new(),
            #[cfg(all(feature = "async", not(feature = "send")))]
            async_ctors: Vec::new(),
            #[cfg(all(feature = "async", not(feature = "send")))]
            async_methods: Vec::new(),
            #[cfg(all(feature = "async", not(feature = "send")))]
            async_mut_methods: Vec::new(),
            #[cfg(feature = "generics")]
            dyn_methods: Vec::new(),
            #[cfg(feature = "generics")]
            dyn_mut_methods: Vec::new(),
            #[cfg(feature = "generics")]
            generic_methods: Vec::new(),
            #[cfg(feature = "generics")]
            generic_mut_methods: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            generic_async_methods: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            generic_async_mut_methods: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            dyn_async_methods: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            dyn_async_mut_methods: Vec::new(),
            pairing: IterPairing::default(),
            idiv_fallback: false,
        }
    }

    /// Sets the iteration pairing, decided from the type's declared iterator
    /// item type.
    pub fn set_pairing(&mut self, pairing: IterPairing) {
        self.pairing = pairing;
    }

    /// Enables the integer-`Div` fallback: the type's `__div` registration
    /// is also installed as `__idiv`, decided statically from the descriptor
    /// (integer-typed `Div`, no explicit `IDiv`). The fallback carries
    /// Rust's TRUNCATING division semantics, not Lua's floor semantics —
    /// declare `traits(IDiv)` for floor behavior on negative operands.
    pub fn set_idiv_fallback(&mut self, fallback: bool) {
        self.idiv_fallback = fallback;
    }

    /// Apply all collected registrations to the Lua state.
    pub fn register(self, lua: &Lua, type_table: &mlua::Table) -> Result<(), LuaBindError> {
        // Some traits register implicit portable methods (`iter` on
        // iterable types, `hash` for `Hash`, `debug` for `Debug`); a
        // user-declared method with one of those names would silently
        // shadow it.
        #[allow(unused_mut)]
        let mut dyn_method_names: Vec<&'static str> = Vec::new();
        #[cfg(feature = "generics")]
        {
            dyn_method_names.extend(self.dyn_methods.iter().map(|(n, _)| *n));
            dyn_method_names.extend(self.dyn_mut_methods.iter().map(|(n, _)| *n));
        }
        // Static generic monomorphs land under mangled names; a collision
        // (same-named mangles, or a mangle shadowing a declared method)
        // would silently overwrite the earlier metatable entry.
        #[cfg(feature = "generics")]
        {
            let mut seen: Vec<&str> = self
                .methods
                .iter()
                .map(|m| m.name)
                .chain(self.mut_methods.iter().map(|m| m.name))
                .chain(dyn_method_names.iter().copied())
                .collect();
            let generic_names = self
                .generic_methods
                .iter()
                .map(|(n, _)| n)
                .chain(self.generic_mut_methods.iter().map(|(n, _)| n));
            #[cfg(all(feature = "async", not(feature = "send")))]
            let generic_names = generic_names
                .chain(self.generic_async_methods.iter().map(|(n, _)| n))
                .chain(self.generic_async_mut_methods.iter().map(|(n, _)| n));
            for name in generic_names {
                if seen.contains(&name.as_str()) {
                    return Err(LuaBindError::DuplicateMethod { name: name.clone() });
                }
                seen.push(name.as_str());
            }
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            dyn_method_names.extend(self.dyn_async_methods.iter().map(|(n, _)| *n));
            dyn_method_names.extend(self.dyn_async_mut_methods.iter().map(|(n, _)| *n));
        }
        for (reserved, taken) in [
            ("iter", self.iter.is_some()),
            ("hash", self.hash.is_some()),
            ("debug", self.debug.is_some()),
        ] {
            if taken
                && self
                    .methods
                    .iter()
                    .map(|m| m.name)
                    .chain(self.mut_methods.iter().map(|m| m.name))
                    .chain(dyn_method_names.iter().copied())
                    .any(|name| name == reserved)
            {
                return Err(LuaBindError::ReservedMethod { name: reserved });
            }
        }
        // Constructors land on the type table by name; a duplicate (e.g. a
        // user constructor named `default` next to the implicit one from
        // `traits(Default)`) would silently overwrite. Sync and async
        // constructors share the namespace.
        {
            let mut seen: Vec<&'static str> = Vec::new();
            let names = self.constructors.iter().map(|c| c.name);
            #[cfg(all(feature = "async", not(feature = "send")))]
            let names = names.chain(self.async_ctors.iter().map(|(n, _)| *n));
            for name in names {
                if seen.contains(&name) {
                    return Err(LuaBindError::DuplicateConstructor { name });
                }
                seen.push(name);
            }
        }
        // Computed properties share the field namespace; mlua would let the
        // later registration silently shadow the earlier one.
        for (name, _) in &self.prop_gets {
            if self.fields.iter().any(|f| f.name == *name) {
                return Err(LuaBindError::DuplicateField { name });
            }
        }
        // Lua has exactly one `__call` metamethod: a type declaring both
        // Call and AsyncCall is ambiguous — refuse rather than pick.
        if self.call.is_some() && self.call_async.is_some() {
            return Err(LuaBindError::AmbiguousCall);
        }
        // Constructors → Lua functions on the type table.
        for ctor in &self.constructors {
            let f = ctor.f;
            let lua_fn = lua.create_function(move |lua, args: mlua::MultiValue| {
                let sv_args: Vec<ScriptValue> = args
                    .into_vec()
                    .iter()
                    .map(lua_to_script)
                    .collect::<mlua::Result<_>>()?;
                let value = f(&sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                lua.create_any_userdata(value)
            })?;
            type_table.set(ctor.name, lua_fn)?;
        }
        // Async constructors: same surface, awaited body.
        #[cfg(all(feature = "async", not(feature = "send")))]
        for (name, f) in &self.async_ctors {
            let f = *f;
            let lua_fn =
                lua.create_async_function(move |lua, args: mlua::MultiValue| async move {
                    let sv_args: Vec<ScriptValue> = args
                        .into_vec()
                        .iter()
                        .map(lua_to_script)
                        .collect::<mlua::Result<_>>()?;
                    let value = f(&sv_args)
                        .await
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    lua.create_any_userdata(value)
                })?;
            type_table.set(*name, lua_fn)?;
        }

        // UserData registration.
        lua.register_userdata_type::<T>(move |reg| {
            // Fields.
            for field in &self.fields {
                let getter = field.getter.clone();
                reg.add_field_function_get(field.name, move |lua, ud| {
                    getter(&*ud.borrow::<T>()?, lua)
                });
                if let Some(setter) = &field.setter {
                    let setter = setter.clone();
                    reg.add_field_function_set(field.name, move |lua, ud, val| {
                        setter(&mut *ud.borrow_mut::<T>()?, val, lua)
                    });
                }
            }

            // Computed properties: the natural Lua surface is a field
            // (`obj.level` reads, `obj.level = v` writes), same as plain
            // fields but through the property fn pointers.
            for (name, get) in &self.prop_gets {
                let get = *get;
                reg.add_field_function_get(*name, move |lua, ud| {
                    script_to_lua(lua, get(&*ud.borrow::<T>()?))
                });
            }
            for (name, set) in &self.prop_sets {
                let set = *set;
                reg.add_field_function_set(*name, move |lua, ud, val| {
                    let sv = lua_to_script(&val)?;
                    let _ = lua;
                    set(&mut *ud.borrow_mut::<T>()?, sv)
                        .map_err(|e| mlua::Error::runtime(e.to_string()))
                });
            }

            // Methods (&self).
            for method in &self.methods {
                let f = method.f;
                reg.add_function(method.name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let this = ud.borrow::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    // Borrowed carrier while the guard is held: zero clones
                    // for `&self`; a consuming method clones inside the
                    // wrapper via `into_owned`.
                    let result = f(ScriptCow::Borrowed(&this), &sv_args)
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }

            // Async methods: value acquisition is CLONE (like the async
            // call metamethod), so the future owns its value and borrows
            // nothing. NOTE the consequence for `&mut self` receivers: the
            // CLONE mutates, not the bound userdata. mlua's plain async
            // methods exist on every Lua version (unlike async metamethods)
            // but still require its `async` feature and, under `send`,
            // `Send` futures — those combos are rejected in `method_async`.
            #[cfg(all(feature = "async", not(feature = "send")))]
            for (name, f) in &self.async_methods {
                let f = *f;
                reg.add_async_method(
                    *name,
                    move |lua, this: mlua::UserDataRef<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        async move {
                            // The async block owns the guard; the wrapper's
                            // future borrows it — zero-clone dispatch.
                            let sv_args = sv_args?;
                            let out = f(ScriptCow::Borrowed(&this), &sv_args)
                                .await
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            script_to_lua(&lua, out)
                        }
                    },
                );
            }
            // `&mut self` async methods: the mutable guard is held across
            // awaits, so the wrapper's future mutates the bound userdata in
            // place — script-visible mutation persists.
            #[cfg(all(feature = "async", not(feature = "send")))]
            for (name, f) in &self.async_mut_methods {
                let f = *f;
                reg.add_async_method_mut(
                    *name,
                    move |lua, this: mlua::UserDataRefMut<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        async move {
                            let mut this = this;
                            let sv_args = sv_args?;
                            let out = f(&mut this, &sv_args)
                                .await
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            script_to_lua(&lua, out)
                        }
                    },
                );
            }

            // Methods (&mut self).
            for method in &self.mut_methods {
                let f = method.f;
                reg.add_function(method.name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let mut this = ud.borrow_mut::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let result =
                        f(&mut *this, &sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }

            // Static generic monomorphs: one plain method per declared
            // instantiation under its mangled name — dispatch is exact (no
            // scan, no fall-through; a wrong-typed argument is a conversion
            // error like any non-generic method).
            #[cfg(feature = "generics")]
            for (name, f) in &self.generic_methods {
                let f = *f;
                reg.add_function(name.as_str(), move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let this = ud.borrow::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let result = f(ScriptCow::Borrowed(&this), &sv_args)
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }
            #[cfg(feature = "generics")]
            for (name, f) in &self.generic_mut_methods {
                let f = *f;
                reg.add_function(name.as_str(), move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let mut this = ud.borrow_mut::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let result =
                        f(&mut *this, &sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }

            // Async static generic monomorphs: same per-instantiation
            // mangled entries, awaited bodies — the async block owns the
            // guard, the wrapper's future borrows it (mutably for
            // `&mut self`, so mutation persists).
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            for (name, f) in &self.generic_async_methods {
                let f = *f;
                reg.add_async_method(
                    name.as_str(),
                    move |lua, this: mlua::UserDataRef<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        async move {
                            let sv_args = sv_args?;
                            let out = f(ScriptCow::Borrowed(&this), &sv_args)
                                .await
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            script_to_lua(&lua, out)
                        }
                    },
                );
            }
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            for (name, f) in &self.generic_async_mut_methods {
                let f = *f;
                reg.add_async_method_mut(
                    name.as_str(),
                    move |lua, this: mlua::UserDataRefMut<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        async move {
                            let mut this = this;
                            let sv_args = sv_args?;
                            let out = f(&mut this, &sv_args)
                                .await
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            script_to_lua(&lua, out)
                        }
                    },
                );
            }

            // Dyn generic methods: ONE Lua method per name scans its
            // monomorph candidates at call time — core's shared resolver
            // ranks, a rejected conversion falls through in declaration
            // order (same machinery as dyn free functions), and a Host
            // error — the matched candidate's Rust impl failed — propagates
            // immediately, never retried. Candidate tables are built here,
            // once.
            #[cfg(feature = "generics")]
            for (name, candidates) in &self.dyn_methods {
                let name = *name;
                let candidates = candidates.clone();
                let scan = DynMethodScan::build(&candidates);
                let signatures = dyn_signatures(&candidates);
                reg.add_function(name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    if v.is_empty() {
                        return Err(mlua::Error::runtime(format!(
                            "{name} takes a receiver (use `:`)"
                        )));
                    }
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let this = ud.borrow::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let mut last_err = None;
                    for i in scan.call_order(&sv_args) {
                        match (candidates[i].wrapper)(ScriptCow::Borrowed(&this), &sv_args) {
                            Ok(out) => return script_to_lua(lua, out),
                            Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                            Err(host) => {
                                return Err(mlua::Error::runtime(host.to_string()));
                            }
                        }
                    }
                    Err(dyn_no_match_error(name, &signatures, last_err))
                });
            }
            #[cfg(feature = "generics")]
            for (name, candidates) in &self.dyn_mut_methods {
                let name = *name;
                let candidates = candidates.clone();
                let scan = DynMethodScan::build(&candidates);
                let signatures = dyn_signatures(&candidates);
                reg.add_function(name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    if v.is_empty() {
                        return Err(mlua::Error::runtime(format!(
                            "{name} takes a receiver (use `:`)"
                        )));
                    }
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let mut this = ud.borrow_mut::<T>()?;
                    let sv_args: Vec<ScriptValue> =
                        v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let mut last_err = None;
                    for i in scan.call_order(&sv_args) {
                        match (candidates[i].wrapper)(&mut this, &sv_args) {
                            Ok(out) => return script_to_lua(lua, out),
                            Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                            Err(host) => {
                                return Err(mlua::Error::runtime(host.to_string()));
                            }
                        }
                    }
                    Err(dyn_no_match_error(name, &signatures, last_err))
                });
            }
            // Async dyn methods: the async block owns the guard; the picked
            // wrapper's future borrows it — same acquisition policy as
            // plain async methods.
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            for (name, candidates) in &self.dyn_async_methods {
                let name = *name;
                let candidates = std::sync::Arc::new(candidates.clone());
                let scan = std::sync::Arc::new(DynMethodScan::build(&candidates));
                let signatures = dyn_signatures(&candidates);
                reg.add_async_method(
                    name,
                    move |lua, this: mlua::UserDataRef<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        let candidates = candidates.clone();
                        let scan = scan.clone();
                        let signatures = signatures.clone();
                        async move {
                            let sv_args = sv_args?;
                            let mut last_err = None;
                            for i in scan.call_order(&sv_args) {
                                match (candidates[i].wrapper)(ScriptCow::Borrowed(&this), &sv_args)
                                    .await
                                {
                                    Ok(out) => return script_to_lua(&lua, out),
                                    Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                                    Err(host) => {
                                        return Err(mlua::Error::runtime(host.to_string()));
                                    }
                                }
                            }
                            Err(dyn_no_match_error(name, &signatures, last_err))
                        }
                    },
                );
            }
            // Async `&mut self` dyn methods: the mutable guard is held
            // across awaits, so the picked wrapper mutates in place.
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            for (name, candidates) in &self.dyn_async_mut_methods {
                let name = *name;
                let candidates = std::sync::Arc::new(candidates.clone());
                let scan = std::sync::Arc::new(DynMethodScan::build(&candidates));
                let signatures = dyn_signatures(&candidates);
                reg.add_async_method_mut(
                    name,
                    move |lua, this: mlua::UserDataRefMut<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        let candidates = candidates.clone();
                        let scan = scan.clone();
                        let signatures = signatures.clone();
                        async move {
                            let mut this = this;
                            let sv_args = sv_args?;
                            let mut last_err = None;
                            for i in scan.call_order(&sv_args) {
                                match (candidates[i].wrapper)(&mut this, &sv_args).await {
                                    Ok(out) => return script_to_lua(&lua, out),
                                    Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                                    Err(host) => {
                                        return Err(mlua::Error::runtime(host.to_string()));
                                    }
                                }
                            }
                            Err(dyn_no_match_error(name, &signatures, last_err))
                        }
                    },
                );
            }

            // Metamethods.
            if let Some(f) = self.tostring {
                reg.add_meta_method(MetaMethod::ToString, move |_, this, ()| Ok(f(this)));
            }
            if let Some(f) = self.concat {
                // Strict string-like rule: each operand must be this type,
                // a string, or a number; anything else errors in Lua's own
                // style.
                let fragment = move |lua: &Lua, v: &mlua::Value| -> mlua::Result<String> {
                    if let mlua::Value::UserData(ud) = v
                        && let Ok(this) = ud.borrow::<T>()
                    {
                        return Ok(f(&this));
                    }
                    match v {
                        mlua::Value::String(s) => Ok(s.to_str()?.to_owned()),
                        mlua::Value::Integer(_) | mlua::Value::Number(_) => Ok(lua
                            .coerce_string(v.clone())?
                            .expect("numbers coerce to strings")
                            .to_str()?
                            .to_owned()),
                        other => Err(mlua::Error::runtime(format!(
                            "attempt to concatenate a {} value",
                            other.type_name()
                        ))),
                    }
                };
                reg.add_meta_function(MetaMethod::Concat, move |lua, args: mlua::MultiValue| {
                    let v = args.into_vec();
                    if v.len() != 2 {
                        return Err(mlua::Error::runtime("expected 2 args"));
                    }
                    let mut out = fragment(lua, &v[0])?;
                    out.push_str(&fragment(lua, &v[1])?);
                    lua.create_string(&out)
                });
            }
            if let Some(f) = self.eq {
                reg.add_meta_function(MetaMethod::Eq, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let a: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let b: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    Ok(f(&*a.borrow::<T>()?, &*b.borrow::<T>()?))
                });
            }
            if let Some(f) = self.lt {
                reg.add_meta_function(MetaMethod::Lt, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let a: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let b: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    Ok(f(&*a.borrow::<T>()?, &*b.borrow::<T>()?))
                });
            }
            if let Some(f) = self.le {
                reg.add_meta_function(MetaMethod::Le, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let a: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let b: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    Ok(f(&*a.borrow::<T>()?, &*b.borrow::<T>()?))
                });
            }
            if let Some(f) = self.unm {
                reg.add_meta_function(MetaMethod::Unm, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    lua.create_any_userdata(f(&*ud.borrow::<T>()?))
                });
            }
            // `__bnot` exists only on Lua 5.3+; other versions reject the
            // registration up front in `meta_bnot`.
            #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
            if let Some(f) = self.bnot {
                reg.add_meta_function(MetaMethod::BNot, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    lua.create_any_userdata(f(&*ud.borrow::<T>()?))
                });
            }

            // Iteration. Value acquisition is this backend's policy: CLONE,
            // exactly as `meta_arith_self` operands are cloned. Each
            // iteration start builds one stepper owning one fresh iterator,
            // so concurrent iterations are independent snapshots.
            if let Some(f) = self.iter {
                let pairing = self.pairing;
                // `__pairs`: the built-in `pairs` consults it on 5.2+ and
                // LuaJIT with 5.2 compatibility.
                #[cfg(any(
                    feature = "lua55",
                    feature = "lua54",
                    feature = "lua53",
                    feature = "lua52",
                    feature = "luajit52"
                ))]
                reg.add_meta_method(MetaMethod::Pairs, move |lua, this: &T, ()| {
                    let stepper = make_stepper(lua, f(this.clone()), pairing)?;
                    Ok((stepper, mlua::Value::Nil, mlua::Value::Nil))
                });
                // `__ipairs`: the 5.2-era array protocol (the metamethod
                // exists only on Lua 5.2 and LuaJIT with 5.2 compatibility;
                // 5.3 deprecated it and 5.4+ removed it — no emulation
                // elsewhere). Always 1-based sequential `(i, item)`,
                // regardless of the kv pairing `pairs` uses: `ipairs` is by
                // definition the array protocol.
                #[cfg(any(feature = "lua52", feature = "luajit52"))]
                reg.add_meta_method(MetaMethod::IPairs, move |lua, this: &T, ()| {
                    let stepper = make_stepper(lua, f(this.clone()), IterPairing::Enumerate)?;
                    Ok((stepper, mlua::Value::Nil, mlua::Value::Nil))
                });
                // Luau's `__iter`: `for k, v in obj do` calls it for the
                // same (function, state, control) triple.
                #[cfg(feature = "luau")]
                reg.add_meta_method(MetaMethod::Iter, move |lua, this: &T, ()| {
                    let stepper = make_stepper(lua, f(this.clone()), pairing)?;
                    Ok((stepper, mlua::Value::Nil, mlua::Value::Nil))
                });
                // Portable `obj:iter()` on every version — the only story on
                // 5.1/LuaJIT, which have no `__pairs`.
                reg.add_function("iter", move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    if v.is_empty() {
                        return Err(mlua::Error::runtime("iter takes a receiver (use `:`)"));
                    }
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let this = ud.borrow::<T>()?;
                    make_stepper(lua, f(this.clone()), pairing)
                });
            }
            if let Some(f) = self.len {
                reg.add_meta_method(MetaMethod::Len, move |_, this: &T, ()| {
                    Ok(f(this.clone()) as mlua::Integer)
                });
            }

            // Lua has no hashing protocol; a portable method is the
            // native-ish surface, mirroring `iter()`. The u64 digest is
            // reinterpreted as Lua's signed 64-bit integer.
            if let Some(f) = self.hash {
                reg.add_method("hash", move |_, this: &T, ()| Ok(f(this) as i64));
            }
            // Debug formatting lands in mlua's own `__todebugstring` slot
            // (an ungated mlua extension consulted before `__tostring` when
            // Rust pretty-formats the userdata), plus a portable
            // `obj:debug()` method — scripts cannot reach the metamethod
            // through mlua's protected metatables. Same native-slot +
            // portable-companion pattern as `__pairs`/`iter()`.
            if let Some(f) = self.debug {
                reg.add_meta_method(MetaMethod::ToDebugString, move |_, this: &T, ()| {
                    Ok(f(this))
                });
                reg.add_method("debug", move |_, this: &T, ()| Ok(f(this)));
            }

            // Indexing. mlua consults registered fields and methods first
            // and falls back to these custom metamethods only for misses
            // (its generated `__index`/`__newindex` chain), so `obj.field`,
            // `obj:method()`, and `obj[key]` coexist. Keys convert verbatim
            // to the declared Rust index type — no 1-based adjustment — and
            // a Rust out-of-bounds panic surfaces as a Lua error.
            if let Some(f) = self.index {
                reg.add_meta_method(MetaMethod::Index, move |lua, this: &T, key: mlua::Value| {
                    let sv = lua_to_script(&key)?;
                    let out = f(this, std::slice::from_ref(&sv))
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, out)
                });
            }
            if let Some(f) = self.newindex {
                reg.add_meta_method_mut(
                    MetaMethod::NewIndex,
                    move |_, this: &mut T, (key, value): (mlua::Value, mlua::Value)| {
                        let k = lua_to_script(&key)?;
                        let v = lua_to_script(&value)?;
                        f(this, &[k, v]).map_err(|e| mlua::Error::runtime(e.to_string()))
                    },
                );
            }

            // Calling. `__call` exists on every Lua version.
            if let Some(f) = self.call {
                reg.add_meta_method(
                    MetaMethod::Call,
                    move |lua, this: &T, args: mlua::MultiValue| {
                        let sv_args: Vec<ScriptValue> = args
                            .into_vec()
                            .iter()
                            .map(lua_to_script)
                            .collect::<mlua::Result<_>>()?;
                        let out =
                            f(this, &sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                        script_to_lua(lua, out)
                    },
                );
            }
            // Async call: value acquisition is CLONE (like iteration), so
            // the future owns its value and borrows nothing. mlua only
            // offers async metamethods with its `async` feature and not on
            // Lua 5.1/Luau; under `send`, mlua demands `Send` futures while
            // `ScriptCallFuture` is deliberately not — all those combos are
            // rejected descriptively in `meta_call_async` instead.
            #[cfg(all(
                feature = "async",
                not(feature = "send"),
                not(any(feature = "lua51", feature = "luau"))
            ))]
            if let Some(f) = self.call_async {
                reg.add_async_meta_method(
                    MetaMethod::Call,
                    move |lua, this: mlua::UserDataRef<T>, args: mlua::MultiValue| {
                        let sv_args: mlua::Result<Vec<ScriptValue>> =
                            args.into_vec().iter().map(lua_to_script).collect();
                        async move {
                            // Borrowed through the guard the async block
                            // owns — zero-clone dispatch.
                            let sv_args = sv_args?;
                            let out = f(ScriptCow::Borrowed(&this), &sv_args)
                                .await
                                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                            script_to_lua(&lua, out)
                        }
                    },
                );
            }

            // Arithmetic: group by op, merge handlers. Repeated operator
            // declarations become overloads of ONE metamethod, resolved as:
            // `Self op Self` first, then scalar overloads whose declared rhs
            // type exactly matches the value's shape, then the remaining
            // overloads in declaration order (where an integer still
            // converts into a float overload).
            let mut self_ops: std::collections::BTreeMap<&str, ScriptArithSelf<T>> =
                std::collections::BTreeMap::new();
            let mut scalar_ops: std::collections::BTreeMap<&str, Vec<ScalarOverload<T>>> =
                std::collections::BTreeMap::new();
            for entry in &self.ariths {
                match entry {
                    ArithEntry::SelfOp(op, f) => {
                        self_ops.insert(op, *f);
                    }
                    ArithEntry::Scalar(op, overload) => {
                        scalar_ops.entry(op).or_default().push(*overload);
                    }
                }
            }
            // Collect all ops.
            let mut all_ops: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            all_ops.extend(self_ops.keys());
            all_ops.extend(scalar_ops.keys());

            // `%` precedence: an explicit `Mod` declaration (Lua floor
            // semantics) wins over `Rem` (Rust truncated semantics) when
            // both are declared — both target `__mod`.
            let has_mod = all_ops.contains("mod");
            for op in all_ops {
                if op == "rem" && has_mod {
                    continue;
                }
                let meta = match op {
                    "add" => MetaMethod::Add,
                    "sub" => MetaMethod::Sub,
                    "mul" => MetaMethod::Mul,
                    "div" => MetaMethod::Div,
                    "mod" | "rem" => MetaMethod::Mod,
                    "pow" => MetaMethod::Pow,
                    // `//`: an explicit IDiv declaration; pre-5.3 (non-Luau)
                    // versions rejected the registration up front.
                    #[cfg(any(
                        feature = "lua55",
                        feature = "lua54",
                        feature = "lua53",
                        feature = "luau"
                    ))]
                    "idiv" => MetaMethod::IDiv,
                    // Bitwise ops reach this map only on 5.3+; other
                    // versions rejected the registration up front.
                    #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
                    "bitand" => MetaMethod::BAnd,
                    #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
                    "bitor" => MetaMethod::BOr,
                    #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
                    "bitxor" => MetaMethod::BXor,
                    #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
                    "shl" => MetaMethod::Shl,
                    #[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
                    "shr" => MetaMethod::Shr,
                    _ => continue,
                };
                // The integer-Div fallback registers the same dispatch
                // under `__idiv` too (never overriding an explicit IDiv:
                // the fallback is only enabled when none is declared).
                let mut metas = vec![meta];
                #[cfg(any(
                    feature = "lua55",
                    feature = "lua54",
                    feature = "lua53",
                    feature = "luau"
                ))]
                if op == "div" && self.idiv_fallback {
                    metas.push(MetaMethod::IDiv);
                }
                for meta in metas {
                    let self_f = self_ops.get(op).copied();
                    let scalar_fs: Vec<ScalarOverload<T>> =
                        scalar_ops.get(op).cloned().unwrap_or_default();

                    reg.add_meta_function(meta, move |lua, args: mlua::MultiValue| {
                        let mut v = args.into_vec();
                        if v.len() != 2 {
                            return Err(mlua::Error::runtime("expected 2 args"));
                        }
                        let second = v.pop().unwrap();
                        let first = v.pop().unwrap();

                        // Try Self op Self.
                        if let Some(f) = self_f
                            && let Ok(a_ud) =
                                <mlua::AnyUserData as mlua::FromLua>::from_lua(first.clone(), lua)
                            && let Ok(b_ud) =
                                <mlua::AnyUserData as mlua::FromLua>::from_lua(second.clone(), lua)
                            && let Ok(a) = a_ud.borrow::<T>()
                            && let Ok(b) = b_ud.borrow::<T>()
                        {
                            return lua.create_any_userdata(f(a.clone(), b.clone()));
                        }

                        // Scalar overloads, two passes: exact rhs-type matches
                        // first, then the rest in declaration order.
                        let try_scalar =
                        |receiver: &mlua::Value,
                         scalar: &mlua::Value|
                         -> Option<mlua::Result<mlua::AnyUserData>> {
                            let ud = <mlua::AnyUserData as mlua::FromLua>::from_lua(
                                receiver.clone(),
                                lua,
                            )
                            .ok()?;
                            let this = ud.borrow::<T>().ok()?;
                            let sv = lua_to_script(scalar).ok()?;
                            for exact_pass in [true, false] {
                                for (rhs, f) in &scalar_fs {
                                    if rhs_matches_exactly(rhs, scalar) == exact_pass
                                        && let Ok(result) =
                                            f(this.clone(), std::slice::from_ref(&sv))
                                    {
                                        return Some(lua.create_any_userdata(result));
                                    }
                                }
                            }
                            None
                        };

                        // Self op scalar, then scalar op Self (commutative).
                        if let Some(result) = try_scalar(&first, &second) {
                            return result;
                        }
                        if let Some(result) = try_scalar(&second, &first) {
                            return result;
                        }

                        Err(mlua::Error::runtime("no matching operand types"))
                    });
                }
            }
        })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TypeBinder implementation — no unsafe, all ScriptValue-based
// ---------------------------------------------------------------------------

impl<T: 'static + Clone + mlua::MaybeSend + mlua::MaybeSync> TypeBinder<T> for LuaTypeBinder<T> {
    type Error = LuaBindError;

    fn field<V: IntoScript + FromScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        getter: fn(&T) -> V,
        setter: Option<fn(&mut T, V)>,
    ) -> Result<(), Self::Error> {
        let get_fn: FieldGetFn<T> =
            Arc::new(move |t, lua| script_to_lua(lua, getter(t).into_script()));
        let set_fn: Option<FieldSetFn<T>> = setter.map(|s| -> FieldSetFn<T> {
            Arc::new(move |t, val, _lua| {
                let sv = lua_to_script(&val)?;
                let v = V::from_script(sv).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                s(t, v);
                Ok(())
            })
        });
        self.fields.push(FieldReg {
            name,
            getter: get_fn,
            setter: set_fn,
        });
        Ok(())
    }

    fn method(&mut self, name: &'static str, f: CowMethodFn<T>) -> Result<(), Self::Error> {
        self.methods.push(MethodReg { name, f });
        Ok(())
    }

    fn method_mut(
        &mut self,
        name: &'static str,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.mut_methods.push(MutMethodReg { name, f });
        Ok(())
    }

    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, haphe::ScriptCallError>,
    ) -> Result<(), Self::Error> {
        self.constructors.push(CtorReg { name, f });
        Ok(())
    }

    fn constructor_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "async"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncFunction {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "async", feature = "send"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncFunction {
                name,
                reason: "the `send` feature demands `Send` futures, and async constructor \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "async", not(feature = "send")))]
        {
            self.async_ctors.push((name, f));
            Ok(())
        }
    }

    fn property_get(
        &mut self,
        name: &'static str,
        f: fn(&T) -> ScriptValue,
    ) -> Result<(), Self::Error> {
        self.prop_gets.push((name, f));
        Ok(())
    }

    fn property_set(
        &mut self,
        name: &'static str,
        f: fn(&mut T, ScriptValue) -> Result<(), haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.prop_sets.push((name, f));
        Ok(())
    }

    // mlua 0.12 exposes no async field accessors (`UserDataFields` is
    // sync-only), so async computed properties cannot surface as `obj.x`
    // in Lua. Rejected descriptively rather than inventing method
    // spellings; declare an async METHOD for awaited access.
    fn property_get_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(ScriptCow<'a, T>) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(LuaBindError::UnsupportedAsyncProperty { name })
    }

    fn property_set_async(
        &mut self,
        name: &'static str,
        _f: for<'a> fn(&'a mut T, ScriptValue) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        Err(LuaBindError::UnsupportedAsyncProperty { name })
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
        self.hash = Some(f);
        Ok(())
    }
    fn meta_debug(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error> {
        self.debug = Some(f);
        Ok(())
    }
    fn meta_eq(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.eq = Some(f);
        Ok(())
    }
    fn meta_lt(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.lt = Some(f);
        Ok(())
    }
    fn meta_le(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.le = Some(f);
        Ok(())
    }
    fn meta_unm(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error> {
        self.unm = Some(f);
        Ok(())
    }
    fn meta_bnot(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error> {
        check_op_supported("bnot")?;
        self.bnot = Some(f);
        Ok(())
    }
    fn meta_arith_self(&mut self, op: &'static str, f: fn(T, T) -> T) -> Result<(), Self::Error> {
        check_op_supported(op)?;
        self.ariths.push(ArithEntry::SelfOp(op, f));
        Ok(())
    }
    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        rhs: &'static haphe::TypeDescriptor<'static>,
        f: fn(T, &[ScriptValue]) -> Result<T, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        check_op_supported(op)?;
        self.ariths.push(ArithEntry::Scalar(op, (rhs, f)));
        Ok(())
    }
    fn meta_iter(&mut self, f: fn(T) -> ScriptIter) -> Result<(), Self::Error> {
        self.iter = Some(f);
        Ok(())
    }
    fn meta_len(&mut self, f: fn(T) -> usize) -> Result<(), Self::Error> {
        self.len = Some(f);
        Ok(())
    }
    fn meta_index(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.index = Some(f);
        Ok(())
    }
    fn meta_newindex(
        &mut self,
        f: fn(&mut T, &[ScriptValue]) -> Result<(), haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.newindex = Some(f);
        Ok(())
    }
    fn meta_call(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.call = Some(f);
        Ok(())
    }
    fn meta_call_async(&mut self, f: AsyncCowFn<T>) -> Result<(), Self::Error> {
        #[cfg(not(feature = "async"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncCall {
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "async", feature = "send"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncCall {
                reason: "the `send` feature demands `Send` futures, and async call \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(
            feature = "async",
            not(feature = "send"),
            any(feature = "lua51", feature = "luau")
        ))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncCall {
                reason: "mlua has no async metamethods on Lua 5.1 or Luau",
            })
        }
        #[cfg(all(
            feature = "async",
            not(feature = "send"),
            not(any(feature = "lua51", feature = "luau"))
        ))]
        {
            self.call_async = Some(f);
            Ok(())
        }
    }
    fn method_async(&mut self, name: &'static str, f: AsyncCowFn<T>) -> Result<(), Self::Error> {
        #[cfg(not(feature = "async"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "async", feature = "send"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "async", not(feature = "send")))]
        {
            self.async_methods.push((name, f));
            Ok(())
        }
    }
    fn method_async_mut(
        &mut self,
        name: &'static str,
        f: AsyncMutFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "async"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "async", feature = "send"))]
        {
            let _ = f;
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "async", not(feature = "send")))]
        {
            self.async_mut_methods.push((name, f));
            Ok(())
        }
    }

    fn method_generic(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: CowMethodFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::GenericFunction { name })
        }
        #[cfg(feature = "generics")]
        {
            self.generic_methods
                .push((mangle_generic_name(name, type_args), f));
            Ok(())
        }
    }

    fn method_generic_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: ScriptMethodMut<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::GenericFunction { name })
        }
        #[cfg(feature = "generics")]
        {
            self.generic_mut_methods
                .push((mangle_generic_name(name, type_args), f));
            Ok(())
        }
    }

    fn method_generic_async(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: AsyncCowFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::GenericFunction { name })
        }
        #[cfg(all(feature = "generics", not(feature = "async")))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", feature = "send"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            self.generic_async_methods
                .push((mangle_generic_name(name, type_args), f));
            Ok(())
        }
    }

    fn method_generic_async_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: AsyncMutFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::GenericFunction { name })
        }
        #[cfg(all(feature = "generics", not(feature = "async")))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", feature = "send"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncMethod {
                name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            self.generic_async_mut_methods
                .push((mangle_generic_name(name, type_args), f));
            Ok(())
        }
    }

    fn method_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: CowMethodFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(feature = "generics")]
        {
            push_dyn_candidate(
                &mut self.dyn_methods,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: self_inst.params,
                    self_args: self_inst.args,
                    wrapper: f,
                },
            );
            Ok(())
        }
    }

    fn method_dyn_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: ScriptMethodMut<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(feature = "generics")]
        {
            push_dyn_candidate(
                &mut self.dyn_mut_methods,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: self_inst.params,
                    self_args: self_inst.args,
                    wrapper: f,
                },
            );
            Ok(())
        }
    }

    fn method_dyn_async(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: AsyncCowFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(all(feature = "generics", not(feature = "async")))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", feature = "send"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            push_dyn_candidate(
                &mut self.dyn_async_methods,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: self_inst.params,
                    self_args: self_inst.args,
                    wrapper: f,
                },
            );
            Ok(())
        }
    }

    fn method_dyn_async_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: AsyncMutFn<T>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(all(feature = "generics", not(feature = "async")))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", feature = "send"))]
        {
            let _ = (type_args, self_inst, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "the `send` feature demands `Send` futures, and async method \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            push_dyn_candidate(
                &mut self.dyn_async_mut_methods,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: self_inst.params,
                    self_args: self_inst.args,
                    wrapper: f,
                },
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Free function binder
// ---------------------------------------------------------------------------

type ScriptFnPtr = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
#[cfg(all(feature = "async", not(feature = "send")))]
type AsyncFnPtr = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;

/// One compiled monomorph of a `dyn`-dispatched generic function or method.
#[cfg(feature = "generics")]
#[derive(Clone, Copy)]
struct DynFn<F> {
    descriptor: &'static haphe::FunctionDescriptor<'static>,
    type_args: &'static [haphe::TypeDescriptor<'static>],
    /// The SELF type's generic parameters/arguments for methods on generic
    /// self types (empty otherwise, and always empty for free functions):
    /// candidate descriptors may reference them (`GenericParam("T")`), so
    /// ranking substitutes them alongside the method's own.
    self_params: &'static [haphe::GenericParam<'static>],
    self_args: &'static [haphe::TypeDescriptor<'static>],
    wrapper: F,
}

/// Appends one dyn candidate to its name's list (declaration order kept).
#[cfg(feature = "generics")]
fn push_dyn_candidate<F>(lists: &mut Vec<(&'static str, Vec<DynFn<F>>)>, entry: DynFn<F>) {
    let name = entry.descriptor.name;
    match lists.iter_mut().find(|(n, _)| *n == name) {
        Some((_, list)) => list.push(entry),
        None => lists.push((name, vec![entry])),
    }
}

/// The scan table core's resolver ranks over, built once at bind time.
#[cfg(feature = "generics")]
fn dyn_scan_table<F>(candidates: &[DynFn<F>]) -> Vec<haphe::dispatch::DynCandidate<'static>> {
    candidates
        .iter()
        .map(|c| haphe::dispatch::DynCandidate {
            type_args: c.type_args,
            descriptor: c.descriptor,
        })
        .collect()
}

/// The scan state for dyn METHODS: on a non-generic self type the table is
/// prebuilt once (like free functions); on a generic self type each
/// candidate's substitution is the SELF instantiation merged with the
/// method's own (self params first — Rust forbids shadowing, so names never
/// collide), and the resolver ranks over per-call stack descriptors carrying
/// the merged parameter list.
#[cfg(feature = "generics")]
struct DynMethodScan {
    /// Prebuilt table when no candidate carries a self instantiation.
    fast: Option<Vec<haphe::dispatch::DynCandidate<'static>>>,
    /// Per-candidate merged `(params, args)` otherwise.
    merged: Vec<(
        Vec<haphe::GenericParam<'static>>,
        Vec<haphe::TypeDescriptor<'static>>,
    )>,
    descriptors: Vec<&'static haphe::FunctionDescriptor<'static>>,
}

#[cfg(feature = "generics")]
impl DynMethodScan {
    fn build<F>(candidates: &[DynFn<F>]) -> Self {
        if candidates.iter().all(|c| c.self_params.is_empty()) {
            return Self {
                fast: Some(dyn_scan_table(candidates)),
                merged: Vec::new(),
                descriptors: Vec::new(),
            };
        }
        Self {
            fast: None,
            merged: candidates
                .iter()
                .map(|c| {
                    let params: Vec<_> = c
                        .self_params
                        .iter()
                        .chain(c.descriptor.generic_params)
                        .copied()
                        .collect();
                    let args: Vec<_> = c.self_args.iter().chain(c.type_args).copied().collect();
                    (params, args)
                })
                .collect(),
            descriptors: candidates.iter().map(|c| c.descriptor).collect(),
        }
    }

    /// Candidate visit order for one call (see [`dyn_call_order`]).
    fn call_order(&self, args: &[ScriptValue]) -> impl Iterator<Item = usize> + use<> {
        match &self.fast {
            Some(scan) => dyn_call_order(args, scan),
            None => {
                let descs: Vec<haphe::FunctionDescriptor<'_>> = self
                    .descriptors
                    .iter()
                    .zip(&self.merged)
                    .map(|(d, (params, _))| haphe::FunctionDescriptor {
                        generic_params: params,
                        ..**d
                    })
                    .collect();
                let scan: Vec<haphe::dispatch::DynCandidate<'_>> = descs
                    .iter()
                    .zip(&self.merged)
                    .map(|(d, (_, args))| haphe::dispatch::DynCandidate {
                        type_args: args,
                        descriptor: d,
                    })
                    .collect();
                dyn_call_order(args, &scan)
            }
        }
    }
}

/// Rendered candidate signatures for no-match diagnostics.
#[cfg(feature = "generics")]
fn dyn_signatures<F>(candidates: &[DynFn<F>]) -> String {
    candidates
        .iter()
        .map(|c| {
            if c.self_params.is_empty() {
                render_candidate(c.descriptor, c.type_args)
            } else {
                // Render through the merged substitution so self-typed
                // parameters show their monomorph.
                let params: Vec<_> = c
                    .self_params
                    .iter()
                    .chain(c.descriptor.generic_params)
                    .copied()
                    .collect();
                let args: Vec<_> = c.self_args.iter().chain(c.type_args).copied().collect();
                let merged = haphe::FunctionDescriptor {
                    generic_params: &params,
                    ..*c.descriptor
                };
                render_candidate(&merged, &args)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Candidate visit order: the resolver's ranked pick first (when it has
/// one), then the remaining candidates in declaration order — the ranked
/// pick's `FromScript` stays authoritative, so a rejected conversion falls
/// through.
#[cfg(feature = "generics")]
fn dyn_call_order(
    args: &[ScriptValue],
    scan: &[haphe::dispatch::DynCandidate<'_>],
) -> impl Iterator<Item = usize> + use<> {
    let ranked = match haphe::dispatch::resolve_dyn_candidate(args, scan) {
        haphe::dispatch::Resolution::Ranked(i) => Some(i),
        haphe::dispatch::Resolution::TryCallOrder => None,
    };
    let len = scan.len();
    ranked
        .into_iter()
        .chain((0..len).filter(move |j| Some(*j) != ranked))
}

/// The error every candidate's rejection ends in: names each candidate
/// signature and the last conversion failure.
#[cfg(feature = "generics")]
fn dyn_no_match_error(
    name: &str,
    signatures: &str,
    last_err: Option<haphe::ScriptConvertError>,
) -> mlua::Error {
    mlua::Error::runtime(format!(
        "no dyn candidate of `{name}` accepts these arguments (candidates: {signatures}){}",
        last_err
            .map(|e| format!("; last error: {e}"))
            .unwrap_or_default()
    ))
}

/// Renders a candidate signature for no-match diagnostics.
#[cfg(feature = "generics")]
fn render_candidate(
    descriptor: &haphe::FunctionDescriptor<'_>,
    type_args: &[haphe::TypeDescriptor<'_>],
) -> String {
    fn render_ty(
        ty: &haphe::TypeDescriptor<'_>,
        subst: &haphe::dispatch::GenericSubst<'_>,
    ) -> String {
        use haphe::TypeDescriptor as T;
        match crate::peel_borrowed(ty) {
            T::GenericParam(name) => subst
                .params
                .iter()
                .position(|p| p.name == *name)
                .and_then(|i| subst.args.get(i))
                .map(|resolved| render_ty(resolved, subst))
                .unwrap_or_else(|| name.to_string()),
            T::Primitive(p) => format!("{p:?}").to_lowercase(),
            T::String => "string".into(),
            T::Bytes => "bytes".into(),
            T::Unit => "nil".into(),
            T::Option(inner) => format!("{}?", render_ty(inner, subst)),
            T::List(inner) | T::Array(inner, _) => format!("{}[]", render_ty(inner, subst)),
            T::Map(k, v) => format!("table<{}, {}>", render_ty(k, subst), render_ty(v, subst)),
            T::Ref(id) => id.as_str().rsplit("::").next().unwrap_or("?").to_string(),
            other => format!("{other:?}"),
        }
    }
    let subst = haphe::dispatch::GenericSubst {
        params: descriptor.generic_params,
        args: type_args,
    };
    let args: Vec<String> = type_args.iter().map(|a| render_ty(a, &subst)).collect();
    let params: Vec<String> = descriptor
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_ty(p.ty, &subst)))
        .collect();
    format!(
        "{}<{}>({})",
        descriptor.name,
        args.join(", "),
        params.join(", ")
    )
}

/// Collects free function registrations and applies them to a Lua table.
pub(crate) struct LuaFnBinder {
    functions: Vec<(&'static str, ScriptFnPtr)>,
    #[cfg(all(feature = "async", not(feature = "send")))]
    async_functions: Vec<(&'static str, AsyncFnPtr)>,
    /// Static generic monomorphs, keyed by mangled per-instantiation name.
    #[cfg(feature = "generics")]
    generic_functions: Vec<(String, ScriptFnPtr)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    generic_async_functions: Vec<(String, AsyncFnPtr)>,
    #[cfg(feature = "generics")]
    dyn_functions: Vec<(&'static str, Vec<DynFn<ScriptFnPtr>>)>,
    #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
    dyn_async_functions: Vec<(&'static str, Vec<DynFn<AsyncFnPtr>>)>,
}

impl LuaFnBinder {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            #[cfg(all(feature = "async", not(feature = "send")))]
            async_functions: Vec::new(),
            #[cfg(feature = "generics")]
            generic_functions: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            generic_async_functions: Vec::new(),
            #[cfg(feature = "generics")]
            dyn_functions: Vec::new(),
            #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
            dyn_async_functions: Vec::new(),
        }
    }

    /// Register all collected functions onto the given Lua table.
    pub fn apply(self, lua: &Lua, table: &mlua::Table) -> Result<(), LuaBindError> {
        // Static generic monomorphs land under mangled names next to the
        // plain functions; a collision would silently overwrite the earlier
        // table entry.
        #[cfg(feature = "generics")]
        {
            let mut seen: Vec<&str> = self.functions.iter().map(|(n, _)| *n).collect();
            #[cfg(all(feature = "async", not(feature = "send")))]
            seen.extend(self.async_functions.iter().map(|(n, _)| *n));
            let generic_names = self.generic_functions.iter().map(|(n, _)| n);
            #[cfg(all(feature = "async", not(feature = "send")))]
            let generic_names =
                generic_names.chain(self.generic_async_functions.iter().map(|(n, _)| n));
            for name in generic_names {
                if seen.contains(&name.as_str()) {
                    return Err(LuaBindError::DuplicateMethod { name: name.clone() });
                }
                seen.push(name.as_str());
            }
        }
        // Static generic monomorphs: one plain callable per declared
        // instantiation under its mangled name — dispatch is exact, no scan.
        #[cfg(feature = "generics")]
        for (name, f) in self.generic_functions {
            let lua_fn = lua.create_function(move |lua, args: mlua::MultiValue| {
                let script_args: Vec<ScriptValue> = args
                    .into_vec()
                    .iter()
                    .map(lua_to_script)
                    .collect::<mlua::Result<_>>()?;
                let result = f(&script_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                script_to_lua(lua, result)
            })?;
            table.set(name, lua_fn)?;
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        for (name, f) in self.generic_async_functions {
            let lua_fn = lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let sv_args: mlua::Result<Vec<ScriptValue>> =
                    args.into_vec().iter().map(lua_to_script).collect();
                async move {
                    let sv_args = sv_args?;
                    let out = f(&sv_args)
                        .await
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(&lua, out)
                }
            })?;
            table.set(name, lua_fn)?;
        }
        for (name, f) in self.functions {
            let lua_fn = lua.create_function(move |lua, args: mlua::MultiValue| {
                let script_args: Vec<ScriptValue> = args
                    .into_vec()
                    .iter()
                    .map(lua_to_script)
                    .collect::<mlua::Result<_>>()?;
                let result = f(&script_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                script_to_lua(lua, result)
            })?;
            table.set(name, lua_fn)?;
        }
        // Dyn generic functions: one Lua callable per name scans the
        // registered monomorph candidates at call time (core's shared
        // resolver), falling through in declaration order when the ranked
        // pick's conversions reject the values. Candidate tables are built
        // here, once — nothing allocates per call beyond the existing arg
        // conversion.
        #[cfg(feature = "generics")]
        for (name, candidates) in self.dyn_functions {
            let scan = dyn_scan_table(&candidates);
            let signatures = dyn_signatures(&candidates);
            let lua_fn = lua.create_function(move |lua, args: mlua::MultiValue| {
                let script_args: Vec<ScriptValue> = args
                    .into_vec()
                    .iter()
                    .map(lua_to_script)
                    .collect::<mlua::Result<_>>()?;
                let mut last_err = None;
                for i in dyn_call_order(&script_args, &scan) {
                    match (candidates[i].wrapper)(&script_args) {
                        Ok(out) => return script_to_lua(lua, out),
                        Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                        Err(host) => {
                            return Err(mlua::Error::runtime(host.to_string()));
                        }
                    }
                }
                Err(dyn_no_match_error(name, &signatures, last_err))
            })?;
            table.set(name, lua_fn)?;
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        for (name, candidates) in self.dyn_async_functions {
            let scan = std::sync::Arc::new(dyn_scan_table(&candidates));
            let signatures = dyn_signatures(&candidates);
            let candidates = std::sync::Arc::new(candidates);
            let lua_fn = lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let sv_args: mlua::Result<Vec<ScriptValue>> =
                    args.into_vec().iter().map(lua_to_script).collect();
                let candidates = candidates.clone();
                let scan = scan.clone();
                let signatures = signatures.clone();
                async move {
                    let script_args = sv_args?;
                    let mut last_err = None;
                    for i in dyn_call_order(&script_args, &scan) {
                        match (candidates[i].wrapper)(&script_args).await {
                            Ok(out) => return script_to_lua(&lua, out),
                            Err(haphe::ScriptCallError::Convert(e)) => last_err = Some(e),
                            Err(host) => {
                                return Err(mlua::Error::runtime(host.to_string()));
                            }
                        }
                    }
                    Err(dyn_no_match_error(name, &signatures, last_err))
                }
            })?;
            table.set(name, lua_fn)?;
        }
        // Async free functions: the async block owns the converted argument
        // vec; the wrapper's future borrows it across awaits.
        #[cfg(all(feature = "async", not(feature = "send")))]
        for (name, f) in self.async_functions {
            let lua_fn = lua.create_async_function(move |lua, args: mlua::MultiValue| {
                let sv_args: mlua::Result<Vec<ScriptValue>> =
                    args.into_vec().iter().map(lua_to_script).collect();
                async move {
                    let sv_args = sv_args?;
                    let out = f(&sv_args)
                        .await
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(&lua, out)
                }
            })?;
            table.set(name, lua_fn)?;
        }
        Ok(())
    }
}

impl FnBinder for LuaFnBinder {
    type Error = LuaBindError;

    fn function(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>,
    ) -> Result<(), Self::Error> {
        // Lua dispatch is by name only: a static generic monomorph gets its
        // own mangled table entry per instantiation (generics feature);
        // without it, same-named instantiations would silently shadow each
        // other.
        if !type_args.is_empty() {
            #[cfg(not(feature = "generics"))]
            {
                return Err(LuaBindError::GenericFunction { name });
            }
            #[cfg(feature = "generics")]
            {
                self.generic_functions
                    .push((mangle_generic_name(name, type_args), f));
                return Ok(());
            }
        }
        self.functions.push((name, f));
        Ok(())
    }

    fn function_async(
        &mut self,
        name: &'static str,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        if !type_args.is_empty() {
            return Err(LuaBindError::GenericFunction { name });
        }
        #[cfg(not(feature = "async"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncFunction {
                name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "async", feature = "send"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedAsyncFunction {
                name,
                reason: "the `send` feature demands `Send` futures, and async function \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "async", not(feature = "send")))]
        {
            #[cfg(feature = "generics")]
            if !type_args.is_empty() {
                self.generic_async_functions
                    .push((mangle_generic_name(name, type_args), f));
                return Ok(());
            }
            self.async_functions.push((name, f));
            Ok(())
        }
    }

    fn function_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(feature = "generics")]
        {
            push_dyn_candidate(
                &mut self.dyn_functions,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: &[],
                    self_args: &[],
                    wrapper: f,
                },
            );
            Ok(())
        }
    }

    fn function_dyn_async(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [haphe::TypeDescriptor<'static>],
        f: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        #[cfg(not(feature = "generics"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `generics` feature",
            })
        }
        #[cfg(all(feature = "generics", not(feature = "async")))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "enable this backend's `async` feature",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", feature = "send"))]
        {
            let _ = (type_args, f);
            Err(LuaBindError::UnsupportedDynFunction {
                name: descriptor.name,
                reason: "the `send` feature demands `Send` futures, and async function \
                         futures are deliberately not Send",
            })
        }
        #[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
        {
            push_dyn_candidate(
                &mut self.dyn_async_functions,
                DynFn {
                    descriptor,
                    type_args,
                    self_params: &[],
                    self_args: &[],
                    wrapper: f,
                },
            );
            Ok(())
        }
    }
}
