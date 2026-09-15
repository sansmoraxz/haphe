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
    let mut generic_names: Vec<String> = Vec::new();
    let mut gp_exprs: Vec<TokenStream> = Vec::new();
    for param in &item.generics.params {
        match param {
            syn::GenericParam::Type(tp) => {
                if tp.default.is_some() {
                    errors.spanned(
                        tp.span(),
                        "type parameter defaults are not supported on foreign traits",
                    );
                }
                let name = tp.ident.to_string();
                let bounds: Vec<String> = tp
                    .bounds
                    .iter()
                    .filter_map(|b| match b {
                        syn::TypeParamBound::Trait(t) => {
                            Some(crate::derive::stringify_bound(quote!(#t)))
                        }
                        _ => None,
                    })
                    .collect();
                gp_exprs.push(quote! {
                    ::haphe::GenericParam {
                        name: #name,
                        bounds: &[#(#bounds),*],
                        default: ::core::option::Option::None,
                    }
                });
                generic_names.push(name);
            }
            syn::GenericParam::Lifetime(lt) => {
                errors.spanned(lt.span(), "foreign traits cannot have lifetime parameters");
            }
            syn::GenericParam::Const(cp) => {
                errors.spanned(
                    cp.span(),
                    "foreign traits cannot have const generic parameters",
                );
            }
        }
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
        let mut method_type_params: Vec<syn::Ident> = Vec::new();
        for param in &m.sig.generics.params {
            match param {
                syn::GenericParam::Type(tp) => {
                    if tp.default.is_some() {
                        errors.spanned(
                            tp.span(),
                            "type parameter defaults are not supported on foreign methods",
                        );
                    }
                    method_type_params.push(tp.ident.clone());
                }
                syn::GenericParam::Lifetime(lt) => {
                    errors.spanned(lt.span(), "foreign methods cannot have lifetime parameters");
                }
                // Const generics are rejected by `build_fn_info`.
                syn::GenericParam::Const(_) => {}
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
        // Each method sees the trait's parameters plus its own.
        let mut declared_names = generic_names.clone();
        declared_names.extend(method_type_params.iter().map(|p| p.to_string()));
        let ctx = TyCtx {
            generic_params: &declared_names,
            self_ty: None,
        };
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

        // Dispatch needs to describe and convert method-level type arguments,
        // so the re-emitted trait carries the bounds the handle requires.
        if !method_type_params.is_empty() {
            for param in &mut m.sig.generics.params {
                if let syn::GenericParam::Type(tp) = param {
                    tp.bounds.push(syn::parse_quote!(::haphe::HapheType));
                    tp.bounds.push(syn::parse_quote!(::haphe::FromScript));
                }
            }
            for p in &method_type_params {
                m.sig
                    .generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote! { ::haphe::ScriptValue: ::core::convert::From<#p> });
            }
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

        let type_arg_count = method_type_params.len();
        let type_args_decl = quote! {
            let __type_args: [::haphe::TypeDescriptor<'static>; #type_arg_count] =
                [#( <#method_type_params as ::haphe::HapheType>::DESCRIPTOR ),*];
        };
        let call = if info.is_async {
            quote! { self.0.call_async(#name, &__type_args, &__args).await }
        } else {
            quote! { self.0.call(#name, &__type_args, &__args) }
        };
        let return_ty_tokens = match &return_ty {
            Some(ty) => quote! { #ty },
            None => quote! { () },
        };
        let body = match &err_ty {
            Some(err_ty) => quote_spanned! {err_ty.span()=>
                {
                    #type_args_decl
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
                    #type_args_decl
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

    let has_params = !generic_names.is_empty();
    let generics = &item.generics;
    let (impl_g, ty_g, where_c) = item.generics.split_for_impl();
    let type_params: Vec<_> = item
        .generics
        .type_params()
        .map(|p| p.ident.clone())
        .collect();

    let struct_def = if has_params {
        quote! {
            #[doc = #handle_doc]
            #vis struct #handle_ident #generics (
                ::std::boxed::Box<dyn ::haphe::ForeignCaller>,
                ::core::marker::PhantomData<(#(#type_params,)*)>,
            );
        }
    } else {
        quote! {
            #[doc = #handle_doc]
            #vis struct #handle_ident(::std::boxed::Box<dyn ::haphe::ForeignCaller>);
        }
    };
    let construct = if has_params {
        quote! { Self(caller, ::core::marker::PhantomData) }
    } else {
        quote! { Self(caller) }
    };

    // Dispatch bounds live on the trait impl only, so the erased DESCRIPTOR
    // and `from_caller` stay unconstrained.
    let mut dispatch_generics = item.generics.clone();
    for param in &type_params {
        dispatch_generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote! { ::haphe::ScriptValue: ::core::convert::From<#param> });
        dispatch_generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote! { #param: ::haphe::FromScript });
    }
    let (dis_impl_g, _, dis_where_c) = dispatch_generics.split_for_impl();

    // The erased DESCRIPTOR needs no bounds, but TYPE_ARGS describes the
    // handle's concrete instantiation, so a generic handle's `ScriptForeign`
    // impl requires each parameter to be describable.
    let mut sf_generics = item.generics.clone();
    for param in &type_params {
        sf_generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote! { #param: ::haphe::HapheType });
    }
    let (sf_impl_g, _, sf_where_c) = sf_generics.split_for_impl();
    let type_args = if type_params.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            const TYPE_ARGS: &'static [::haphe::TypeDescriptor<'static>] =
                &[#( <#type_params as ::haphe::HapheType>::DESCRIPTOR ),*];
        }
    };

    quote! {
        #allow_async
        #item

        #struct_def

        #[automatically_derived]
        impl #sf_impl_g ::haphe::ScriptForeign for #handle_ident #ty_g #sf_where_c {
            const DESCRIPTOR: ::haphe::ForeignInterfaceDescriptor<'static> =
                ::haphe::ForeignInterfaceDescriptor {
                    id: ::haphe::TypeId::new(
                        ::core::concat!(::core::module_path!(), "::", #trait_name),
                    ),
                    name: #exposed_name,
                    doc: #doc,
                    generic_params: &[#(#gp_exprs),*],
                    functions: &[#(#fn_descs),*],
                    thread_safety: ::haphe::ThreadSafety::NONE,
                };
            #type_args
        }

        #[automatically_derived]
        impl #impl_g ::haphe::ForeignHandle for #handle_ident #ty_g #where_c {
            fn from_caller(caller: ::std::boxed::Box<dyn ::haphe::ForeignCaller>) -> Self {
                #construct
            }
        }

        #[automatically_derived]
        impl #dis_impl_g #trait_ident #ty_g for #handle_ident #ty_g #dis_where_c {
            #(#impl_methods)*
        }
    }
}
