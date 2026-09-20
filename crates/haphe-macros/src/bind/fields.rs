//! Struct field registration: direct `field` channels for whitelisted types
//! and trait-presence dispatch blocks for the rest.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Type;

use super::BindField;
use super::gates::{is_bridge_value_type, is_generic_type_param, needs_bridge_dispatch};

// ---------------------------------------------------------------------------
// Field registration
// ---------------------------------------------------------------------------

/// A field registration that compiles to a real registration when the field
/// type implements the bridge traits (e.g. a transparent primitive newtype)
/// and to a no-op otherwise — decided at compile time via autoref
/// specialization, never at runtime.
pub(crate) fn gen_dispatched_field_registration(self_ty: &Type, f: &BindField) -> TokenStream {
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

pub(crate) fn gen_field_registrations(
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
            // Value-whitelisted fields register directly below; dispatch
            // covers only the syntactically unjudgeable rest.
            .filter(|f| {
                !is_bridge_value_type(&f.ty) && needs_bridge_dispatch(&f.ty, generic_params)
            })
            .map(|f| gen_dispatched_field_registration(self_ty, f))
            .collect()
    };
    // The full value-type whitelist (containers, std semantic types), like
    // methods and free functions — fields must not be narrower. OWNED types
    // only: the `field` channel's accessors clone and store by value.
    let regs = fields
        .iter()
        .filter(|f| {
            (!super::gates::is_reference(&f.ty) && is_bridge_value_type(&f.ty))
                || is_generic_type_param(&f.ty, generic_params)
        })
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
