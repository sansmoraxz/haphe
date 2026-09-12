//! `TypeBinder<T>` implementation for mlua.
//!
//! Collects field/method/metamethod registrations from `ScriptBind::bind`,
//! then applies them to a Lua state by creating UserData + FromLua impls
//! at registration time.

use std::sync::Arc;

use haphe::bridge::{FromScript, IntoScript, ScriptValue};
use haphe::{FnBinder, TypeBinder};
use mlua::{Lua, MetaMethod, UserDataFields, UserDataMethods};

use crate::LuaBindError;

/// Converts a [`ScriptValue`] to an [`mlua::Value`].
fn script_to_lua(lua: &Lua, v: ScriptValue) -> mlua::Result<mlua::Value> {
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
        ScriptValue::UserData(ud) => lua
            .create_any_userdata(ud)
            .map(mlua::Value::UserData),
        _ => Err(mlua::Error::runtime("unsupported ScriptValue variant")),
    }
}

/// Converts an [`mlua::Value`] to a [`ScriptValue`].
fn lua_to_script(v: &mlua::Value) -> mlua::Result<ScriptValue> {
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
                Err(mlua::Error::runtime("cannot convert foreign userdata to script value"))
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "cannot convert {} to script value",
            other.type_name()
        ))),
    }
}

fn lua_table_to_script(t: &mlua::Table) -> mlua::Result<ScriptValue> {
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
type FieldSetFn<T> = Arc<dyn Fn(&mut T, mlua::Value, &Lua) -> mlua::Result<()> + Send + Sync + 'static>;

// Method/constructor fn pointers using ScriptValue — no generics needed.
type ScriptMethodRef<T> = fn(&T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>;
type ScriptMethodMut<T> = fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>;
type ScriptCtorFn<T> = fn(&[ScriptValue]) -> Result<T, haphe::ScriptConvertError>;
type ScriptArithSelf<T> = fn(T, T) -> T;
type ScriptArithScalar<T> = fn(T, &[ScriptValue]) -> Result<T, haphe::ScriptConvertError>;

struct FieldReg<T: 'static> {
    name: &'static str,
    getter: FieldGetFn<T>,
    setter: Option<FieldSetFn<T>>,
}

struct MethodReg<T: 'static> {
    name: &'static str,
    f: ScriptMethodRef<T>,
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
    Scalar(&'static str, ScriptArithScalar<T>),
}

/// Collects registrations from `ScriptBind::bind`, then applies them to Lua.
pub(crate) struct LuaTypeBinder<T: 'static> {
    fields: Vec<FieldReg<T>>,
    methods: Vec<MethodReg<T>>,
    mut_methods: Vec<MutMethodReg<T>>,
    constructors: Vec<CtorReg<T>>,
    tostring: Option<fn(&T) -> String>,
    eq: Option<fn(&T, &T) -> bool>,
    lt: Option<fn(&T, &T) -> bool>,
    le: Option<fn(&T, &T) -> bool>,
    unm: Option<fn(&T) -> T>,
    ariths: Vec<ArithEntry<T>>,
}

