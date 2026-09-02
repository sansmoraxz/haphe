//! Maps a `syn::Type` to an expression of type `TypeDescriptor<'static>`.
//!
//! Types that don't mention a declared generic parameter resolve through
//! `<Ty as ::haphe::HapheType>::DESCRIPTOR`, letting the compiler handle
//! aliases and re-exports and produce trait-bound errors for non-describable
//! types. Types that *do* mention a declared parameter must be folded
//! syntactically (a `const` cannot depend on the surrounding type's generics),
//! which only works inside built-in container shapes.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;
use syn::{GenericArgument, PathArguments, Type};

/// Context for descriptor mapping.
#[derive(Default)]
pub struct TyCtx<'a> {
    /// Generic type parameter names declared on the containing type.
    pub generic_params: &'a [String],
    /// Concrete replacement for `Self`, when mapping inside an impl block or
    /// a `traits(...)` argument.
    pub self_ty: Option<&'a Type>,
}

struct ReplaceSelf<'a>(&'a Type);

impl VisitMut for ReplaceSelf<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(p) = ty
            && p.qself.is_none()
            && p.path.is_ident("Self")
        {
            *ty = self.0.clone();
            return;
        }
        syn::visit_mut::visit_type_mut(self, ty);
    }
}

/// Erases reference lifetimes (`&'a str` → `&str`) so descriptor expressions
/// emitted into `const` items never mention in-scope lifetimes.
struct EraseRefLifetimes;

impl VisitMut for EraseRefLifetimes {
    fn visit_type_reference_mut(&mut self, r: &mut syn::TypeReference) {
        r.lifetime = None;
        syn::visit_mut::visit_type_reference_mut(self, r);
    }
}

/// Substitutes `Self` with the concrete self type, if one is known.
pub fn substitute_self(ty: &Type, ctx: &TyCtx) -> Type {
    let mut ty = ty.clone();
    if let Some(self_ty) = ctx.self_ty {
        ReplaceSelf(self_ty).visit_type_mut(&mut ty);
    }
    EraseRefLifetimes.visit_type_mut(&mut ty);
    ty
}

/// Whether `ty` mentions any declared generic parameter.
///
/// A parameter is mentioned only when it appears as the first segment of a
/// plain path (`T`, `T::Assoc`, `Vec<T>`), never as a namespaced item that
/// merely shares the name (`some_mod::T`).
fn mentions_generic_param(ty: &Type, params: &[String]) -> bool {
    struct Mentions<'p> {
        params: &'p [String],
        found: bool,
    }
    impl Visit<'_> for Mentions<'_> {
        fn visit_type_path(&mut self, tp: &syn::TypePath) {
            if tp.qself.is_none()
                && let Some(first) = tp.path.segments.first()
                && tp.path.leading_colon.is_none()
                && self.params.iter().any(|p| first.ident == p)
            {
                self.found = true;
                return;
            }
            syn::visit::visit_type_path(self, tp);
        }
    }
    if params.is_empty() {
        return false;
    }
    let mut visitor = Mentions {
        params,
        found: false,
    };
    visitor.visit_type(ty);
    visitor.found
}

