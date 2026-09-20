//! Bridgeability gates and type utilities: syntactic whitelists deciding
//! which signatures get bridge codegen, plus shared type transforms.

use syn::Type;

/// Checks whether a type is bridge-compatible: a primitive, `&str`, or a
/// reference to a primitive. Used to gate bridge codegen for fields/methods.
pub fn is_bridge_compatible_type(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) => {
            // &str is fine (maps to String), &f64 is fine
            if let Type::Path(p) = r.elem.as_ref()
                && p.path.is_ident("str")
            {
                return true;
            }
            is_bridge_primitive(&r.elem)
        }
        _ => is_bridge_primitive(ty),
    }
}

/// Peels transparent carrier wrappers, recursively, so gates judge the
/// carried type — an unsized carried type is judged as its owned form. Which
/// types count as carriers is std knowledge and lives in
/// [`crate::std_types`]; this is only the peeling mechanism.
pub(crate) fn strip_carriers(ty: &Type) -> Type {
    match crate::std_types::carrier_parts(ty) {
        Some((_, _, inner)) => strip_carriers(&crate::std_types::owned_form(inner)),
        None => ty.clone(),
    }
}

/// Rewrites signature lifetimes for embedding in generated code, where the
/// signature's lifetime parameters are not in scope: reference lifetimes are
/// elided and path-argument lifetimes become the anonymous `'_`, letting
/// inference carry them — sound in the let-binding positions the generated
/// conversions use, where the constructed owned value coerces covariantly.
pub(crate) fn anonymize_lifetimes(ty: &Type) -> Type {
    struct Anonymize;
    impl syn::visit_mut::VisitMut for Anonymize {
        fn visit_type_reference_mut(&mut self, r: &mut syn::TypeReference) {
            r.lifetime = None;
            syn::visit_mut::visit_type_reference_mut(self, r);
        }
        fn visit_lifetime_mut(&mut self, lt: &mut syn::Lifetime) {
            if lt.ident != "static" && lt.ident != "_" {
                *lt = syn::Lifetime::new("'_", lt.apostrophe);
            }
        }
    }
    let mut ty = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut Anonymize, &mut ty);
    ty
}

/// Whether the type mentions a lifetime in path arguments (`Cow<'a, str>`),
/// after reference stripping. Such types cannot be embedded in the
/// trait-presence dispatch blocks — associated type definitions accept
/// neither `'_` nor a signature lifetime.
pub(crate) fn mentions_path_lifetime(ty: &Type) -> bool {
    struct FindLifetime(bool);
    impl syn::visit::Visit<'_> for FindLifetime {
        fn visit_lifetime(&mut self, lt: &syn::Lifetime) {
            if lt.ident != "static" {
                self.0 = true;
            }
        }
        fn visit_type_reference(&mut self, r: &syn::TypeReference) {
            // Reference lifetimes are stripped before embedding; only
            // recurse into the referent.
            syn::visit::visit_type(self, &r.elem);
        }
    }
    let mut visitor = FindLifetime(false);
    syn::visit::Visit::visit_type(&mut visitor, ty);
    visitor.0
}

/// Checks (syntactically) whether a type is a primitive that implements
/// `IntoScript + FromScript` — through any transparent carrier. Covers the
/// blanket impls in the bridge module.
pub(crate) fn is_bridge_primitive(ty: &Type) -> bool {
    let stripped = strip_carriers(ty);
    let Some(ident) = crate::std_types::std_ident(&stripped) else {
        return false;
    };
    crate::std_types::BRIDGE_PRIMITIVES.contains(&ident.to_string().as_str())
}

/// Std types with string/tuple bridge representations (see
/// [`crate::std_types::SEMANTIC_VALUE_TYPES`]), spelled bare or through an
/// explicit `std`/`core` path (`std::path::PathBuf`). Other crates' paths
/// never match.
pub(crate) fn is_bridge_std_semantic(ty: &Type) -> bool {
    let Some(ident) = crate::std_types::std_ident(ty) else {
        return false;
    };
    crate::std_types::SEMANTIC_VALUE_TYPES.contains(&ident.to_string().as_str())
}

pub(crate) fn is_generic_type_param(ty: &Type, params: &[String]) -> bool {
    if let Type::Path(p) = ty
        && p.qself.is_none()
        && let Some(ident) = p.path.get_ident()
    {
        params.iter().any(|g| ident == g)
    } else {
        false
    }
}

/// Owned, bare single-ident path type that isn't a known primitive: its
/// bridgeability can't be judged syntactically (it may be a transparent
/// primitive newtype), so registration goes through compile-time trait
/// dispatch instead.
pub(crate) fn needs_bridge_dispatch(ty: &Type, generic_params: &[String]) -> bool {
    if is_bridge_primitive(ty) || is_generic_type_param(ty, generic_params) {
        return false;
    }
    // Fields must be owned; references never dispatch.
    !is_reference(ty) && is_dispatchable_path(ty)
}