impl<T: 'static + Clone + mlua::MaybeSend + mlua::MaybeSync> LuaTypeBinder<T> {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            methods: Vec::new(),
            mut_methods: Vec::new(),
            constructors: Vec::new(),
            tostring: None,
            eq: None,
            lt: None,
            le: None,
            unm: None,
            ariths: Vec::new(),
        }
    }

    /// Apply all collected registrations to the Lua state.
    pub fn register(self, lua: &Lua, type_table: &mlua::Table) -> Result<(), LuaBindError> {
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

            // Methods (&self).
            for method in &self.methods {
                let f = method.f;
                reg.add_function(method.name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let this = ud.borrow::<T>()?;
                    let sv_args: Vec<ScriptValue> = v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let result = f(&*this, &sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }

            // Methods (&mut self).
            for method in &self.mut_methods {
                let f = method.f;
                reg.add_function(method.name, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    let ud: mlua::AnyUserData = mlua::FromLua::from_lua(v.remove(0), lua)?;
                    let mut this = ud.borrow_mut::<T>()?;
                    let sv_args: Vec<ScriptValue> = v.iter().map(lua_to_script).collect::<mlua::Result<_>>()?;
                    let result = f(&mut *this, &sv_args).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    script_to_lua(lua, result)
                });
            }

            // Metamethods.
            if let Some(f) = self.tostring {
                reg.add_meta_method(MetaMethod::ToString, move |_, this, ()| Ok(f(this)));
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

            // Arithmetic: group by op, merge handlers.
            let mut self_ops: std::collections::BTreeMap<&str, ScriptArithSelf<T>> =
                std::collections::BTreeMap::new();
            let mut scalar_ops: std::collections::BTreeMap<&str, Vec<ScriptArithScalar<T>>> =
                std::collections::BTreeMap::new();
            for entry in &self.ariths {
                match entry {
                    ArithEntry::SelfOp(op, f) => { self_ops.insert(op, *f); }
                    ArithEntry::Scalar(op, f) => { scalar_ops.entry(op).or_default().push(*f); }
                }
            }
            // Collect all ops.
            let mut all_ops: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            all_ops.extend(self_ops.keys());
            all_ops.extend(scalar_ops.keys());

            for op in all_ops {
                let meta = match op {
                    "add" => MetaMethod::Add,
                    "sub" => MetaMethod::Sub,
                    "mul" => MetaMethod::Mul,
                    "div" => MetaMethod::Div,
                    "mod" | "rem" => MetaMethod::Mod,
                    _ => continue,
                };
                let self_f = self_ops.get(op).copied();
                let scalar_fs: Vec<ScriptArithScalar<T>> =
                    scalar_ops.get(op).cloned().unwrap_or_default();

                reg.add_meta_function(meta, move |lua, args: mlua::MultiValue| {
                    let mut v = args.into_vec();
                    if v.len() != 2 { return Err(mlua::Error::runtime("expected 2 args")); }
                    let second = v.pop().unwrap();
                    let first = v.pop().unwrap();

                    // Try Self op Self.
                    if let Some(f) = self_f
                        && let Ok(a_ud) = <mlua::AnyUserData as mlua::FromLua>::from_lua(first.clone(), lua)
                        && let Ok(b_ud) = <mlua::AnyUserData as mlua::FromLua>::from_lua(second.clone(), lua)
                        && let Ok(a) = a_ud.borrow::<T>()
                        && let Ok(b) = b_ud.borrow::<T>()
                    {
                        return lua.create_any_userdata(f(a.clone(), b.clone()));
                    }

                    // Try Self op scalar.
                    for f in &scalar_fs {
                        if let Ok(a_ud) = <mlua::AnyUserData as mlua::FromLua>::from_lua(first.clone(), lua)
                            && let Ok(a) = a_ud.borrow::<T>()
                            && let Ok(sv) = lua_to_script(&second)
                            && let Ok(result) = f(a.clone(), &[sv])
                        {
                            return lua.create_any_userdata(result);
                        }
                    }

                    // Try scalar op Self (commutative).
                    for f in &scalar_fs {
                        if let Ok(b_ud) = <mlua::AnyUserData as mlua::FromLua>::from_lua(second.clone(), lua)
                            && let Ok(b) = b_ud.borrow::<T>()
                            && let Ok(sv) = lua_to_script(&first)
                            && let Ok(result) = f(b.clone(), &[sv])
                        {
                            return lua.create_any_userdata(result);
                        }
                    }

                    Err(mlua::Error::runtime("no matching operand types"))
                });
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
        let get_fn: FieldGetFn<T> = Arc::new(move |t, lua| {
            script_to_lua(lua, getter(t).into_script())
        });
        let set_fn: Option<FieldSetFn<T>> = setter.map(|s| -> FieldSetFn<T> {
            Arc::new(move |t, val, _lua| {
                let sv = lua_to_script(&val)?;
                let v = V::from_script(sv).map_err(|e| mlua::Error::runtime(e.to_string()))?;
                s(t, v);
                Ok(())
            })
        });
        self.fields.push(FieldReg { name, getter: get_fn, setter: set_fn });
        Ok(())
    }

    fn method_ref(
        &mut self,
        name: &'static str,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.methods.push(MethodReg { name, f });
        Ok(())
    }

    fn method_mut(
        &mut self,
        name: &'static str,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.mut_methods.push(MutMethodReg { name, f });
        Ok(())
    }

    fn method_owned(
        &mut self,
        _name: &'static str,
        _f: fn(T, &[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        // TODO: owned methods need special handling in mlua
        Ok(())
    }

    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.constructors.push(CtorReg { name, f });
        Ok(())
    }

    fn meta_tostring(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error> {
        self.tostring = Some(f); Ok(())
    }
    fn meta_eq(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.eq = Some(f); Ok(())
    }
    fn meta_lt(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.lt = Some(f); Ok(())
    }
    fn meta_le(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error> {
        self.le = Some(f); Ok(())
    }
    fn meta_unm(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error> {
        self.unm = Some(f); Ok(())
    }
    fn meta_arith_self(
        &mut self,
        op: &'static str,
        f: fn(T, T) -> T,
    ) -> Result<(), Self::Error> {
        self.ariths.push(ArithEntry::SelfOp(op, f));
        Ok(())
    }
    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        f: fn(T, &[ScriptValue]) -> Result<T, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.ariths.push(ArithEntry::Scalar(op, f));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Free function binder
// ---------------------------------------------------------------------------

type ScriptFnPtr = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>;

/// Collects free function registrations and applies them to a Lua table.
pub(crate) struct LuaFnBinder {
    functions: Vec<(&'static str, ScriptFnPtr)>,
}

impl LuaFnBinder {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
        }
    }

    /// Register all collected functions onto the given Lua table.
    pub fn apply(self, lua: &Lua, table: &mlua::Table) -> Result<(), LuaBindError> {
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
        Ok(())
    }
}

impl FnBinder for LuaFnBinder {
    type Error = LuaBindError;

    fn function(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.functions.push((name, f));
        Ok(())
    }
}
