//! Foreign direction: Rust foreign-trait handles dispatching into Lua
//! functions, typically supplied by a script as a table of callbacks.

use std::collections::HashMap;
use std::fmt;

use haphe::{
    ForeignCaller, ForeignError, ForeignErrorKind, ForeignHandle, ForeignInterfaceDescriptor,
    ScriptForeign, ScriptValue, TypeDescriptor,
};
use mlua::{Function, Lua, Table};

use crate::LuaBindError;
use crate::binder::{lua_to_script, script_to_lua};

/// Builds a [`ForeignCaller`] backed by Lua functions looked up in `table`,
/// one per function of `descriptor`, keyed by the exposed function name.
/// Every function is resolved up front; a missing or non-function field
/// fails with [`MissingForeignFunction`](LuaBindError::MissingForeignFunction).
///
/// Connecting to a callbacks table involves no binder or registration
/// machinery — only an existing Lua state and the table.
///
/// Generic interfaces and generic functions require the `generics` feature:
/// Lua dispatch is dynamic, so one Lua function serves every instantiation
/// and `type_args` are ignored.
pub fn foreign_caller(
    lua: &Lua,
    table: Table,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &[TypeDescriptor<'static>],
) -> Result<Box<dyn ForeignCaller>, LuaBindError> {
    if !cfg!(feature = "generics") && !descriptor.generic_params.is_empty() {
        return Err(LuaBindError::GenericForeignInterface {
            name: descriptor.name,
        });
    }
    // One table serves every instantiation of a generic interface.
    let _ = type_args;

    let mut funcs = HashMap::new();
    for f in descriptor.functions {
        if !cfg!(feature = "generics") && !f.generic_params.is_empty() {
            return Err(LuaBindError::GenericFunction { name: f.name });
        }
        let value: mlua::Value = table.get(f.name).map_err(LuaBindError::Lua)?;
        let mlua::Value::Function(func) = value else {
            return Err(LuaBindError::MissingForeignFunction {
                interface: descriptor.name,
                function: f.name,
            });
        };
        funcs.insert(f.name, func);
    }
    Ok(Box::new(LuaForeignCaller {
        lua: lua.clone(),
        interface: descriptor.name,
        funcs,
    }))
}

/// Builds the foreign-trait handle `H`, dispatching into the Lua functions
/// of `table` (see [`foreign_caller`]).
pub fn foreign_handle<H: ScriptForeign + ForeignHandle>(
    lua: &Lua,
    table: Table,
) -> Result<H, LuaBindError> {
    Ok(H::from_caller(foreign_caller(
        lua,
        table,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
    )?))
}

/// [`ForeignCaller`] over Lua functions resolved from a callbacks table.
struct LuaForeignCaller {
    lua: Lua,
    interface: &'static str,
    funcs: HashMap<&'static str, Function>,
}

/// A Lua error carried through [`ForeignErrorKind::Call`] (`mlua::Error` is
/// `Send + Sync` only with the `error-send` feature, so it is stringified).
#[derive(Debug)]
struct LuaCallError(String);

impl fmt::Display for LuaCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LuaCallError {}

fn call_error(function: &'static str, message: String) -> ForeignError {
    ForeignError {
        function,
        kind: ForeignErrorKind::Call(Box::new(LuaCallError(message))),
    }
}

impl LuaForeignCaller {
    fn lower_args(
        &self,
        function: &'static str,
        args: &[ScriptValue],
    ) -> Result<(&Function, mlua::MultiValue), ForeignError> {
        let func = self.funcs.get(function).ok_or_else(|| {
            call_error(
                function,
                format!(
                    "function was not resolved from foreign interface `{}`",
                    self.interface
                ),
            )
        })?;
        let mut lowered = Vec::with_capacity(args.len());
        for arg in args {
            lowered.push(
                script_to_lua(&self.lua, arg.clone())
                    .map_err(|e| call_error(function, e.to_string()))?,
            );
        }
        Ok((func, mlua::MultiValue::from_iter(lowered)))
    }
}

impl ForeignCaller for LuaForeignCaller {
    fn call(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        // Dynamic dispatch: every instantiation shares one Lua function.
        let _ = type_args;
        let (func, lowered) = self.lower_args(function, args)?;
        let ret: mlua::Value = func
            .call(lowered)
            .map_err(|e| call_error(function, e.to_string()))?;
        lua_to_script(&ret).map_err(|e| call_error(function, e.to_string()))
    }

    #[cfg(feature = "async")]
    fn call_async<'a>(
        &'a self,
        function: &'static str,
        type_args: &'a [TypeDescriptor<'static>],
        args: &'a [ScriptValue],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ScriptValue, ForeignError>> + 'a>>
    {
        let _ = type_args;
        Box::pin(async move {
            let (func, lowered) = self.lower_args(function, args)?;
            let ret: mlua::Value = func
                .call_async(lowered)
                .await
                .map_err(|e| call_error(function, e.to_string()))?;
            lua_to_script(&ret).map_err(|e| call_error(function, e.to_string()))
        })
    }
}