/// A type whose VALUES cross the bridge: a bridge primitive, a std semantic
/// type, or a standard container (`Vec`, `Option`, sets, deques,
/// string-keyed maps, tuples) of such types — each spelled bare or through
/// an explicit `std`/`alloc`/`core` path — mirroring the blanket
/// `FromScript`/`IntoScript` impls. References are NOT value types at any
/// depth (`Vec<&str>` has no conversion); callers strip one outer reference
/// before asking. Used to gate wrapper emission for free functions, methods,
/// properties, and fields (with trait-presence dispatch covering the rest).
pub fn is_bridge_value_type(ty: &Type) -> bool {
    if is_reference(ty) {
        return false;
    }
    if is_bridge_primitive(ty) || is_bridge_std_semantic(ty) {
        return true;
    }
    let stripped = strip_carriers(ty);
    let ty = &stripped;
    if is_bridge_std_semantic(ty) {
        return true;
    }
    match ty {
        Type::Tuple(tuple) => tuple.elems.iter().all(is_bridge_value_type),
        Type::Array(array) => is_bridge_value_type(&array.elem),
        Type::Path(p) => {
            if p.qself.is_some() || !crate::std_types::is_std_path(&p.path) {
                return false;
            }
            let Some(last) = p.path.segments.last() else {
                return false;
            };
            let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
                return false;
            };
            let type_args: Vec<&Type> = args
                .args
                .iter()
                .filter_map(|a| match a {
                    syn::GenericArgument::Type(t) => Some(t),
                    _ => None,
                })
                .collect();
            let name = last.ident.to_string();
            if crate::std_types::UNARY_CONTAINERS.contains(&name.as_str()) {
                type_args.len() == 1 && is_bridge_value_type(type_args[0])
            } else if crate::std_types::STRING_MAPS.contains(&name.as_str()) {
                type_args.len() == 2
                    && matches!(type_args[0], Type::Path(k) if k.path.is_ident("String"))
                    && is_bridge_value_type(type_args[1])
            } else {
                false
            }
        }
        _ => false,
    }
}

/// An owned bare single-ident path type (or one behind a single reference)
/// whose bridgeability is decided by trait presence rather than the
/// syntactic whitelist.
pub fn is_dispatchable_path(ty: &Type) -> bool {
    // Dispatch blocks embed the type in associated type definitions, which
    // cannot carry a signature lifetime.
    if mentions_path_lifetime(ty) {
        return false;
    }
    let stripped = strip_carriers(&strip_ref(ty));
    let Type::Path(p) = &stripped else {
        return false;
    };
    p.qself.is_none() && p.path.get_ident().is_some()
}

/// Replaces bare type-parameter paths (`T`, `Vec<T>`) with concrete types.
pub(crate) fn substitute_type_params(
    ty: &Type,
    subst: &std::collections::HashMap<String, Type>,
) -> Type {
    struct Replace<'a>(&'a std::collections::HashMap<String, Type>);
    impl syn::visit_mut::VisitMut for Replace<'_> {
        fn visit_type_mut(&mut self, ty: &mut Type) {
            if let Type::Path(p) = ty
                && p.qself.is_none()
                && let Some(ident) = p.path.get_ident()
                && let Some(concrete) = self.0.get(&ident.to_string())
            {
                *ty = concrete.clone();
                return;
            }
            syn::visit_mut::visit_type_mut(self, ty);
        }
    }
    let mut ty = ty.clone();
    syn::visit_mut::VisitMut::visit_type_mut(&mut Replace(subst), &mut ty);
    ty
}

/// Strips one outer reference for use as a `FromScript` target, and
/// anonymizes signature lifetimes so the result can be embedded in generated
/// code (see [`anonymize_lifetimes`]).
pub(crate) fn strip_ref(ty: &Type) -> Type {
    let stripped = match ty {
        Type::Reference(r) => {
            // &str → String (str is unsized, can't be a FromScript target)
            if let Type::Path(p) = r.elem.as_ref()
                && p.path.is_ident("str")
            {
                syn::parse_quote!(String)
            } else {
                (*r.elem).clone()
            }
        }
        other => other.clone(),
    };
    anonymize_lifetimes(&stripped)
}

pub(crate) fn is_reference(ty: &Type) -> bool {
    matches!(ty, Type::Reference(_))
}

/// The span of a reference nested INSIDE a composite type — one outer
/// reference is legitimate (wrappers strip it), but `Vec<&str>` has no
/// conversion at any bridge surface. `None` when no nested reference exists.
pub(crate) fn nested_reference_span(ty: &Type) -> Option<proc_macro2::Span> {
    struct Find(Option<proc_macro2::Span>);
    impl syn::visit::Visit<'_> for Find {
        fn visit_type_reference(&mut self, r: &syn::TypeReference) {
            if self.0.is_none() {
                self.0 = Some(r.and_token.span);
            }
            syn::visit::visit_type_reference(self, r);
        }
        fn visit_type_fn_ptr(&mut self, _: &syn::TypeFnPtr) {
            // Callback signatures have their own reference rules and
            // diagnostics; don't descend.
        }
    }
    let inner = strip_ref(ty);
    let mut find = Find(None);
    syn::visit::Visit::visit_type(&mut find, &inner);
    find.0
}
