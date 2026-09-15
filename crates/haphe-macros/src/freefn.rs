//! Expansion of `#[script]` on free functions: emits the function unchanged
//! plus a hidden type sharing its name that carries the descriptor via the
//! `ScriptFunction` trait. Because the type shares the function's name, it
//! travels with `use` imports and re-exports, so `registry!` can resolve the
//! descriptor through any path that names the function.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ItemFn, Pat, Type};

use crate::attrs::{Errors, parse_fn_args, strip_script_attrs};
use crate::fn_desc::{ReceiverShape, build_fn_info};
use crate::ty_map::TyCtx;

pub fn expand(mut item: ItemFn) -> TokenStream {
    let mut errors = Errors::default();
    let fn_args = parse_fn_args(&item.attrs, &mut errors, "a free function");
    strip_script_attrs(&mut item.attrs);

    for (flag, span) in [
        ("skip", fn_args.skip),
        ("constructor", fn_args.constructor),
        ("getter", fn_args.getter),
        ("setter", fn_args.setter.as_ref().map(|(_, s)| *s)),
    ] {
        if let Some(span) = span {
            errors.spanned(
                span,
                format!("`{flag}` only applies inside a `#[script] impl` block"),
            );
        }
    }

    let fn_generic_names: Vec<String> = item
        .sig
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(tp.ident.to_string()),
            _ => None,
        })
        .collect();
    let ctx = TyCtx {
        generic_params: &fn_generic_names,
        self_ty: None,
    };
    // Strips parameter attrs even on error paths.
    let info = build_fn_info(&mut item.sig, &fn_args, &item.attrs, &ctx, &mut errors);
    if let Some(info) = &info
        && info.receiver != ReceiverShape::None
    {
        errors.spanned(item.sig.span(), "free functions cannot take `self`");
    }

    if let Err(err) = errors.finish() {
        let compile_error = err.to_compile_error();
        return quote! { #item #compile_error };
    }
    let info = info.expect("no errors implies info was built");

    let vis = &item.vis;
    let ident = &item.sig.ident;
    let cfgs: Vec<_> = item
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();
    let descriptor = &info.descriptor;

    // Bridge: generate ScriptBindFn impl that wraps the fn in a tuple-arg
    // closure for the FnBinder.
    let fn_ident = &item.sig.ident;
    let exposed_name = &info.name;

    let param_info: Vec<(syn::Ident, Type)> = item
        .sig
        .inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(pat_ty) = input else {
                return None;
            };
            let Pat::Ident(pi) = pat_ty.pat.as_ref() else {
                return None;
            };
            // Strip outer references for the tuple type (bridge receives owned values).
            // &str → String (str is unsized).
            let ty = match pat_ty.ty.as_ref() {
                Type::Reference(r) => {
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
            Some((pi.ident.clone(), ty))
        })
        .collect();

    // Build call args — re-add & for params that were originally references.
    let call_args: Vec<TokenStream> = item
        .sig
        .inputs
        .iter()
        .filter_map(|input| {
            let FnArg::Typed(pat_ty) = input else {
                return None;
            };
            let Pat::Ident(pi) = pat_ty.pat.as_ref() else {
                return None;
            };
            let name = &pi.ident;
            if matches!(pat_ty.ty.as_ref(), Type::Reference(_)) {
                Some(quote! { &#name })
            } else {
                Some(quote! { #name })
            }
        })
        .collect();

    // Generate a wrapper that converts ScriptValue args to concrete types
    // and the result back to ScriptValue. Each param uses FromScript, the
    // return uses IntoScript — both with concrete types, no unsafe. For a
    // generic function, one wrapper is generated per `instantiate(...)`
    // declaration with the type parameters substituted.
    let type_params: Vec<syn::Ident> = item
        .sig
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(tp.ident.clone()),
            _ => None,
        })
        .collect();
    let has_type_params = !type_params.is_empty();

    let make_wrapper = |subst: &std::collections::HashMap<String, Type>| -> TokenStream {
        let conversions: Vec<TokenStream> = param_info
            .iter()
            .enumerate()
            .map(|(i, (name, ty))| {
                let ty = substitute_type_params(ty, subst);
                quote! {
                    let #name = <#ty as ::haphe::FromScript>::from_script(
                        __args.get(#i).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                    ).map_err(|__e| ::haphe::ScriptConvertError {
                        expected: stringify!(#ty),
                        got: __e.got,
                    })?;
                }
            })
            .collect();
        let turbofish = if subst.is_empty() {
            TokenStream::new()
        } else {
            let args: Vec<&Type> = type_params
                .iter()
                .map(|p| subst.get(&p.to_string()).expect("all params substituted"))
                .collect();
            quote! { ::<#(#args),*> }
        };
        let return_conversion = if info.return_ty.is_some() {
            quote! { ::haphe::IntoScript::into_script(#fn_ident #turbofish (#(#call_args),*)) }
        } else {
            quote! { { #fn_ident #turbofish (#(#call_args),*); ::haphe::ScriptValue::Unit } }
        };
        quote! {
            |__args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                #(#conversions)*
                ::core::result::Result::Ok(#return_conversion)
            }
        }
    };

    let compatible = |subst: &std::collections::HashMap<String, Type>| -> bool {
        param_info.iter().all(|(_, ty)| {
            crate::bind::is_bridge_compatible_type(&substitute_type_params(ty, subst))
        }) && info.return_ty.as_ref().is_none_or(|t| {
            let t = substitute_type_params(t, subst);
            !matches!(t, Type::Reference(_)) && crate::bind::is_bridge_compatible_type(&t)
        })
    };

    let empty_subst = std::collections::HashMap::new();
    let (can_bind, bind_body) = if has_type_params {
        let substs: Vec<std::collections::HashMap<String, Type>> = fn_args
            .instantiate
            .iter()
            .map(|(types, _)| {
                type_params
                    .iter()
                    .map(|p| p.to_string())
                    .zip(types.iter().cloned())
                    .collect()
            })
            .collect();
        let can = !info.is_async
            && cfgs.is_empty()
            && !substs.is_empty()
            && substs.iter().all(&compatible);
        let registrations: Vec<TokenStream> = fn_args
            .instantiate
            .iter()
            .zip(&substs)
            .map(|((types, span), subst)| {
                let wrapper = make_wrapper(subst);
                quote_spanned! {*span=>
                    __binder.function(
                        #exposed_name,
                        &[#( <#types as ::haphe::HapheType>::DESCRIPTOR ),*],
                        #wrapper,
                    )?;
                }
            })
            .collect();
        (
            can,
            quote! { #(#registrations)* ::core::result::Result::Ok(()) },
        )
    } else {
        let can = !info.is_async && cfgs.is_empty() && compatible(&empty_subst);
        let wrapper = make_wrapper(&empty_subst);
        (
            can,
            quote! { __binder.function(#exposed_name, &[], #wrapper) },
        )
    };

    let bind_fn = if can_bind {
        quote! {
            #[automatically_derived]
            impl ::haphe::ScriptBindFn for #ident {
                fn bind<__B: ::haphe::FnBinder>(__binder: &mut __B) -> ::core::result::Result<(), __B::Error> {
                    #bind_body
                }
            }
        }
    } else {
        TokenStream::new()
    };

    quote! {
        #item

        #(#cfgs)*
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #ident {}

        #(#cfgs)*
        #[automatically_derived]
        impl ::haphe::ScriptFunction for #ident {
            const DESCRIPTOR: ::haphe::FunctionDescriptor<'static> = #descriptor;
        }

        #(#cfgs)*
        #bind_fn
    }
}

/// Replaces bare type-parameter paths (`T`, `Vec<T>`) with concrete types.
fn substitute_type_params(ty: &Type, subst: &std::collections::HashMap<String, Type>) -> Type {
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
