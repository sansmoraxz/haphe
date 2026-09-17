//! Foreign direction: Rust foreign-trait handles dispatching into Lua
//! functions, typically supplied by a script as a table of callbacks.

use std::collections::HashMap;
use std::fmt;

use haphe::{
    Dispatch, ForeignCaller, ForeignError, ForeignErrorKind, ForeignHandle,
    ForeignInterfaceDescriptor, ScriptForeign, ScriptValue, TypeDescriptor,
};
use mlua::{Function, Lua, Table};

use crate::LuaBindError;
use crate::binder::{lua_to_script, mangle_generic_name, script_to_lua};

/// Builds a [`ForeignCaller`] backed by Lua functions looked up in `table`,
/// one per function of `descriptor`, keyed by the exposed function name.
/// Every function is resolved up front; a missing or non-function field
/// fails with [`MissingForeignFunction`](LuaBindError::MissingForeignFunction)
/// (or [`MissingForeignInstantiation`](LuaBindError::MissingForeignInstantiation)
/// for a static generic's monomorph handler).
///
/// Connecting to a callbacks table involves no binder or registration
/// machinery — only an existing Lua state and the table.
///
/// Generic interfaces and generic functions require the `generics` feature.
/// A function-level generic honors its declared dispatch mode:
///
/// - STATIC (`instantiate(...)` without `dyn`): the table provides one
///   handler per declared instantiation under its mangled monomorph name
///   (`convert__i64`), the same spelling inbound static generics use; a
///   call whose type arguments match no declared instantiation fails
///   descriptively at call time.
/// - DYN: one handler under the plain name serves every instantiation —
///   type arguments are erased at the boundary (Lua values are
///   self-describing).
///
/// Trait-level generics stay erased: one table serves every instantiation
/// of a generic interface.
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

    let resolve = |name: &str| -> Result<Option<Function>, LuaBindError> {
        let value: mlua::Value = table.get(name).map_err(LuaBindError::Lua)?;
        match value {
            mlua::Value::Function(func) => Ok(Some(func)),
            _ => Ok(None),
        }
    };

    let mut funcs = HashMap::new();
    for f in descriptor.functions {
        if !cfg!(feature = "generics") && !f.generic_params.is_empty() {
            return Err(LuaBindError::GenericFunction { name: f.name });
        }
        let entry = if !f.generic_params.is_empty() && f.dispatch == Dispatch::Static {
            // Static dispatch: one handler per DECLARED instantiation,
            // addressed by the mangled monomorph name.
            let mut by_mangle = HashMap::new();
            let mut declared = Vec::new();
            for args in f.instantiations {
                let mangled = mangle_generic_name(f.name, args);
                let Some(func) = resolve(&mangled)? else {
                    return Err(LuaBindError::MissingForeignInstantiation {
                        interface: descriptor.name,
                        function: f.name,
                        mangled,
                    });
                };
                declared.push(mangled.clone());
                by_mangle.insert(mangled, func);
            }
            if by_mangle.is_empty() {
                // Unreachable through the macro (static generics require
                // instantiate), but hand-built descriptors stay loud.
                return Err(LuaBindError::GenericFunction { name: f.name });
            }
            FnEntry::Static {
                by_mangle,
                declared: declared.join(", "),
            }
        } else {
            // Non-generic, or `dyn` (erased): one handler, plain name.
            let Some(func) = resolve(f.name)? else {
                return Err(LuaBindError::MissingForeignFunction {
                    interface: descriptor.name,
                    function: f.name,
                });
            };
            FnEntry::Plain(func)
        };
        funcs.insert(f.name, entry);
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

/// One function's resolved handler(s), shaped by its declared dispatch.
enum FnEntry {
    /// Non-generic, or `dyn` (erased): one handler under the plain name.
    Plain(Function),
    /// Static generic: one handler per declared instantiation, keyed by the
    /// mangled monomorph name; `declared` is the rendered list for errors.
    Static {
        by_mangle: HashMap<String, Function>,
        declared: String,
    },
}

/// [`ForeignCaller`] over Lua functions resolved from a callbacks table.
struct LuaForeignCaller {
    lua: Lua,
    interface: &'static str,
    funcs: HashMap<&'static str, FnEntry>,
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
    /// Resolves the handler honoring the function's declared dispatch: the
    /// plain-named handler for non-generic and `dyn` functions, the mangled
    /// monomorph handler for static generics — where type arguments outside
    /// the declared instantiation set fail descriptively.
    fn resolve(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&Function, ForeignError> {
        match self.funcs.get(function) {
            None => Err(call_error(
                function,
                format!(
                    "function was not resolved from foreign interface `{}`",
                    self.interface
                ),
            )),
            Some(FnEntry::Plain(func)) => Ok(func),
            Some(FnEntry::Static {
                by_mangle,
                declared,
            }) => {
                let mangled = mangle_generic_name(function, type_args);
                by_mangle.get(&mangled).ok_or_else(|| {
                    call_error(
                        function,
                        format!(
                            "no handler for instantiation `{mangled}`: static dispatch \
                             serves only the declared instantiations ({declared})"
                        ),
                    )
                })
            }
        }
    }

    fn lower_args(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<(&Function, mlua::MultiValue), ForeignError> {
        let func = self.resolve(function, type_args)?;
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
        let (func, lowered) = self.lower_args(function, type_args, args)?;
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
        Box::pin(async move {
            let (func, lowered) = self.lower_args(function, type_args, args)?;
            let ret: mlua::Value = func
                .call_async(lowered)
                .await
                .map_err(|e| call_error(function, e.to_string()))?;
            lua_to_script(&ret).map_err(|e| call_error(function, e.to_string()))
        })
    }
}
