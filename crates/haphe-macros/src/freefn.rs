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
    let mut fn_args = parse_fn_args(&item.attrs, &mut errors, "a free function");
    strip_script_attrs(&mut item.attrs);

    fn_args.inject_bare_dyn_defaults(item.sig.generics.type_params().count());

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
            // Strip outer references (bridge receives owned values, &str →
            // String) and anonymize signature lifetimes for embedding.
            Some((pi.ident.clone(), crate::bind::strip_ref(&pat_ty.ty)))
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

    let make_async_wrapper = |subst: &std::collections::HashMap<String, Type>| -> TokenStream {
        let conversions: Vec<TokenStream> = param_info
            .iter()
            .enumerate()
            .map(|(i, (name, ty))| {
                let ty = substitute_type_params(ty, subst);
                quote! {
                    let #name = <#ty as ::haphe::FromScript>::from_script(
                        __args.get(#i).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                    )?;
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
            quote! { ::haphe::IntoScript::into_script(#fn_ident #turbofish (#(#call_args),*).await) }
        } else {
            quote! { { #fn_ident #turbofish (#(#call_args),*).await; ::haphe::ScriptValue::Unit } }
        };
        quote! {
            (|__args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                ::std::boxed::Box::pin(async move {
                    #(#conversions)*
                    ::core::result::Result::Ok(#return_conversion)
                })
            }) as for<'a> fn(&'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>
        }
    };

    let compatible = |subst: &std::collections::HashMap<String, Type>| -> bool {
        param_info
            .iter()
            .all(|(_, ty)| crate::bind::is_bridge_value_type(&substitute_type_params(ty, subst)))
            && info.return_ty.as_ref().is_none_or(|t| {
                let t = substitute_type_params(t, subst);
                !matches!(t, Type::Reference(_)) && crate::bind::is_bridge_value_type(&t)
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
        let is_dyn = fn_args.dyn_dispatch.is_some();
        let can = cfgs.is_empty() && !substs.is_empty() && substs.iter().all(&compatible);
        let registrations: Vec<TokenStream> = fn_args
            .instantiate
            .iter()
            .zip(&substs)
            .map(|((types, span), subst)| {
                let type_args = quote! { &[#( <#types as ::haphe::HapheType>::DESCRIPTOR ),*] };
                if is_dyn && info.is_async {
                    let wrapper = make_async_wrapper(subst);
                    quote_spanned! {*span=>
                        __binder.function_dyn_async(
                            &<#ident as ::haphe::ScriptFunction>::DESCRIPTOR,
                            #type_args,
                            #wrapper,
                        )?;
                    }
                } else if is_dyn {
                    let wrapper = make_wrapper(subst);
                    quote_spanned! {*span=>
                        __binder.function_dyn(
                            &<#ident as ::haphe::ScriptFunction>::DESCRIPTOR,
                            #type_args,
                            #wrapper,
                        )?;
                    }
                } else if info.is_async {
                    let wrapper = make_async_wrapper(subst);
                    quote_spanned! {*span=>
                        __binder.function_async(
                            #exposed_name,
                            #type_args,
                            #wrapper,
                        )?;
                    }
                } else {
                    let wrapper = make_wrapper(subst);
                    quote_spanned! {*span=>
                        __binder.function(
                            #exposed_name,
                            #type_args,
                            #wrapper,
                        )?;
                    }
                }
            })
            .collect();
        (
            can,
            quote! { #(#registrations)* ::core::result::Result::Ok(()) },
        )
    } else {
        let whitelisted = compatible(&empty_subst);
        let dispatch_eligible = !whitelisted
            && param_info.iter().all(|(_, ty)| {
                crate::bind::is_bridge_compatible_type(ty) || crate::bind::is_dispatchable_path(ty)
            })
            && info.return_ty.as_ref().is_none_or(|t| {
                !matches!(t, Type::Reference(_))
                    && (crate::bind::is_bridge_compatible_type(t)
                        || crate::bind::is_dispatchable_path(t))
            });
        let can = cfgs.is_empty() && (whitelisted || (!info.is_async && dispatch_eligible));
        let body = if info.is_async && whitelisted {
            // Async free functions register through the async channel; the
            // boxed future borrows the argument slice.
            let conversions: Vec<TokenStream> = param_info
                .iter()
                .enumerate()
                .map(|(i, (name, ty))| {
                    quote! {
                        let #name = <#ty as ::haphe::FromScript>::from_script(
                            __args.get(#i).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                        )?;
                    }
                })
                .collect();
            let return_conversion = if info.return_ty.is_some() {
                quote! { ::haphe::IntoScript::into_script(#fn_ident(#(#call_args),*).await) }
            } else {
                quote! { { #fn_ident(#(#call_args),*).await; ::haphe::ScriptValue::Unit } }
            };
            quote! {
                __binder.function_async(
                    #exposed_name,
                    &[],
                    (|__args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                        ::std::boxed::Box::pin(async move {
                            #(#conversions)*
                            ::core::result::Result::Ok(#return_conversion)
                        })
                    }) as for<'a> fn(&'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                )
            }
        } else if whitelisted {
            let wrapper = make_wrapper(&empty_subst);
            quote! { __binder.function(#exposed_name, &[], #wrapper) }
        } else {
            // Types the whitelist can't judge (e.g. transparent primitive
            // newtypes): registration is decided by compile-time
            // trait-presence dispatch — real registration when the bridge
            // bounds hold, no-op otherwise.
            {
                let param_is_ref: Vec<bool> = item
                    .sig
                    .inputs
                    .iter()
                    .filter_map(|input| match input {
                        syn::FnArg::Typed(pat_ty) => {
                            Some(matches!(pat_ty.ty.as_ref(), Type::Reference(_)))
                        }
                        _ => None,
                    })
                    .collect();
                gen_dispatched_fn_registration(
                    ident,
                    fn_ident,
                    exposed_name,
                    &param_info,
                    &param_is_ref,
                    &info,
                )
            }
        };
        (can, body)
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

use crate::bind::substitute_type_params;

/// A free-fn registration decided by compile-time trait-presence dispatch
/// (autoref specialization): registers when every stripped param implements
/// `FromScript` and the return converts to `ScriptValue`, no-ops otherwise.
/// Mirrors `bind::gen_dispatched_method_registration`; the hidden descriptor
/// struct doubles as the dispatch carrier. Non-generic functions only.
fn gen_dispatched_fn_registration(
    carrier: &syn::Ident,
    fn_ident: &syn::Ident,
    exposed_name: &str,
    param_info: &[(syn::Ident, Type)],
    param_is_ref: &[bool],
    info: &crate::fn_desc::FnInfo,
) -> TokenStream {
    // param_info types are already stripped of outer references.
    let stripped: Vec<Type> = param_info.iter().map(|(_, t)| t.clone()).collect();
    let p_assoc: Vec<syn::Ident> = (0..stripped.len())
        .map(|i| quote::format_ident!("__P{i}"))
        .collect();
    let p_vars: Vec<syn::Ident> = (0..stripped.len())
        .map(|i| quote::format_ident!("__p{i}"))
        .collect();
    let call_args: Vec<TokenStream> = param_is_ref
        .iter()
        .zip(&p_vars)
        .map(|(is_ref, v)| {
            if *is_ref {
                quote! { &#v }
            } else {
                quote! { #v }
            }
        })
        .collect();
    let ret_ty: TokenStream = match &info.return_ty {
        Some(t) => quote! { #t },
        None => quote! { () },
    };
    let idx: Vec<usize> = (0..stripped.len()).collect();

    quote! {
        {
            #[allow(non_camel_case_types)]
            trait __Call {
                #( type #p_assoc; )*
                type __R;
                fn __invoke(#( #p_vars: Self::#p_assoc ),*) -> Self::__R;
            }
            impl __Call for #carrier {
                #( type #p_assoc = #stripped; )*
                type __R = #ret_ty;
                #[allow(unused_variables)]
                fn __invoke(#( #p_vars: #stripped ),*) -> #ret_ty {
                    #fn_ident(#(#call_args),*)
                }
            }
            #[allow(non_camel_case_types)]
            trait __Go {
                fn __haphe_bind_fn<__B: ::haphe::FnBinder>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error>;
            }
            impl<__T> __Go for &::haphe::BridgeProbe<__T>
            where
                __T: __Call,
                #( __T::#p_assoc: ::haphe::FromScript, )*
                ::haphe::ScriptValue: ::core::convert::From<__T::__R>,
            {
                fn __haphe_bind_fn<__B: ::haphe::FnBinder>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error> {
                    __b.function(
                        #exposed_name,
                        &[],
                        (|__args: &[::haphe::ScriptValue]|
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                            #(
                                let #p_vars = <__T::#p_assoc as ::haphe::FromScript>::from_script(
                                    __args.get(#idx).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                            )*
                            ::core::result::Result::Ok(::haphe::ScriptValue::from(
                                __T::__invoke(#( #p_vars ),*)
                            ))
                        }) as fn(&[::haphe::ScriptValue])
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError>,
                    )
                }
            }
            #[allow(unused_imports)]
            use ::haphe::SkipBindFn as _;
            (&&::haphe::BridgeProbe::<#carrier>(::core::marker::PhantomData))
                .__haphe_bind_fn(__binder)
        }
    }
}
