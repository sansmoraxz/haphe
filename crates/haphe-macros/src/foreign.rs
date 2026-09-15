//! Expansion of `#[script(foreign)]` on traits: the trait declares signatures
//! Rust calls out to, with bodies supplied by the embedding host. Emits the
//! trait unchanged plus a `{Trait}Handle` struct that implements it by
//! dispatching every call through a boxed `ForeignCaller`, and carries the
//! interface descriptor via the `ScriptForeign` trait.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{FnArg, ItemTrait, Pat, ReturnType, TraitItem, Type};

use crate::attrs::{
    Errors, ThreadSafetyKind, extract_doc, option_str_tokens, parse_fn_args, parse_foreign_args,
    strip_script_attrs,
};
use crate::fn_desc::{ReceiverShape, build_fn_info, strip_param_script_attrs};
use crate::ty_map::TyCtx;

/// `Result<T, E>` split syntactically, mirroring the built-in-container rules
/// of `ty_map` (bare name, or a `std`/`core` path).
fn result_types(ty: &Type) -> Option<(&Type, &Type)> {
    let Type::Path(p) = ty else { return None };
    if p.qself.is_some() {
        return None;
    }
    let is_builtin_path = p.path.segments.len() == 1
        || matches!(
            p.path.segments.first().map(|seg| seg.ident.to_string()),
            Some(ref first) if matches!(first.as_str(), "std" | "core")
        );
    if !is_builtin_path {
        return None;
    }
    let last = p.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    });
    match (types.next(), types.next(), types.next()) {
        (Some(ok), Some(err), None) => Some((ok, err)),
        _ => None,
    }
}

/// Strips helper attributes from every method so error paths never re-emit
/// them (they would be unresolved attributes on plain trait items).
fn strip_method_attrs(item: &mut ItemTrait) {
    for ti in &mut item.items {
        if let TraitItem::Fn(m) = ti {
            strip_script_attrs(&mut m.attrs);
            strip_param_script_attrs(&mut m.sig);
        }
    }
}

