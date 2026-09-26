use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use haphe::{
    Dispatch, ForeignCaller, ForeignError, ForeignErrorKind, ForeignHandle,
    ForeignInterfaceDescriptor, ScriptForeign, ScriptValue, TypeDescriptor,
};
use rhai::{Dynamic, Engine, FnPtr, Map};

use crate::RhaiBindError;
use crate::binder::{dynamic_to_script, mangle_generic_name, script_to_dynamic};

/// Builds a [`ForeignCaller`] backed by Rhai functions looked up in
/// `callbacks`, one per function of `descriptor`, keyed by the exposed
/// function name.
///
/// `engine` is stored (reference-counted) so the caller can dispatch through
/// [`Engine::call_fn_with_options`] at call time.  `ast` must contain the
/// script-defined functions that the `FnPtr` values in `callbacks` refer to;
/// only functions in this AST are reachable at call time.
///
/// Build the foreign caller AFTER all types and functions have been
/// registered on the engine.
///
/// Generic interfaces and generic functions require the `generics` feature.
pub fn foreign_caller(
    engine: Arc<Engine>,
    callbacks: &Map,
    ast: rhai::AST,
    descriptor: &ForeignInterfaceDescriptor<'static>,
    type_args: &[TypeDescriptor<'static>],
) -> Result<Box<dyn ForeignCaller>, RhaiBindError> {
    if !cfg!(feature = "generics") && !descriptor.generic_params.is_empty() {
        return Err(RhaiBindError::GenericForeignInterface {
            name: descriptor.name,
        });
    }
    let _ = type_args;

    let resolve = |name: &str| -> Option<FnPtr> {
        callbacks
            .get(name)
            .and_then(|v| v.clone().try_cast::<FnPtr>())
    };

    let mut funcs = HashMap::new();
    for f in descriptor.functions {
        if !cfg!(feature = "generics") && !f.generic_params.is_empty() {
            return Err(RhaiBindError::GenericFunction { name: f.name });
        }
        let entry = if !f.generic_params.is_empty() && f.dispatch == Dispatch::Static {
            let mut by_mangle = HashMap::new();
            let mut declared = Vec::new();
            for args in f.instantiations {
                let mangled = mangle_generic_name(f.name, args);
                let Some(func) = resolve(&mangled) else {
                    return Err(RhaiBindError::MissingForeignInstantiation {
                        interface: descriptor.name,
                        function: f.name,
                        mangled,
                    });
                };
                declared.push(mangled.clone());
                by_mangle.insert(mangled, func);
            }
            if by_mangle.is_empty() {
                return Err(RhaiBindError::GenericFunction { name: f.name });
            }
            FnEntry::Static {
                by_mangle,
                declared: declared.join(", "),
            }
        } else {
            let Some(func) = resolve(f.name) else {
                return Err(RhaiBindError::MissingForeignFunction {
                    interface: descriptor.name,
                    function: f.name,
                });
            };
            FnEntry::Plain(func)
        };
        funcs.insert(f.name, entry);
    }

    Ok(Box::new(RhaiForeignCaller {
        engine,
        ast,
        interface: descriptor.name,
        funcs,
    }))
}

/// Builds the foreign-trait handle `H`, dispatching into the Rhai functions
/// of `callbacks` (see [`foreign_caller`]).
///
/// `ast` must contain the script-defined functions that the `FnPtr` values
/// in `callbacks` refer to.
pub fn foreign_handle<H: ScriptForeign + ForeignHandle>(
    engine: Arc<Engine>,
    callbacks: &Map,
    ast: rhai::AST,
) -> Result<H, RhaiBindError> {
    Ok(H::from_caller(foreign_caller(
        engine,
        callbacks,
        ast,
        &H::DESCRIPTOR,
        H::TYPE_ARGS,
    )?))
}

enum FnEntry {
    Plain(FnPtr),
    Static {
        by_mangle: HashMap<String, FnPtr>,
        declared: String,
    },
}

struct RhaiForeignCaller {
    engine: Arc<Engine>,
    ast: rhai::AST,
    interface: &'static str,
    funcs: HashMap<&'static str, FnEntry>,
}

#[derive(Debug)]
struct RhaiCallError(String);

impl fmt::Display for RhaiCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RhaiCallError {}

fn foreign_call_error(function: &'static str, message: String) -> ForeignError {
    ForeignError {
        function,
        kind: ForeignErrorKind::Call(Box::new(RhaiCallError(message))),
    }
}

impl RhaiForeignCaller {
    fn resolve(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
    ) -> Result<&FnPtr, ForeignError> {
        match self.funcs.get(function) {
            None => Err(foreign_call_error(
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
                    foreign_call_error(
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
}

impl ForeignCaller for RhaiForeignCaller {
    fn call(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        let func = self.resolve(function, type_args)?;
        let fn_name = func.fn_name();
        let dynamic_args: Vec<Dynamic> = args.iter().cloned().map(script_to_dynamic).collect();

        let options = rhai::CallFnOptions::new()
            .eval_ast(false)
            .rewind_scope(true);

        let mut scope = rhai::Scope::new();

        let result: Dynamic = self
            .engine
            .call_fn_with_options(options, &mut scope, &self.ast, fn_name, dynamic_args)
            .map_err(|e| foreign_call_error(function, e.to_string()))?;

        dynamic_to_script(result).map_err(|e| foreign_call_error(function, e.to_string()))
    }
}
