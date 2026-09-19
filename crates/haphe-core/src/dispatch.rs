//! Dynamic-dispatch resolution for `dyn`-declared generic functions.
//!
//! Compilation is identical to static generics — the declared instantiations
//! compile the same monomorph wrappers. What changes is dispatch: a backend
//! (natively dynamic, or one that injects synthesized machinery behind a
//! feature flag) scans the registered candidates at call time and picks the
//! one whose type arguments match the incoming values.
//!
//! The matcher pre-ranks; the winning wrapper's `FromScript` conversions
//! remain authoritative — a conversion error from the chosen candidate
//! should fall through to try-calling the remaining candidates in
//! declaration order.

use crate::bridge::ScriptValue;
use crate::function::FunctionDescriptor;
use crate::types::{GenericParam, PrimitiveType, TypeDescriptor};

/// How well a runtime value fits a declared descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchQuality {
    /// Shapes disagree; this candidate cannot accept the value.
    No,
    /// Plausible after conversion (or shape unknown, e.g. an empty list).
    Coercible,
    /// Shapes agree directly.
    Exact,
}

/// Resolves [`TypeDescriptor::GenericParam`] references against one
/// candidate's concrete type arguments, without allocating.
pub struct GenericSubst<'a> {
    /// The function's declared generic parameters, in order.
    pub params: &'a [GenericParam<'a>],
    /// The candidate's concrete type arguments, in the same order.
    pub args: &'a [TypeDescriptor<'a>],
}

impl<'a> GenericSubst<'a> {
    fn resolve(&self, name: &str) -> Option<&'a TypeDescriptor<'a>> {
        self.params
            .iter()
            .position(|p| p.name == name)
            .and_then(|i| self.args.get(i))
    }
}

/// Ranks one runtime value against one declared descriptor.
///
/// A pure shape heuristic over the bridge vocabulary: integer widths are
/// erased to `I64` by the bridge (`FromScript` range-checks), containers
/// judge their first element (empty containers are `Coercible` — unknown,
/// ranking below an exact competitor), and userdata matches exactly only
/// when its [type tag](crate::OpaqueUserData::type_tag) names the same type.
#[allow(
    clippy::match_same_arms,
    reason = "the matcher is a POLICY TABLE — one documented rule per arm; merging same-bodied arms would obscure which rule grants which quality"
)]
pub fn value_matches_descriptor(
    value: &ScriptValue,
    desc: &TypeDescriptor<'_>,
    subst: &GenericSubst<'_>,
) -> MatchQuality {
    use MatchQuality::{Coercible, Exact, No};
    use PrimitiveType as P;
    use TypeDescriptor as T;

    if let T::GenericParam(name) = desc {
        return match subst.resolve(name) {
            Some(resolved) => value_matches_descriptor(value, resolved, subst),
            None => No,
        };
    }

    // The lifetime never changes what value shape matches.
    if let T::Borrowed { inner, .. } = desc {
        return value_matches_descriptor(value, inner, subst);
    }

    match (value, desc) {
        (ScriptValue::Unit, T::Unit) => Exact,
        (ScriptValue::Bool(_), T::Primitive(P::Bool)) => Exact,
        (
            ScriptValue::I64(_),
            T::Primitive(P::I8 | P::I16 | P::I32 | P::I64 | P::U8 | P::U16 | P::U32 | P::U64),
        ) => Exact,
        (ScriptValue::I64(_), T::Primitive(P::F32 | P::F64)) => Coercible,
        (ScriptValue::F64(_), T::Primitive(P::F32 | P::F64)) => Exact,
        (ScriptValue::String(_), T::String) => Exact,
        (ScriptValue::String(s), T::Primitive(P::Char)) if s.chars().count() == 1 => Coercible,
        (ScriptValue::Char(_), T::Primitive(P::Char)) => Exact,
        (ScriptValue::Char(_), T::String) => Coercible,
        (ScriptValue::Bytes(_), T::Bytes) => Exact,
        (ScriptValue::List(items), T::List(elem) | T::Array(elem, _)) => match items.first() {
            Some(first) => value_matches_descriptor(first, elem, subst),
            None => Coercible,
        },
        (ScriptValue::List(items), T::Tuple(elems)) => {
            if items.len() != elems.len() {
                return No;
            }
            let mut worst = Exact;
            for (item, elem) in items.iter().zip(elems.iter()) {
                let q = value_matches_descriptor(item, elem, subst);
                if q == No {
                    return No;
                }
                worst = worst.min(q);
            }
            worst
        }
        (ScriptValue::Map(entries), T::Map(key, val)) => {
            if !matches!(key, T::String) {
                return No;
            }
            match entries.first() {
                Some((_, first)) => value_matches_descriptor(first, val, subst),
                None => Coercible,
            }
        }
        (ScriptValue::Optional(None), T::Option(_)) => Exact,
        (ScriptValue::Optional(Some(inner)), T::Option(elem)) => {
            value_matches_descriptor(inner, elem, subst)
        }
        (ScriptValue::Optional(_), _) => No,
        (_, T::Option(elem)) => value_matches_descriptor(value, elem, subst),
        // Enum identity needs registry context (deliberately absent here);
        // try-call disambiguates between enum-typed candidates.
        (ScriptValue::Enum { .. }, T::Ref(_) | T::Instance { .. }) => Coercible,
        (ScriptValue::UserData(ud), T::Ref(id)) => match ud.type_tag() {
            Some(tag) if tag.const_eq(id) => Exact,
            Some(_) => No,
            None => Coercible,
        },
        (ScriptValue::UserData(_), T::Instance { .. }) => Coercible,
        _ => No,
    }
}

/// One scan candidate: a compiled monomorph registration.
pub struct DynCandidate<'a> {
    /// The candidate's concrete type arguments, in declaration order.
    pub type_args: &'a [TypeDescriptor<'a>],
    /// The function's descriptor (parameters + generic parameters).
    pub descriptor: &'a FunctionDescriptor<'a>,
}

/// Outcome of a candidate scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// A ranked winner: call this candidate first (fall through to the rest
    /// in declaration order if its conversions reject the values).
    Ranked(usize),
    /// No ranked winner; try-call every candidate in declaration order.
    TryCallOrder,
}

/// Scans candidates for the best match against the incoming values.
///
/// Pass 1 accepts only candidates where every parameter matches `Exact`;
/// pass 2 accepts `Exact | Coercible`. Within a pass the first candidate in
/// declaration order wins. No allocation.
pub fn resolve_dyn_candidate(
    values: &[ScriptValue],
    candidates: &[DynCandidate<'_>],
) -> Resolution {
    for minimum in [MatchQuality::Exact, MatchQuality::Coercible] {
        for (index, candidate) in candidates.iter().enumerate() {
            let params = candidate.descriptor.params;
            if params.len() != values.len() {
                continue;
            }
            let subst = GenericSubst {
                params: candidate.descriptor.generic_params,
                args: candidate.type_args,
            };
            let all_fit = params
                .iter()
                .zip(values.iter())
                .all(|(param, value)| value_matches_descriptor(value, param.ty, &subst) >= minimum);
            if all_fit {
                return Resolution::Ranked(index);
            }
        }
    }
    Resolution::TryCallOrder
}
