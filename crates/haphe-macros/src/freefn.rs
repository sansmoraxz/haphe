//! Expansion of `#[script]` on free functions: emits the function unchanged
//! plus a hidden type sharing its name that carries the descriptor via the
//! `ScriptFunction` trait. Because the type shares the function's name, it
//! travels with `use` imports and re-exports, so `registry!` can resolve the
//! descriptor through any path that names the function.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemFn;
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

    let ctx = TyCtx::default();
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
    quote! {
        #item

        // A type sharing the function's name (types and values live in
        // different namespaces) so the descriptor is reachable wherever the
        // function is.
        #(#cfgs)*
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #ident {}

        #(#cfgs)*
        #[automatically_derived]
        impl ::haphe::ScriptFunction for #ident {
            const DESCRIPTOR: ::haphe::FunctionDescriptor<'static> = #descriptor;
        }
    }
}