/// Rejects `fn` pointer types with reference parameters (or explicit
/// higher-ranked lifetimes): such types are higher-ranked and no blanket
/// [`HapheType`] implementation can cover them, which would otherwise surface
/// as a cryptic "implementation is not general enough" error.
fn reject_higher_ranked_fn_ptrs(ty: &Type) -> syn::Result<()> {
    struct FindHrtb(Option<syn::Error>);
    impl Visit<'_> for FindHrtb {
        fn visit_type_fn_ptr(&mut self, f: &syn::TypeFnPtr) {
            if self.0.is_none()
                && (f.lifetimes.is_some()
                    || f.inputs
                        .iter()
                        .any(|arg| matches!(arg.ty, Type::Reference(_))))
            {
                self.0 = Some(syn::Error::new(
                    f.span(),
                    "callback types with reference parameters are not supported; \
                     take owned values instead (e.g. `String` rather than `&str`)",
                ));
            }
            syn::visit::visit_type_fn_ptr(self, f);
        }
    }
    let mut visitor = FindHrtb(None);
    visitor.visit_type(ty);
    match visitor.0 {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Returns an expression evaluating to `TypeDescriptor<'static>` for `ty`.
pub fn descriptor_expr(ty: &Type, ctx: &TyCtx) -> syn::Result<TokenStream> {
    let ty = substitute_self(ty, ctx);
    reject_higher_ranked_fn_ptrs(&ty)?;
    if !mentions_generic_param(&ty, ctx.generic_params) {
        return Ok(quote_spanned! {ty.span()=>
            <#ty as ::haphe::HapheType>::DESCRIPTOR
        });
    }
    generic_fold(&ty, ctx)
}

/// Syntactic fold for types that mention a declared generic parameter.
fn generic_fold(ty: &Type, ctx: &TyCtx) -> syn::Result<TokenStream> {
    // Anything not mentioning a declared parameter resolves through the trait,
    // at any nesting depth (e.g. the `i32` in `HashMap<i32, T>`).
    if !mentions_generic_param(ty, ctx.generic_params) {
        return Ok(quote_spanned! {ty.span()=>
            <#ty as ::haphe::HapheType>::DESCRIPTOR
        });
    }
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            // A bare declared parameter: `T`.
            if let Some(ident) = p.path.get_ident()
                && ctx.generic_params.iter().any(|g| ident == g)
            {
                let name = ident.to_string();
                return Ok(quote! { ::haphe::TypeDescriptor::GenericParam(#name) });
            }
            let last = p.path.segments.last().expect("path has segments");
            // Only bare names (`Vec<T>`, assumed to be the std type) or
            // explicit std paths (`std::vec::Vec<T>`) fold as built-in
            // containers; another crate's `Vec` cannot be told apart
            // syntactically, so namespaced paths are rejected below.
            let is_builtin_path = p.path.segments.len() == 1
                || matches!(
                    p.path.segments.first().map(|seg| seg.ident.to_string()),
                    Some(ref first) if matches!(first.as_str(), "std" | "alloc" | "core")
                );
            let args = if is_builtin_path {
                generic_type_args(&last.arguments)
            } else {
                Vec::new()
            };
            match (last.ident.to_string().as_str(), args.as_slice()) {
                ("Option", [inner]) => {
                    let inner = generic_fold(inner, ctx)?;
                    Ok(quote! { ::haphe::TypeDescriptor::Option(&#inner) })
                }
                ("Vec", [inner]) => {
                    let inner = generic_fold(inner, ctx)?;
                    Ok(quote! { ::haphe::TypeDescriptor::List(&#inner) })
                }
                ("Box", [inner]) => generic_fold(inner, ctx),
                ("HashMap" | "BTreeMap", [k, v]) => {
                    let k = generic_fold(k, ctx)?;
                    let v = generic_fold(v, ctx)?;
                    Ok(quote! { ::haphe::TypeDescriptor::Map(&#k, &#v) })
                }
                ("Result", [t, e]) => {
                    let t = generic_fold(t, ctx)?;
                    let e = generic_fold(e, ctx)?;
                    Ok(quote! { ::haphe::TypeDescriptor::Result(&#t, &#e) })
                }
                _ => Err(syn::Error::new(
                    ty.span(),
                    "generic parameters may only appear directly or inside built-in containers \
                     (`Option`, `Vec`, `Box`, slices, arrays, tuples, `HashMap`, `BTreeMap`, \
                     `Result`, references) in this position",
                )),
            }
        }
        Type::Reference(r) => generic_fold(&r.elem, ctx),
        Type::Slice(s) => {
            let inner = generic_fold(&s.elem, ctx)?;
            Ok(quote! { ::haphe::TypeDescriptor::List(&#inner) })
        }
        Type::Array(a) => {
            let inner = generic_fold(&a.elem, ctx)?;
            let len = &a.len;
            Ok(quote! { ::haphe::TypeDescriptor::Array(&#inner, #len) })
        }
        Type::Tuple(t) if t.elems.is_empty() => Ok(quote! { ::haphe::TypeDescriptor::Unit }),
        Type::Tuple(t) => {
            let elems = t
                .elems
                .iter()
                .map(|e| generic_fold(e, ctx))
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote! { ::haphe::TypeDescriptor::Tuple(&[#(#elems),*]) })
        }
        Type::Paren(p) => generic_fold(&p.elem, ctx),
        _ => Err(syn::Error::new(
            ty.span(),
            "generic parameters may only appear directly or inside built-in containers in this position",
        )),
    }
}

/// Extracts the type arguments of a path segment (`Vec<T>` → `[T]`).
fn generic_type_args(args: &PathArguments) -> Vec<&Type> {
    match args {
        PathArguments::AngleBracketed(ab) => ab
            .args
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Ownership tokens for a parameter or return type, derived syntactically.
pub fn ownership_expr(ty: &Type, clone_override: bool) -> TokenStream {
    if clone_override {
        return quote! { ::haphe::Ownership::Clone };
    }
    match ty {
        Type::Reference(r) if r.mutability.is_some() => quote! { ::haphe::Ownership::RefMut },
        Type::Reference(_) => quote! { ::haphe::Ownership::Ref },
        Type::Paren(p) => ownership_expr(&p.elem, false),
        _ => quote! { ::haphe::Ownership::Owned },
    }
}