pub fn expand(mut item: ItemTrait) -> TokenStream {
    let mut errors = Errors::default();
    let args = parse_foreign_args(&item.attrs, &mut errors);
    strip_script_attrs(&mut item.attrs);

    if args.foreign.is_none() {
        strip_method_attrs(&mut item);
        let error = syn::Error::new_spanned(
            &item.ident,
            "`#[script]` on a trait requires the `foreign` flag \
             (`#[script(foreign)]`): traits describe host-supplied functions",
        )
        .to_compile_error();
        return quote! { #item #error };
    }

    if let Some((kind, span)) = args.thread_safety
        && kind != ThreadSafetyKind::None
    {
        errors.spanned(
            span,
            "foreign handles are not thread-safe; only `thread_safety = none` is supported",
        );
    }

    if let Some(token) = &item.unsafety {
        errors.spanned(token.span(), "foreign traits cannot be `unsafe`");
    }
    if let Some(token) = &item.modifiers.auto_token {
        errors.spanned(token.span(), "foreign traits cannot be `auto`");
    }
    if !item.generics.params.is_empty() {
        errors.spanned(
            item.generics.span(),
            "foreign traits cannot have generic parameters",
        );
    }
    if let Some(where_clause) = &item.generics.where_clause {
        errors.spanned(
            where_clause.span(),
            "foreign traits cannot have `where` clauses",
        );
    }
    if !item.supertraits.is_empty() {
        errors.spanned(
            item.supertraits.span(),
            "foreign traits cannot have supertraits",
        );
    }

    let ctx = TyCtx::default();
    let mut fn_descs = Vec::new();
    let mut impl_methods = Vec::new();
    let mut has_async = false;

    for ti in &mut item.items {
        let TraitItem::Fn(m) = ti else {
            errors.spanned(
                ti.span(),
                "foreign traits may only contain methods (no associated types or constants)",
            );
            continue;
        };
        let fn_args = parse_fn_args(&m.attrs, &mut errors, "a foreign method");
        for (flag, span) in [
            ("skip", fn_args.skip),
            ("constructor", fn_args.constructor),
            ("getter", fn_args.getter),
            ("setter", fn_args.setter.as_ref().map(|(_, s)| *s)),
        ] {
            if let Some(span) = span {
                errors.spanned(span, format!("`{flag}` does not apply to a foreign method"));
            }
        }
        if let Some(cfg) = m.attrs.iter().find(|a| a.path().is_ident("cfg")) {
            errors.spanned(cfg.span(), "`#[cfg]` is not supported on foreign methods");
        }
        if let Some(default) = &m.default {
            errors.spanned(
                default.span(),
                "foreign methods cannot have default bodies — the host supplies the implementation",
            );
        }
        for param in &m.sig.generics.params {
            if matches!(
                param,
                syn::GenericParam::Type(_) | syn::GenericParam::Lifetime(_)
            ) {
                errors.spanned(
                    param.span(),
                    "foreign methods cannot have generic parameters",
                );
            }
        }
        for input in &m.sig.inputs {
            if let FnArg::Typed(pat_ty) = input
                && matches!(pat_ty.ty.as_ref(), Type::Reference(_))
            {
                errors.spanned(
                    pat_ty.ty.span(),
                    "foreign methods take owned parameters — values cross into the host \
                     runtime and cannot be lent (e.g. take `String` rather than `&str`)",
                );
            }
        }

        let (return_ty, err_ty) = match &m.sig.output {
            ReturnType::Default => (None, None),
            ReturnType::Type(_, ty) => {
                if matches!(ty.as_ref(), Type::Reference(_)) {
                    errors.spanned(
                        ty.span(),
                        "foreign methods cannot return references; return an owned value",
                    );
                    continue;
                }
                match result_types(ty) {
                    Some((ok, err)) => {
                        if matches!(ok, Type::Reference(_)) {
                            errors.spanned(
                                ok.span(),
                                "foreign methods cannot return references; return an owned value",
                            );
                            continue;
                        }
                        (Some(ok.clone()), Some(err.clone()))
                    }
                    None => (Some((**ty).clone()), None),
                }
            }
        };

        // The descriptor sees the host-visible signature: a `Result<T, E>`
        // return is described as `T` — the error type exists only on the
        // Rust side (built via `From<ForeignError>`), the host just fails.
        let mut desc_sig = m.sig.clone();
        if err_ty.is_some() {
            let ok_ty = return_ty.clone().expect("Result always has an Ok type");
            desc_sig.output = syn::parse_quote! { -> #ok_ty };
        }
        // Strips parameter attrs even on error paths (on the clone; the
        // re-emitted trait method is stripped below).
        let info = build_fn_info(&mut desc_sig, &fn_args, &m.attrs, &ctx, &mut errors);
        strip_param_script_attrs(&mut m.sig);
        strip_script_attrs(&mut m.attrs);
        let Some(info) = info else { continue };

        if info.receiver != ReceiverShape::Ref {
            errors.spanned(m.sig.span(), "foreign methods must take `&self`");
            continue;
        }
        if info.is_async {
            has_async = true;
        }

        let name = &info.name;
        let arg_exprs: Vec<TokenStream> = m
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
                let ident = &pi.ident;
                Some(quote! { ::haphe::IntoScript::into_script(#ident) })
            })
            .collect();

        let call = if info.is_async {
            quote! { self.0.call_async(#name, &__args).await }
        } else {
            quote! { self.0.call(#name, &__args) }
        };
        let return_ty_tokens = match &return_ty {
            Some(ty) => quote! { #ty },
            None => quote! { () },
        };
        let body = match &err_ty {
            Some(err_ty) => quote_spanned! {err_ty.span()=>
                {
                    let __args = [#(#arg_exprs),*];
                    match #call {
                        ::core::result::Result::Ok(__v) => {
                            match <#return_ty_tokens as ::haphe::FromScript>::from_script(__v) {
                                ::core::result::Result::Ok(__v) => ::core::result::Result::Ok(__v),
                                ::core::result::Result::Err(__e) => ::core::result::Result::Err(
                                    <#err_ty as ::core::convert::From<::haphe::ForeignError>>::from(
                                        ::haphe::ForeignError {
                                            function: #name,
                                            kind: ::haphe::ForeignErrorKind::Convert(__e),
                                        },
                                    ),
                                ),
                            }
                        }
                        ::core::result::Result::Err(__e) => ::core::result::Result::Err(
                            <#err_ty as ::core::convert::From<::haphe::ForeignError>>::from(__e),
                        ),
                    }
                }
            },
            None => quote! {
                {
                    let __args = [#(#arg_exprs),*];
                    match #call {
                        ::core::result::Result::Ok(__v) => {
                            match <#return_ty_tokens as ::haphe::FromScript>::from_script(__v) {
                                ::core::result::Result::Ok(__v) => __v,
                                ::core::result::Result::Err(__e) => ::core::panic!(
                                    "{}",
                                    ::haphe::ForeignError {
                                        function: #name,
                                        kind: ::haphe::ForeignErrorKind::Convert(__e),
                                    },
                                ),
                            }
                        }
                        ::core::result::Result::Err(__e) => ::core::panic!("{}", __e),
                    }
                }
            },
        };

        let sig = &m.sig;
        impl_methods.push(quote! { #sig #body });
        fn_descs.push(info.descriptor);
    }

    if has_async && args.thread_safety.is_none() {
        errors.spanned(
            item.ident.span(),
            "foreign traits with async methods must declare #[script(thread_safety = ...)] explicitly",
        );
    }

    if let Err(err) = errors.finish() {
        let compile_error = err.to_compile_error();
        return quote! { #item #compile_error };
    }

    let vis = &item.vis;
    let trait_ident = &item.ident;
    let trait_name = trait_ident.unraw().to_string();
    let handle_ident = quote::format_ident!("{}Handle", trait_ident.unraw());
    let exposed_name = args
        .rename
        .as_ref()
        .map(|r| r.value())
        .unwrap_or_else(|| trait_name.clone());
    let doc = option_str_tokens(&extract_doc(&item.attrs));
    let handle_doc = format!("Generated foreign-interface handle for [`{trait_name}`].");
    // Async here is deliberately executor- and `Send`-agnostic; the handle is
    // the only implementor the macro contract requires.
    let allow_async = has_async.then(|| quote! { #[allow(async_fn_in_trait)] });

    quote! {
        #allow_async
        #item

        #[doc = #handle_doc]
        #vis struct #handle_ident(::std::boxed::Box<dyn ::haphe::ForeignCaller>);

        #[automatically_derived]
        impl ::haphe::ScriptForeign for #handle_ident {
            const DESCRIPTOR: ::haphe::ForeignInterfaceDescriptor<'static> =
                ::haphe::ForeignInterfaceDescriptor {
                    id: ::haphe::TypeId::new(
                        ::core::concat!(::core::module_path!(), "::", #trait_name),
                    ),
                    name: #exposed_name,
                    doc: #doc,
                    functions: &[#(#fn_descs),*],
                    thread_safety: ::haphe::ThreadSafety::NONE,
                };
        }

        #[automatically_derived]
        impl ::haphe::ForeignHandle for #handle_ident {
            fn from_caller(caller: ::std::boxed::Box<dyn ::haphe::ForeignCaller>) -> Self {
                Self(caller)
            }
        }

        #[automatically_derived]
        impl #trait_ident for #handle_ident {
            #(#impl_methods)*
        }
    }
}
