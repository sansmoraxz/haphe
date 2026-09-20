//! Synthesized variant-based dispatchers for `dyn`-declared generics
//! (cargo feature `dyn-generics`).
//!
//! WIT guests always name a monomorph statically, so bare dynamic dispatch
//! is unrepresentable natively. Behind the feature, the generator emits —
//! in addition to the static monomorphs, which remain — one dispatcher per
//! dyn function or method: `{name}-dyn`, where every generic-typed
//! parameter (and return) becomes a NAMED `variant` type with one case per
//! declared instantiation, the case named by the instantiation's mangled
//! type arguments. WIT admits no anonymous variants, so the README's inline
//! sketch materializes as named types, deduplicated by shape within an
//! interface.
//!
//! The runtime host implements the dispatcher by reading the case tag
//! (which names the instantiation, so matching degenerates to exact),
//! running the SAME shared core resolver
//! ([`haphe::dispatch::resolve_dyn_candidate`]) over the collected dyn
//! candidate table, and lifting the result back under the winning case.
//!
//! Compiled only with the feature; a minimal inert stub in `lib.rs` keeps
//! call sites `cfg`-free without it, so a feature-less build carries none
//! of this machinery and emits byte-identical text.

use std::collections::HashMap;

use haphe::{Dispatch, FunctionDescriptor, TypeDescriptor};

use crate::WitGenError;
use crate::emit::Printer;
use crate::model::{Env, Plan};
use crate::names::to_kebab;
use crate::types::{Pos, render_type};

/// Whether `f` gets a synthesized dispatcher: declared `dyn` and generic.
pub(crate) fn is_dyn(f: &FunctionDescriptor<'_>) -> bool {
    !f.generic_params.is_empty() && matches!(f.dispatch, Dispatch::Dyn)
}

pub(crate) use crate::types::mentions_named_generic;

/// One rendered dispatcher signature: parameter and return types are final
/// WIT text (variant names or passed-through plain types).
pub(crate) struct DispatcherSig {
    /// Raw member name (pre-kebab): `{fn_name}-dyn`.
    pub raw_name: String,
    /// `(kebab param name, rendered type)`, receiver excluded.
    pub params: Vec<(String, String)>,
    /// Rendered return type, `None` for unit.
    pub ret: Option<String>,
    pub is_async: bool,
    /// `haphe:dyn-dispatcher = {fn_name}` marker doc line.
    pub marker: String,
}

impl DispatcherSig {
    pub fn param_list(&self) -> String {
        let parts: Vec<String> = self
            .params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect();
        parts.join(", ")
    }

    pub fn arrow(&self) -> String {
        match &self.ret {
            Some(t) => format!(" -> {t}"),
            None => String::new(),
        }
    }

    pub fn keyword(&self) -> &'static str {
        if self.is_async { "async func" } else { "func" }
    }
}

/// Per-interface emission context: deduplicates variant type definitions by
/// shape (an identical case/payload list reuses the first definition's
/// name).
#[derive(Default)]
pub(crate) struct DynCx {
    by_shape: HashMap<Vec<(String, Option<String>)>, String>,
}

