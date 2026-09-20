//! Generates `impl ScriptBind for T` from parsed struct fields and impl
//! block methods. Called from `derive.rs` (fields + metamethods) and
//! `imp.rs` (methods + constructors).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Generics, Ident, Type};

use crate::attrs::TraitDecl;
use crate::fn_desc::ReceiverShape;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A struct field to register via the bridge.
pub struct BindField {
    pub ident: Ident,
    pub name: String,
    pub ty: Type,
    pub readonly: bool,
}

/// A method or constructor to register via the bridge.
pub struct BindMethod {
    pub ident: Ident,
    pub name: String,
    pub receiver: ReceiverShape,
    pub params: Vec<(Ident, Type)>,
    pub has_return: bool,
    pub return_ty: Option<Type>,
    /// The declared return was `Result<T, E>`: `return_ty` holds `T` and the
    /// wrapper maps `Err(e)` into [`ScriptCallError::Callee`] (rendered via
    /// `Display`), tagged with `error_kind`.
    pub fallible: bool,
    /// The `error_kind = "..."` hint, carried into `Callee` errors.
    pub error_kind: Option<String>,
    pub is_async: bool,
}

/// A generic method: one monomorphized wrapper per declared instantiation.
/// `dyn`-dispatched ones register through the descriptor-carrying
/// `method_dyn*` channels; statically dispatched ones through
/// `method_generic*`, keyed on `(name, type_args)`.
pub struct GenericBindMethod {
    pub method: BindMethod,
    /// The impl's (self type's) generic type parameters — empty on a
    /// non-generic self type. `dyn` registrations carry their identity as
    /// [`SelfInstantiation`](https://docs.rs/haphe) so the resolver can
    /// substitute them alongside the method's own.
    pub self_params: Vec<Ident>,
    /// The function's own generic type parameters, in declaration order.
    pub type_params: Vec<Ident>,
    /// Declared instantiations: concrete type arguments per entry.
    pub instantiations: Vec<(Vec<Type>, proc_macro2::Span)>,
    pub is_async: bool,
    pub dyn_dispatch: bool,
    /// The method's `FunctionDescriptor { ... }` literal.
    pub descriptor: TokenStream,
}

mod fields;
pub(crate) mod gates;
mod metamethods;
mod methods;

pub(crate) use fields::*;
pub(crate) use gates::*;
pub(crate) use metamethods::*;
pub(crate) use methods::*;

// ---------------------------------------------------------------------------
// Derive-side codegen (fields + metamethods)
// ---------------------------------------------------------------------------

pub fn hidden_mod_ident(ident: &Ident) -> Ident {
    format_ident!("__haphe_bind_{}", ident.unraw())
}

/// Generates the hidden module with field and metamethod registration,
/// and optionally the full `impl ScriptBind` (when no `#[script(methods)]`).
#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
pub fn gen_derive_bind(
    ident: &Ident,
    self_ty: &Type,
    fields: &[BindField],
    traits: &[TraitDecl],
    has_methods: bool,
    async_probe_in_bind: bool,
    generics: &Generics,
) -> TokenStream {
    let mod_ident = hidden_mod_ident(ident);
    let generic_param_names: Vec<String> = generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(tp.ident.to_string()),
            _ => None,
        })
        .collect();
    // Only include generic-param fields when the type has bridge methods —
    // descriptor-only types don't need FromScript/IntoScript bounds.
    let field_generic_params = if has_methods {
        &generic_param_names[..]
    } else {
        &[]
    };
    // Any generic parameter (type or lifetime) rules out dispatch blocks,
    // whose local trait impls cannot reference outer parameters.
    let self_is_generic = !generics.params.is_empty();
    let field_regs =
        gen_field_registrations(self_ty, fields, field_generic_params, self_is_generic);
    let meta_regs = gen_metamethod_registrations(self_ty, traits);

    // For bridge types (has_methods), add FromScript + IntoScript bounds on
    // type params so generic fields/methods resolve at monomorphization.
    let mut bind_generics = generics.clone();
    if has_methods {
        for param in &mut bind_generics.params {
            if let syn::GenericParam::Type(tp) = param {
                tp.bounds.push(syn::parse_quote!(::haphe::FromScript));
                tp.bounds.push(syn::parse_quote!(::haphe::IntoScript));
                tp.bounds.push(syn::parse_quote!(::haphe::HapheType));
            }
        }
    }
    let bind_type_params: Vec<_> = bind_generics.params.iter().collect();
    let (bind_impl_g, _, bind_where_c) = bind_generics.split_for_impl();

    let type_params: Vec<_> = generics.params.iter().collect();
    let (_, _, where_c) = generics.split_for_impl();

    let methods_call = if has_methods {
        quote! { <#self_ty as #mod_ident::__BindMethods>::__bind_methods(__b)?; }
    } else {
        TokenStream::new()
    };

    let methods_trait = if has_methods {
        quote! {
            #[doc(hidden)]
            pub trait __BindMethods: ::core::marker::Sized {
                fn __bind_methods<__B: ::haphe::TypeBinder<Self>>(
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error>;
            }
        }
    } else {
        TokenStream::new()
    };

    let fields_fn_ident = format_ident!("__haphe_fields_{}", ident.unraw());
    let metamethods_fn_ident = format_ident!("__haphe_meta_{}", ident.unraw());

    // Functions live at the struct's scope (not inside a module) so they
    // can resolve `#self_ty` even when the struct is defined inside a
    // function body (e.g. doctests wrapped in `fn main`).
    let field_fn = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #fields_fn_ident<#(#bind_type_params,)* __B: ::haphe::TypeBinder<#self_ty>>(
            __b: &mut __B,
        ) -> ::core::result::Result<(), __B::Error> #bind_where_c {
            #field_regs
            ::core::result::Result::Ok(())
        }
    };

    let meta_fn = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #metamethods_fn_ident<#(#type_params,)* __B: ::haphe::TypeBinder<#self_ty>>(
            __b: &mut __B,
        ) -> ::core::result::Result<(), __B::Error> #where_c {
            #meta_regs
            ::core::result::Result::Ok(())
        }
    };

    // Only the __BindMethods trait stays in the module — it uses `Self`,
    // not the concrete struct name, so `use super::*` isn't needed.
    let hidden_mod = if has_methods {
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            mod #mod_ident {
                #methods_trait
            }
        }
    } else {
        TokenStream::new()
    };

    // A generic type without a declared thread_safety checks the async rule
    // here: `bind` monomorphizes exactly when the type is exposed (a
    // top-level `const _` cannot name the type's parameters).
    let async_probe = if async_probe_in_bind {
        quote! {
            const {
                ::core::assert!(
                    !<Self as ::haphe::ScriptImpl>::HAS_ASYNC,
                    "types with async methods must declare #[script(thread_safety = ...)] explicitly"
                )
            };
        }
    } else {
        TokenStream::new()
    };
    let script_bind = quote! {
        #[automatically_derived]
        impl #bind_impl_g ::haphe::ScriptBind for #self_ty #bind_where_c {
            fn bind<__B: ::haphe::TypeBinder<Self>>(
                __b: &mut __B,
            ) -> ::core::result::Result<(), __B::Error> {
                #async_probe
                #fields_fn_ident(__b)?;
                #metamethods_fn_ident(__b)?;
                #methods_call
                ::core::result::Result::Ok(())
            }
        }
    };
    quote! { #field_fn #meta_fn #hidden_mod #script_bind }
}

