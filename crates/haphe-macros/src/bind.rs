//! Generates `impl ScriptBind for T` from parsed struct fields and impl
//! block methods. Called from `derive.rs` (fields + metamethods) and
//! `imp.rs` (methods + constructors).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Generics, Ident, Type};

use crate::attrs::TraitDecl;
use crate::fn_desc::ReceiverShape;
use crate::ty_map::substitute_self;

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
#[allow(dead_code)]
pub struct BindMethod {
    pub ident: Ident,
    pub name: String,
    pub receiver: ReceiverShape,
    pub params: Vec<(Ident, Type)>,
    pub has_return: bool,
    pub return_ty: Option<Type>,
}

// ---------------------------------------------------------------------------
// Derive-side codegen (fields + metamethods)
// ---------------------------------------------------------------------------

pub fn hidden_mod_ident(ident: &Ident) -> Ident {
    format_ident!("__haphe_bind_{}", ident.unraw())
}

/// Generates the hidden module with field and metamethod registration,
/// and optionally the full `impl ScriptBind` (when no `#[script(methods)]`).
pub fn gen_derive_bind(
    ident: &Ident,
    self_ty: &Type,
    fields: &[BindField],
    traits: &[TraitDecl],
    has_methods: bool,
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
    let field_regs = gen_field_registrations(self_ty, fields, field_generic_params);
    let meta_regs = gen_metamethod_registrations(self_ty, traits);

    // For bridge types (has_methods), add FromScript + IntoScript bounds on
    // type params so generic fields/methods resolve at monomorphization.
    let mut bind_generics = generics.clone();
    if has_methods {
        for param in &mut bind_generics.params {
            if let syn::GenericParam::Type(tp) = param {
                tp.bounds.push(syn::parse_quote!(::haphe::FromScript));
                tp.bounds.push(syn::parse_quote!(::haphe::IntoScript));
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

    let script_bind = quote! {
        #[automatically_derived]
        impl #bind_impl_g ::haphe::ScriptBind for #self_ty #bind_where_c {
            fn bind<__B: ::haphe::TypeBinder<Self>>(
                __b: &mut __B,
            ) -> ::core::result::Result<(), __B::Error> {
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
pub fn gen_impl_bind_methods(
    ident: &Ident,
    self_ty: &Type,
    methods: &[BindMethod],
    constructors: &[BindMethod],
    generics: &Generics,
) -> TokenStream {
    let mod_ident = hidden_mod_ident(ident);
    let method_regs = methods.iter().map(|m| gen_method_registration(self_ty, m));
    let ctor_regs = constructors
        .iter()
        .map(|c| gen_constructor_registration(self_ty, c));

    // For generic impls, add FromScript + IntoScript bounds on type params.
    let mut bind_generics = generics.clone();
    for param in &mut bind_generics.params {
        if let syn::GenericParam::Type(tp) = param {
            tp.bounds.push(syn::parse_quote!(::haphe::FromScript));
            tp.bounds.push(syn::parse_quote!(::haphe::IntoScript));
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
                #(#method_regs)*
                ::core::result::Result::Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Field registration
// ---------------------------------------------------------------------------

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

/// Checks (syntactically) whether a type is a primitive that implements
/// `IntoScript + FromScript`. Covers the blanket impls in the bridge module.
fn is_bridge_primitive(ty: &Type) -> bool {
    let Type::Path(p) = ty else { return false };
    if p.qself.is_some() {
        return false;
    }
    let Some(ident) = p.path.get_ident() else {
        return false;
    };
    matches!(
        ident.to_string().as_str(),
        "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
            | "char"
            | "String"
    )
}

fn is_generic_type_param(ty: &Type, params: &[String]) -> bool {
    if let Type::Path(p) = ty
        && p.qself.is_none()
        && let Some(ident) = p.path.get_ident()
    {
        params.iter().any(|g| ident == g)
    } else {
        false
    }
}

fn gen_field_registrations(
    self_ty: &Type,
    fields: &[BindField],
    generic_params: &[String],
) -> TokenStream {
    let regs = fields
        .iter()
        .filter(|f| is_bridge_primitive(&f.ty) || is_generic_type_param(&f.ty, generic_params))
        .map(|f| {
            let ident = &f.ident;
            let name = &f.name;
            let ty = &f.ty;

            let setter = if f.readonly {
                quote! { ::core::option::Option::None }
            } else {
                quote! {
                    ::core::option::Option::Some(
                        (|__t: &mut #self_ty, __v: #ty| __t.#ident = __v) as fn(&mut #self_ty, #ty)
                    )
                }
            };

            quote! {
                __b.field::<#ty>(
                    #name,
                    (|__t: &#self_ty| __t.#ident.clone()) as fn(&#self_ty) -> #ty,
                    #setter,
                )?;
            }
        });

    quote! { #(#regs)* }
}

// ---------------------------------------------------------------------------
// Metamethod registration
// ---------------------------------------------------------------------------

fn gen_metamethod_registrations(self_ty: &Type, traits: &[TraitDecl]) -> TokenStream {
    let mut tokens = TokenStream::new();
    let has_display = traits.iter().any(|t| t.name == "Display");

    let ctx = crate::ty_map::TyCtx {
        generic_params: &[],
        self_ty: Some(self_ty),
    };

    for decl in traits {
        let name = decl.name.to_string();
        match name.as_str() {
            "Display" => {
                tokens.extend(quote! {
                    __b.meta_tostring(
                        (|__t: &#self_ty| ::std::format!("{__t}")) as fn(&#self_ty) -> ::std::string::String,
                    )?;
                });
            }
            "Debug" if !has_display => {
                tokens.extend(quote! {
                    __b.meta_tostring(
                        (|__t: &#self_ty| ::std::format!("{__t:?}")) as fn(&#self_ty) -> ::std::string::String,
                    )?;
                });
            }
            "PartialEq" => {
                tokens.extend(quote! {
                    __b.meta_eq(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a == __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                });
            }
            "PartialOrd" => {
                tokens.extend(quote! {
                    __b.meta_lt(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a < __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                    __b.meta_le(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a <= __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                });
            }
            "Neg" => {
                tokens.extend(quote! {
                    __b.meta_unm(
                        (|__t: &#self_ty| -__t.clone()) as fn(&#self_ty) -> #self_ty,
                    )?;
                });
            }
            "Add" | "Sub" | "Mul" | "Div" | "Rem" => {
                let rhs_ty = decl
                    .args
                    .iter()
                    .find(|(n, _)| n == "rhs")
                    .map(|(_, ty)| substitute_self(ty, &ctx))
                    .unwrap_or_else(|| self_ty.clone());

                let op_name_str = name.to_lowercase();
                let op_method = Ident::new(&op_name_str, decl.name.span());
                let op_trait = &decl.name;
                let rhs_is_self = quote!(#rhs_ty).to_string() == quote!(#self_ty).to_string();

                if rhs_is_self {
                    tokens.extend(quote! {
                        __b.meta_arith_self(
                            #op_name_str,
                            |__a: #self_ty, __b_val: #self_ty| -> #self_ty {
                                <#self_ty as ::core::ops::#op_trait>::#op_method(__a, __b_val)
                            },
                        )?;
                    });
                } else {
                    tokens.extend(quote! {
                        __b.meta_arith_scalar(
                            #op_name_str,
                            &<#rhs_ty as ::haphe::HapheType>::DESCRIPTOR,
                            |__self: #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<#self_ty, ::haphe::ScriptConvertError> {
                                let __rhs = <#rhs_ty as ::haphe::FromScript>::from_script(
                                    __args.first().cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                                ::core::result::Result::Ok(
                                    <#self_ty as ::core::ops::#op_trait<#rhs_ty>>::#op_method(__self, __rhs)
                                )
                            },
                        )?;
                    });
                }
            }
            _ => {}
        }
    }

    tokens
}

// ---------------------------------------------------------------------------
// Method registration
// ---------------------------------------------------------------------------

/// Generates `FromScript` extraction code for method params.
fn gen_param_extractions(params: &[(Ident, Type)]) -> Vec<TokenStream> {
    params
        .iter()
        .enumerate()
        .map(|(i, (name, ty))| {
            let inner_ty = strip_ref(ty);
            quote! {
                let #name = <#inner_ty as ::haphe::FromScript>::from_script(
                    __args.get(#i).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                )?;
            }
        })
        .collect()
}

fn gen_call_args(params: &[(Ident, Type)]) -> Vec<TokenStream> {
    params
        .iter()
        .map(|(n, t)| {
            if is_reference(t) {
                quote! { &#n }
            } else {
                quote! { #n }
            }
        })
        .collect()
}

fn gen_method_registration(self_ty: &Type, method: &BindMethod) -> TokenStream {
    let name = &method.name;
    let ident = &method.ident;

    let extractions = gen_param_extractions(&method.params);
    let call_args = gen_call_args(&method.params);

    let result_expr = if method.has_return {
        quote! { ::haphe::IntoScript::into_script(__t.#ident(#(#call_args),*)) }
    } else {
        quote! { { __t.#ident(#(#call_args),*); ::haphe::ScriptValue::Unit } }
    };

    match method.receiver {
        ReceiverShape::Ref => {
            quote! {
                __b.method_ref(
                    #name,
                    |__t: &#self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
        ReceiverShape::RefMut => {
            quote! {
                __b.method_mut(
                    #name,
                    |__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
        ReceiverShape::Owned => {
            let result_expr = if method.has_return {
                quote! { ::haphe::IntoScript::into_script(__t.#ident(#(#call_args),*)) }
            } else {
                quote! { { __t.#ident(#(#call_args),*); ::haphe::ScriptValue::Unit } }
            };
            quote! {
                __b.method_owned(
                    #name,
                    |__t: #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
        ReceiverShape::None => {
            quote! {
                __b.method_ref(
                    #name,
                    |_: &#self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(::haphe::IntoScript::into_script(<#self_ty>::#ident(#(#call_args),*)))
                    },
                )?;
            }
        }
    }
}

fn gen_constructor_registration(self_ty: &Type, ctor: &BindMethod) -> TokenStream {
    let name = &ctor.name;
    let ident = &ctor.ident;

    let extractions = gen_param_extractions(&ctor.params);
    let call_args = gen_call_args(&ctor.params);

    quote! {
        __b.constructor(
            #name,
            |__args: &[::haphe::ScriptValue]| -> ::core::result::Result<#self_ty, ::haphe::ScriptConvertError> {
                #(#extractions)*
                ::core::result::Result::Ok(<#self_ty>::#ident(#(#call_args),*))
            },
        )?;
    }
}

fn strip_ref(ty: &Type) -> Type {
    match ty {
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
    }
}

fn is_reference(ty: &Type) -> bool {
    matches!(ty, Type::Reference(_))
}
