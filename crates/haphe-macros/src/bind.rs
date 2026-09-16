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
    async_methods: &[BindMethod],
    dispatch_methods: &[BindMethod],
    constructors: &[BindMethod],
    generics: &Generics,
) -> TokenStream {
    let mod_ident = hidden_mod_ident(ident);
    let method_regs = methods.iter().map(|m| gen_method_registration(self_ty, m));
    let async_regs = async_methods
        .iter()
        .map(|m| gen_async_method_registration(self_ty, m));
    let dispatch_regs = dispatch_methods
        .iter()
        .map(|m| gen_dispatched_method_registration(self_ty, m));
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
                #(#async_regs)*
                #(#dispatch_regs)*
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

/// Owned, bare single-ident path type that isn't a known primitive: its
/// bridgeability can't be judged syntactically (it may be a transparent
/// primitive newtype), so registration goes through compile-time trait
/// dispatch instead.
fn needs_bridge_dispatch(ty: &Type, generic_params: &[String]) -> bool {
    if is_bridge_primitive(ty) || is_generic_type_param(ty, generic_params) {
        return false;
    }
    // Fields must be owned; references never dispatch.
    !is_reference(ty) && is_dispatchable_path(ty)
}

/// An owned bare single-ident path type (or one behind a single reference)
/// whose bridgeability is decided by trait presence rather than the
/// syntactic whitelist.
pub fn is_dispatchable_path(ty: &Type) -> bool {
    let stripped = strip_ref(ty);
    let Type::Path(p) = &stripped else {
        return false;
    };
    p.qself.is_none() && p.path.get_ident().is_some()
}

/// A field registration that compiles to a real registration when the field
/// type implements the bridge traits (e.g. a transparent primitive newtype)
/// and to a no-op otherwise — decided at compile time via autoref
/// specialization, never at runtime.
fn gen_dispatched_field_registration(self_ty: &Type, f: &BindField) -> TokenStream {
    let ident = &f.ident;
    let name = &f.name;
    let ty = &f.ty;
    let (set_decl, set_impl, setter) = if f.readonly {
        (
            TokenStream::new(),
            TokenStream::new(),
            quote! { ::core::option::Option::None },
        )
    } else {
        (
            quote! { fn __set(&mut self, __v: Self::__V); },
            quote! { fn __set(&mut self, __v: #ty) { self.#ident = __v; } },
            quote! {
                ::core::option::Option::Some(
                    (|__t: &mut __T, __v: __T::__V| __T::__set(__t, __v))
                        as fn(&mut __T, __T::__V)
                )
            },
        )
    };
    quote! {
        {
            #[allow(non_camel_case_types)]
            trait __FieldAccess {
                type __V;
                fn __get(&self) -> &Self::__V;
                #set_decl
            }
            impl __FieldAccess for #self_ty {
                type __V = #ty;
                fn __get(&self) -> &#ty {
                    &self.#ident
                }
                #set_impl
            }
            #[allow(non_camel_case_types)]
            trait __Go<__T> {
                fn __haphe_bind<__B: ::haphe::TypeBinder<__T>>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error>;
            }
            impl<__T> __Go<__T> for &::haphe::BridgeProbe<__T>
            where
                __T: __FieldAccess,
                __T::__V: ::haphe::IntoScript
                    + ::haphe::FromScript
                    + ::core::clone::Clone
                    + 'static,
            {
                fn __haphe_bind<__B: ::haphe::TypeBinder<__T>>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error> {
                    __b.field::<__T::__V>(
                        #name,
                        (|__t: &__T| ::core::clone::Clone::clone(__T::__get(__t)))
                            as fn(&__T) -> __T::__V,
                        #setter,
                    )
                }
            }
            #[allow(unused_imports)]
            use ::haphe::SkipBind as _;
            (&&::haphe::BridgeProbe::<#self_ty>(::core::marker::PhantomData))
                .__haphe_bind(__b)?;
        }
    }
}

fn gen_field_registrations(
    self_ty: &Type,
    fields: &[BindField],
    generic_params: &[String],
    self_is_generic: bool,
) -> TokenStream {
    // Dispatch blocks define local trait impls on the self type, which
    // cannot reference outer generic parameters — generic types keep the
    // syntactic fast path only.
    let dispatched: Vec<TokenStream> = if self_is_generic {
        Vec::new()
    } else {
        fields
            .iter()
            .filter(|f| needs_bridge_dispatch(&f.ty, generic_params))
            .map(|f| gen_dispatched_field_registration(self_ty, f))
            .collect()
    };
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

    quote! { #(#regs)* #(#dispatched)* }
}

// ---------------------------------------------------------------------------
// Metamethod registration
// ---------------------------------------------------------------------------

fn gen_metamethod_registrations(self_ty: &Type, traits: &[TraitDecl]) -> TokenStream {
    let mut tokens = TokenStream::new();
    let has_display = traits.iter().any(|t| t.name == "Display");
    // `Eq` implies `PartialEq` and `Ord` implies `PartialOrd`: either marker
    // yields the comparison metamethods, deduplicated when both are declared.
    let has_partial_eq = traits.iter().any(|t| t.name == "PartialEq");
    let has_partial_ord = traits.iter().any(|t| t.name == "PartialOrd");
    let has_into_iterator = traits.iter().any(|t| t.name == "IntoIterator");

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
            "ToString" => {
                tokens.extend(quote! {
                    __b.meta_concat(
                        (|__t: &#self_ty| ::std::string::ToString::to_string(__t)) as fn(&#self_ty) -> ::std::string::String,
                    )?;
                });
            }
            "Debug" => {
                if !has_display {
                    tokens.extend(quote! {
                        __b.meta_tostring(
                            (|__t: &#self_ty| ::std::format!("{__t:?}")) as fn(&#self_ty) -> ::std::string::String,
                        )?;
                    });
                }
                tokens.extend(quote! {
                    __b.meta_debug(
                        (|__t: &#self_ty| ::std::format!("{__t:?}")) as fn(&#self_ty) -> ::std::string::String,
                    )?;
                });
            }
            "Hash" => {
                tokens.extend(quote! {
                    __b.meta_hash(
                        (|__t: &#self_ty| {
                            use ::std::hash::{Hash, Hasher};
                            let mut __h = ::std::hash::DefaultHasher::new();
                            Hash::hash(__t, &mut __h);
                            __h.finish()
                        }) as fn(&#self_ty) -> u64,
                    )?;
                });
            }
            // `Default` projects as a nullary constructor named `default`
            // through the ordinary constructor channel — reachable from
            // every backend (user ruling: full runtime interusability).
            "Default" => {
                tokens.extend(quote! {
                    __b.constructor(
                        "default",
                        (|_: &[::haphe::ScriptValue]| ::core::result::Result::Ok(
                            <#self_ty as ::core::default::Default>::default()
                        )) as fn(&[::haphe::ScriptValue]) -> ::core::result::Result<#self_ty, ::haphe::ScriptConvertError>,
                    )?;
                });
            }
            "PartialEq" | "Eq" => {
                if name == "Eq" && has_partial_eq {
                    continue;
                }
                tokens.extend(quote! {
                    __b.meta_eq(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a == __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                });
            }
            "PartialOrd" | "Ord" => {
                if name == "Ord" && has_partial_ord {
                    continue;
                }
                tokens.extend(quote! {
                    __b.meta_lt(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a < __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                    __b.meta_le(
                        (|__a: &#self_ty, __b_val: &#self_ty| __a <= __b_val) as fn(&#self_ty, &#self_ty) -> bool,
                    )?;
                });
            }
            "Neg" | "Not" => {
                let variant = &decl.name;
                let path = quote! { ::haphe::ops::#variant };
                let method = Ident::new(&name.to_lowercase(), decl.name.span());
                let register = Ident::new(
                    if name == "Neg" {
                        "meta_unm"
                    } else {
                        "meta_bnot"
                    },
                    decl.name.span(),
                );
                tokens.extend(quote! {
                    __b.#register(
                        (|__t: &#self_ty| <#self_ty as #path>::#method(__t.clone()))
                            as fn(&#self_ty) -> #self_ty,
                    )?;
                });
            }
            "Add" | "Sub" | "Mul" | "Div" | "Rem" | "IDiv" | "Mod" | "BitAnd" | "BitOr"
            | "BitXor" | "Shl" | "Shr" | "Pow" => {
                let rhs_ty = decl
                    .args
                    .iter()
                    .find(|(n, _)| n == "rhs")
                    .map(|(_, ty)| substitute_self(ty, &ctx))
                    .unwrap_or_else(|| self_ty.clone());

                let op_name_str = name.to_lowercase();
                // `mod` is a Rust keyword; the trait's method is `modulo`.
                let method_str = if name == "Mod" {
                    "modulo"
                } else {
                    &op_name_str
                };
                let op_method = Ident::new(method_str, decl.name.span());
                let op_trait = &decl.name;
                let trait_path = quote! { ::haphe::ops::#op_trait };
                let rhs_is_self = quote!(#rhs_ty).to_string() == quote!(#self_ty).to_string();

                if rhs_is_self {
                    tokens.extend(quote! {
                        __b.meta_arith_self(
                            #op_name_str,
                            |__a: #self_ty, __b_val: #self_ty| -> #self_ty {
                                <#self_ty as #trait_path>::#op_method(__a, __b_val)
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
                                    <#self_ty as #trait_path<#rhs_ty>>::#op_method(__self, __rhs)
                                )
                            },
                        )?;
                    });
                }
            }
            // `Iterator` bridges through the blanket `IntoIterator` impl
            // (`t.into_iter()` is `t` itself) — the consuming registration
            // owns the value, so advancing needs no shared access. Deduped
            // when both are declared.
            "IntoIterator" | "Iterator" => {
                if name == "Iterator" && has_into_iterator {
                    continue;
                }
                tokens.extend(quote! {
                    __b.meta_iter(
                        (|__t: #self_ty| ::haphe::ScriptIter::new(
                            ::core::iter::IntoIterator::into_iter(__t)
                                .map(|__item| ::haphe::ScriptValue::from(__item))
                        )) as fn(#self_ty) -> ::haphe::ScriptIter,
                    )?;
                    __b.meta_len(
                        (|__t: #self_ty| ::core::iter::Iterator::size_hint(
                            &::core::iter::IntoIterator::into_iter(__t)
                        ).0) as fn(#self_ty) -> usize,
                    )?;
                });
            }
            "Call" => {
                let Some(args_ty) = decl
                    .args
                    .iter()
                    .find(|(n, _)| n == "args")
                    .map(|(_, t)| substitute_self(t, &ctx))
                else {
                    continue; // missing `args` already errored in the descriptor pass
                };
                let Type::Tuple(args_tuple) = &args_ty else {
                    continue; // non-tuple `args` already errored
                };
                let elem_tys: Vec<&Type> = args_tuple.elems.iter().collect();
                let vars: Vec<Ident> = (0..elem_tys.len())
                    .map(|i| format_ident!("__a{i}"))
                    .collect();
                let idx: Vec<usize> = (0..elem_tys.len()).collect();
                tokens.extend(quote! {
                    __b.meta_call(
                        (|__t: &#self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                            #(
                                let #vars = <#elem_tys as ::haphe::FromScript>::from_script(
                                    __args.get(#idx).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                            )*
                            ::core::result::Result::Ok(::haphe::IntoScript::into_script(
                                <#self_ty as ::haphe::ops::Call<#args_ty>>::call(
                                    __t,
                                    (#(#vars,)*),
                                )
                            ))
                        }) as fn(&#self_ty, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError>,
                    )?;
                });
            }
            "AsyncCall" => {
                let Some(args_ty) = decl
                    .args
                    .iter()
                    .find(|(n, _)| n == "args")
                    .map(|(_, t)| substitute_self(t, &ctx))
                else {
                    continue; // missing `args` already errored in the descriptor pass
                };
                let Type::Tuple(args_tuple) = &args_ty else {
                    continue; // non-tuple `args` already errored
                };
                let elem_tys: Vec<&Type> = args_tuple.elems.iter().collect();
                let vars: Vec<Ident> = (0..elem_tys.len())
                    .map(|i| format_ident!("__a{i}"))
                    .collect();
                let idx: Vec<usize> = (0..elem_tys.len()).collect();
                tokens.extend(quote! {
                    __b.meta_call_async(
                        (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                            ::std::boxed::Box::pin(async move {
                                #(
                                    let #vars = <#elem_tys as ::haphe::FromScript>::from_script(
                                        __args.get(#idx).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                    )?;
                                )*
                                ::core::result::Result::Ok(::haphe::IntoScript::into_script(
                                    <#self_ty as ::haphe::ops::AsyncCall<#args_ty>>::call_async(
                                        &__recv,
                                        (#(#vars,)*),
                                    )
                                    .await
                                ))
                            })
                        }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                    )?;
                });
            }
            "Index" | "IndexMut" => {
                let arg = |key: &str| {
                    decl.args
                        .iter()
                        .find(|(n, _)| n == key)
                        .map(|(_, ty)| substitute_self(ty, &ctx))
                };
                // Missing args already errored in the descriptor pass.
                let (Some(idx_ty), Some(out_ty)) = (arg("index"), arg("output")) else {
                    continue;
                };
                let variant = &decl.name;
                let trait_path = quote! { ::haphe::ops::#variant };
                if name == "Index" {
                    tokens.extend(quote! {
                        __b.meta_index(
                            (|__t: &#self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                                let __idx = <#idx_ty as ::haphe::FromScript>::from_script(
                                    __args.first().cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                                ::core::result::Result::Ok(::haphe::IntoScript::into_script(
                                    ::core::clone::Clone::clone(
                                        <#self_ty as #trait_path<#idx_ty>>::index(__t, __idx)
                                    )
                                ))
                            }) as fn(&#self_ty, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError>,
                        )?;
                    });
                } else {
                    tokens.extend(quote! {
                        __b.meta_newindex(
                            (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<(), ::haphe::ScriptConvertError> {
                                let __idx = <#idx_ty as ::haphe::FromScript>::from_script(
                                    __args.first().cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                                let __val = <#out_ty as ::haphe::FromScript>::from_script(
                                    __args.get(1).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                                *<#self_ty as #trait_path<#idx_ty>>::index_mut(__t, __idx) = __val;
                                ::core::result::Result::Ok(())
                            }) as fn(&mut #self_ty, &[::haphe::ScriptValue]) -> ::core::result::Result<(), ::haphe::ScriptConvertError>,
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

/// A method registration that compiles to a real registration when every
/// stripped param type implements `FromScript` and the return type
/// implements `IntoScript` (e.g. transparent primitive newtypes), and to a
/// no-op otherwise — compile-time autoref-specialization dispatch, mirroring
/// [`gen_dispatched_field_registration`]. Non-generic self types only.
pub fn gen_dispatched_method_registration(self_ty: &Type, method: &BindMethod) -> TokenStream {
    let name = &method.name;
    let ident = &method.ident;

    let stripped: Vec<Type> = method.params.iter().map(|(_, t)| strip_ref(t)).collect();
    let p_assoc: Vec<Ident> = (0..stripped.len())
        .map(|i| format_ident!("__P{i}"))
        .collect();
    let p_vars: Vec<Ident> = (0..stripped.len())
        .map(|i| format_ident!("__p{i}"))
        .collect();
    let call_args: Vec<TokenStream> = method
        .params
        .iter()
        .zip(&p_vars)
        .map(|((_, t), v)| {
            if is_reference(t) {
                quote! { &#v }
            } else {
                quote! { #v }
            }
        })
        .collect();
    let ret_ty: TokenStream = match (&method.return_ty, method.has_return) {
        (Some(t), true) => quote! { #t },
        _ => quote! { () },
    };
    let idx: Vec<usize> = (0..stripped.len()).collect();

    struct DispatchShape {
        recv_decl: TokenStream,
        recv_call: TokenStream,
        wrapper_sig: TokenStream,
        wrapper_prep: TokenStream,
        cast_ty: TokenStream,
        extra_bound: TokenStream,
        register: Ident,
    }
    let shape = match method.receiver {
        ReceiverShape::RefMut => DispatchShape {
            recv_decl: quote! { __t: &mut Self },
            recv_call: quote! { __t.#ident(#(#call_args),*) },
            wrapper_sig: quote! { __t: &mut __T },
            wrapper_prep: TokenStream::new(),
            cast_ty: quote! { fn(&mut __T, &[::haphe::ScriptValue]) },
            extra_bound: TokenStream::new(),
            register: format_ident!("method_mut"),
        },
        ReceiverShape::Owned => DispatchShape {
            recv_decl: quote! { __t: Self },
            recv_call: quote! { __t.#ident(#(#call_args),*) },
            wrapper_sig: quote! { __recv: ::haphe::ScriptCow<'_, __T> },
            wrapper_prep: quote! { let __t: __T = __recv.into_owned(); },
            cast_ty: quote! { for<'a> fn(::haphe::ScriptCow<'a, __T>, &[::haphe::ScriptValue]) },
            extra_bound: quote! { __T: ::core::clone::Clone, },
            register: format_ident!("method"),
        },
        // `None` mirrors gen_method_registration's associated-fn handling.
        ReceiverShape::Ref | ReceiverShape::None => DispatchShape {
            recv_decl: quote! { __t: &Self },
            recv_call: quote! { __t.#ident(#(#call_args),*) },
            wrapper_sig: quote! { __recv: ::haphe::ScriptCow<'_, __T> },
            wrapper_prep: quote! { let __t: &__T = &__recv; },
            cast_ty: quote! { for<'a> fn(::haphe::ScriptCow<'a, __T>, &[::haphe::ScriptValue]) },
            extra_bound: TokenStream::new(),
            register: format_ident!("method"),
        },
    };
    let DispatchShape {
        recv_decl,
        recv_call,
        wrapper_sig,
        wrapper_prep,
        cast_ty,
        extra_bound,
        register,
    } = shape;
    let recv_decl2 = recv_decl.clone();

    quote! {
        {
            #[allow(non_camel_case_types)]
            trait __Call {
                #( type #p_assoc; )*
                type __R;
                fn __invoke(#recv_decl, #( #p_vars: Self::#p_assoc ),*) -> Self::__R;
            }
            impl __Call for #self_ty {
                #( type #p_assoc = #stripped; )*
                type __R = #ret_ty;
                #[allow(unused_variables)]
                fn __invoke(#recv_decl2, #( #p_vars: #stripped ),*) -> #ret_ty {
                    #recv_call
                }
            }
            #[allow(non_camel_case_types)]
            trait __Go<__T> {
                fn __haphe_bind<__B: ::haphe::TypeBinder<__T>>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error>;
            }
            impl<__T> __Go<__T> for &::haphe::BridgeProbe<__T>
            where
                __T: __Call,
                #extra_bound
                #( __T::#p_assoc: ::haphe::FromScript, )*
                ::haphe::ScriptValue: ::core::convert::From<__T::__R>,
            {
                fn __haphe_bind<__B: ::haphe::TypeBinder<__T>>(
                    &self,
                    __b: &mut __B,
                ) -> ::core::result::Result<(), __B::Error> {
                    __b.#register(
                        #name,
                        (|#wrapper_sig, __args: &[::haphe::ScriptValue]|
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                            #wrapper_prep
                            #(
                                let #p_vars = <__T::#p_assoc as ::haphe::FromScript>::from_script(
                                    __args.get(#idx).cloned().unwrap_or(::haphe::ScriptValue::Unit)
                                )?;
                            )*
                            ::core::result::Result::Ok(::haphe::ScriptValue::from(
                                __T::__invoke(__t, #( #p_vars ),*)
                            ))
                        }) as #cast_ty
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError>,
                    )
                }
            }
            #[allow(unused_imports)]
            use ::haphe::SkipBind as _;
            (&&::haphe::BridgeProbe::<#self_ty>(::core::marker::PhantomData))
                .__haphe_bind(__b)?;
        }
    }
}

/// Registration for an `async` method. The receiver arrives as a
/// [`ScriptCow`] whose borrow the boxed future may hold across `await`s
/// (`&self` derefs, consuming `self` takes `into_owned`); `&mut self` goes
/// through the dedicated mutable channel, so mutation writes back in place.
fn gen_async_method_registration(self_ty: &Type, method: &BindMethod) -> TokenStream {
    let name = &method.name;
    let ident = &method.ident;

    let extractions = gen_param_extractions(&method.params);
    let call_args = gen_call_args(&method.params);

    let result_expr = if method.has_return {
        quote! { ::haphe::IntoScript::into_script(__t.#ident(#(#call_args),*).await) }
    } else {
        quote! { { __t.#ident(#(#call_args),*).await; ::haphe::ScriptValue::Unit } }
    };

    if matches!(method.receiver, ReceiverShape::RefMut) {
        return quote! {
            __b.method_async_mut(
                #name,
                (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                    ::std::boxed::Box::pin(async move {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    })
                }) as for<'a> fn(&'a mut #self_ty, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
            )?;
        };
    }

    let recv_prep = match method.receiver {
        ReceiverShape::Owned => quote! { let __t: #self_ty = __recv.into_owned(); },
        _ => quote! { let __t: &#self_ty = &__recv; },
    };

    quote! {
        __b.method_async(
            #name,
            (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                ::std::boxed::Box::pin(async move {
                    #recv_prep
                    #(#extractions)*
                    ::core::result::Result::Ok(#result_expr)
                })
            }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
        )?;
    }
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
                __b.method(
                    #name,
                    |__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        let __t: &#self_ty = &__recv;
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
                __b.method(
                    #name,
                    |__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
                        // Consuming receiver: clone-at-boundary only when the
                        // backend handed a borrowed carrier.
                        let __t: #self_ty = __recv.into_owned();
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
        ReceiverShape::None => {
            quote! {
                __b.method(
                    #name,
                    |_: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptConvertError> {
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