// ---------------------------------------------------------------------------
// Impl-side codegen (methods + constructors + full ScriptBind)
// ---------------------------------------------------------------------------

/// Generates the `__BindMethods` trait impl for the type.
/// Called from `#[script] impl` — the derive emitted the trait definition
/// and the `impl ScriptBind` that calls it.
/// Everything `gen_impl_bind_methods` renders into `__bind_methods`.
pub struct BindImplInput<'a> {
    pub ident: &'a Ident,
    pub self_ty: &'a Type,
    pub methods: &'a [BindMethod],
    pub async_methods: &'a [BindMethod],
    pub dispatch_methods: &'a [BindMethod],
    pub generic_methods: &'a [GenericBindMethod],
    pub constructors: &'a [BindMethod],
    pub async_constructors: &'a [BindMethod],
    pub property_regs: &'a [TokenStream],
    pub generics: &'a Generics,
}

pub fn gen_impl_bind_methods(input: &BindImplInput<'_>) -> TokenStream {
    let BindImplInput {
        ident,
        self_ty,
        methods,
        async_methods,
        dispatch_methods,
        generic_methods,
        constructors,
        async_constructors,
        property_regs,
        generics,
    } = input;
    let mod_ident = hidden_mod_ident(ident);
    let method_regs = methods.iter().map(|m| gen_method_registration(self_ty, m));
    let generic_regs = generic_methods
        .iter()
        .map(|m| gen_generic_method_registration(self_ty, m));
    let async_ctor_regs = async_constructors
        .iter()
        .map(|c| gen_async_constructor_registration(self_ty, c));
    let async_regs = async_methods
        .iter()
        .map(|m| gen_async_method_registration(self_ty, m));
    let dispatch_regs = dispatch_methods
        .iter()
        .map(|m| gen_dispatched_method_registration(self_ty, m));
    let ctor_regs = constructors
        .iter()
        .map(|c| gen_constructor_registration(self_ty, c));

    // For generic impls, add FromScript + IntoScript + HapheType bounds on
    // type params (the last so dyn methods can materialize the self type's
    // instantiation descriptors; every bridgeable type is describable).
    let mut bind_generics = (*generics).clone();
    for param in &mut bind_generics.params {
        if let syn::GenericParam::Type(tp) = param {
            tp.bounds.push(syn::parse_quote!(::haphe::FromScript));
            tp.bounds.push(syn::parse_quote!(::haphe::IntoScript));
            tp.bounds.push(syn::parse_quote!(::haphe::HapheType));
        }
    }
    let (impl_g, _ty_g, where_c) = bind_generics.split_for_impl();

    quote! {
        #[automatically_derived]
        impl #impl_g #mod_ident::__BindMethods for #self_ty #where_c {
            fn __bind_methods<__B: ::haphe::TypeBinder<Self>>(
                __b: &mut __B,
            ) -> ::core::result::Result<(), __B::Error> {
                #(#ctor_regs)*
                #(#async_ctor_regs)*
                #(#property_regs)*
                #(#method_regs)*
                #(#async_regs)*
                #(#dispatch_regs)*
                #(#generic_regs)*
                ::core::result::Result::Ok(())
            }
        }
    }
}
