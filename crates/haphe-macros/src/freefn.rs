//! Expansion of `#[script]` on free functions: emits the function unchanged
//! plus a hidden type sharing its name that carries the descriptor via the
//! `ScriptFunction` trait. Because the type shares the function's name, it
//! travels with `use` imports and re-exports, so `registry!` can resolve the
//! descriptor through any path that names the function.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, Type};
use syn::spanned::Spanned;

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
    // return uses IntoScript — both with concrete types, no unsafe.
    let param_conversions: Vec<TokenStream> = param_info
        .iter()
        .enumerate()
        .map(|(i, (name, ty))| {
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

    let return_conversion = if info.return_ty.is_some() {
        quote! { ::haphe::IntoScript::into_script(#fn_ident(#(#call_args),*)) }
    } else {
        quote! { { #fn_ident(#(#call_args),*); ::haphe::ScriptValue::Unit } }
    };

    let has_type_params = !fn_generic_names.is_empty();
    let type_params: Vec<_> = item
        .sig
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(&tp.ident),
            _ => None,
        })
        .collect();

    // Build a generics set with only type params (no lifetimes) for the
    // hidden struct + trait impls. The function itself keeps its lifetimes.
    let struct_generics = {
        let mut g = item.sig.generics.clone();
        g.params = g
            .params
            .into_iter()
            .filter(|p| matches!(p, syn::GenericParam::Type(_)))
            .collect();
        g
    };
    let (impl_g, ty_g, where_c) = struct_generics.split_for_impl();

    let all_params_compatible = !has_type_params && item.sig.inputs.iter().all(|input| {
        let FnArg::Typed(pat_ty) = input else { return true };
        crate::bind::is_bridge_compatible_type(&pat_ty.ty)
    });
    let return_compatible = !has_type_params && info.return_ty.as_ref().is_none_or(|t| {
        !matches!(t, Type::Reference(_)) && crate::bind::is_bridge_compatible_type(t)
    });

    let can_bind = !info.is_async && cfgs.is_empty() && (has_type_params || (all_params_compatible && return_compatible));

    let bind_fn = if can_bind {
        quote! {
            #[automatically_derived]
            impl #impl_g ::haphe::ScriptBindFn for #ident #ty_g #where_c {
                fn bind<__B: ::haphe::FnBinder>(__binder: &mut __B) -> ::core::result::Result<(), __B::Error> {
                    __binder.function(
                        #exposed_name,
                        |__args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                            #(#param_conversions)*
                            ::core::result::Result::Ok(#return_conversion)
                        },
                    )
                }
            }
        }
    } else {
        TokenStream::new()
    };

    let struct_def = if has_type_params {
        quote! {
            #(#cfgs)*
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #vis struct #ident #impl_g #where_c {
                _marker: ::core::marker::PhantomData<(#(#type_params,)*)>,
            }
        }
    } else {
        quote! {
            #(#cfgs)*
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #vis struct #ident {}
        }
    };

    quote! {
        #item

        #struct_def

        #(#cfgs)*
        #[automatically_derived]
        impl #impl_g ::haphe::ScriptFunction for #ident #ty_g #where_c {
            const DESCRIPTOR: ::haphe::FunctionDescriptor<'static> = #descriptor;
        }

        #(#cfgs)*
        #bind_fn
    }
}