impl DynCx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the dispatcher signature for `f`, emitting any NEW variant
    /// type definitions through `p` (interface level — callers ensure this
    /// runs outside resource blocks). `owner` prefixes synthesized variant
    /// names to keep them collision-free across a type's methods; `env` is
    /// the self type's instantiation environment, if any.
    pub fn dispatcher(
        &mut self,
        p: &mut Printer,
        f: &FunctionDescriptor<'_>,
        owner: Option<&str>,
        plan: &Plan<'_>,
        env: Option<&Env<'_, '_>>,
    ) -> Result<DispatcherSig, WitGenError> {
        let context = format!("{} (dyn dispatcher)", f.name);
        let base = match owner {
            Some(owner) => format!("{owner}-{}-dyn", to_kebab(f.name)),
            None => format!("{}-dyn", to_kebab(f.name)),
        };

        // Case names: one per declared instantiation, the instantiation's
        // mangled type arguments joined with `-` (single-parameter generics
        // read as the plain type name, e.g. `s64`). Duplicate declarations
        // collapse to one case.
        let mut cases: Vec<(String, &[TypeDescriptor<'_>])> = Vec::new();
        for args in f.instantiations {
            let key = case_key_planned(plan, args, env)?;
            if !cases.iter().any(|(k, _)| *k == key) {
                cases.push((key, args));
            }
        }

        let mut params = Vec::new();
        for param in f.params {
            let pname = to_kebab(param.name);
            if !mentions_named_generic(param.ty, f.generic_params) {
                let ty = render_type(param.ty, Pos::Param(param.ownership), plan, env, &context)?;
                params.push((pname, ty));
                continue;
            }
            let ty = self.variant_for(
                p,
                &format!("{base}-{}", to_kebab(param.name)),
                &cases,
                f,
                param.ty,
                Pos::Param(param.ownership),
                plan,
                env,
                &context,
            )?;
            params.push((pname, ty));
        }

        let ret = match crate::types::peel_borrowed(f.return_type) {
            TypeDescriptor::Unit => None,
            ret if !mentions_named_generic(ret, f.generic_params) => Some(render_type(
                ret,
                Pos::Return(f.return_ownership),
                plan,
                env,
                &context,
            )?),
            ret => Some(self.variant_for(
                p,
                &format!("{base}-result"),
                &cases,
                f,
                ret,
                Pos::Return(f.return_ownership),
                plan,
                env,
                &context,
            )?),
        };

        // A FALLIBLE dispatcher's return wraps like any other fallible
        // surface, in both directions: `result<T, script-error>`.
        let ret = if f.fallible {
            Some(match ret {
                Some(t) => format!("result<{t}, script-error>"),
                None => "result<_, script-error>".to_string(),
            })
        } else {
            ret
        };

        Ok(DispatcherSig {
            raw_name: format!("{}-dyn", f.name),
            params,
            ret,
            is_async: f.is_async,
            marker: format!("haphe:dyn-dispatcher = {}", f.name),
        })
    }

    /// Returns the (possibly shared) variant type name for one generic
    /// position, emitting the definition if its shape is new.
    #[allow(
        clippy::too_many_arguments,
        reason = "each parameter names one independent emission input; bundling them into an ad-hoc struct would obscure the call sites"
    )]
    fn variant_for(
        &mut self,
        p: &mut Printer,
        preferred_name: &str,
        cases: &[(String, &[TypeDescriptor<'_>])],
        f: &FunctionDescriptor<'_>,
        pos_ty: &TypeDescriptor<'_>,
        pos: Pos,
        plan: &Plan<'_>,
        env: Option<&Env<'_, '_>>,
        context: &str,
    ) -> Result<String, WitGenError> {
        let mut shape: Vec<(String, Option<String>)> = Vec::new();
        for (key, args) in cases {
            let fenv = Plan::fn_env(f, args, env);
            let payload = match crate::types::peel_borrowed(pos_ty) {
                TypeDescriptor::Unit => None,
                _ => Some(render_type(pos_ty, pos, plan, Some(&fenv), context)?),
            };
            // A substituted payload of `unit` (e.g. `T = ()`) becomes a
            // payload-less case.
            let payload = payload.filter(|t| t != "unit");
            shape.push((to_kebab(key), payload));
        }
        if let Some(existing) = self.by_shape.get(&shape) {
            return Ok(existing.clone());
        }
        p.doc(Some(&format!(
            "Candidate payloads for the `{}` dynamic dispatcher.",
            f.name
        )));
        p.open(&format!("variant {preferred_name}"));
        for (case, payload) in &shape {
            match payload {
                Some(t) => p.line(&format!("{case}({t}),")),
                None => p.line(&format!("{case},")),
            }
        }
        p.close();
        self.by_shape.insert(shape, preferred_name.to_string());
        Ok(preferred_name.to_string())
    }
}

/// Plan-aware case key for one instantiation: its mangled type arguments
/// joined with `-` (unescaped — this is the RESOLVED case name the runtime
/// compares against).
fn case_key_planned(
    plan: &Plan<'_>,
    args: &[TypeDescriptor<'_>],
    env: Option<&Env<'_, '_>>,
) -> Result<String, WitGenError> {
    let fragments: Vec<String> = args
        .iter()
        .map(|a| plan.mangle_type(a, env))
        .collect::<Result<_, _>>()?;
    Ok(fragments.join("-"))
}

/// Plan-independent case key, for the runtime host (which has no plan).
/// Type arguments referencing registered types are not supported here, the
/// same limitation as runtime instance registration. KEEP IN SYNC with
/// [`case_key_planned`].
#[cfg_attr(
    not(feature = "runtime"),
    allow(dead_code, reason = "only the runtime host consumes it")
)]
pub(crate) fn case_key_plain(args: &[TypeDescriptor<'_>]) -> Result<String, WitGenError> {
    let fragments: Vec<String> = args
        .iter()
        .map(crate::model::mangle_plain_type)
        .collect::<Result<_, _>>()?;
    Ok(fragments.join("-"))
}
