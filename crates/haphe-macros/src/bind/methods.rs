//! Method, constructor, and property registration through the `TypeBinder`
//! channels, including trait-presence-dispatched and `dyn`-generic methods.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::fn_desc::ReceiverShape;

use super::gates::{is_reference, strip_ref, substitute_type_params};
use super::{BindMethod, GenericBindMethod};

// ---------------------------------------------------------------------------
// Method registration
// ---------------------------------------------------------------------------

/// Generates `FromScript` extraction code for method params.
pub(crate) fn gen_param_extractions(params: &[(Ident, Type)]) -> Vec<TokenStream> {
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

/// The wrapper expression producing the call's `ScriptValue` result:
/// infallible calls convert directly; fallible ones (`Result<T, E>` in the
/// declared signature) map `Err` into [`ScriptCallError::Host`], rendering
/// `E` via `Display` and tagging the declared `error_kind`.
pub(crate) fn gen_result_expr(method: &BindMethod, call: &TokenStream) -> TokenStream {
    if method.fallible {
        let kind = if let Some(kind) = &method.error_kind {
            quote! { ::core::option::Option::Some(#kind) }
        } else {
            quote! { ::core::option::Option::None }
        };
        return quote! {
            match #call {
                ::core::result::Result::Ok(__v) => ::haphe::IntoScript::into_script(__v),
                ::core::result::Result::Err(__e) => {
                    return ::core::result::Result::Err(::haphe::ScriptCallError::Host {
                        message: ::std::string::ToString::to_string(&__e),
                        kind: #kind,
                    });
                }
            }
        };
    }
    if method.has_return {
        quote! { ::haphe::IntoScript::into_script(#call) }
    } else {
        quote! { { #call; ::haphe::ScriptValue::Unit } }
    }
}

pub(crate) fn gen_call_args(params: &[(Ident, Type)]) -> Vec<TokenStream> {
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
#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
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
    let ret_ty: TokenStream = if let (Some(t), true) = (&method.return_ty, method.has_return) {
        quote! { #t }
    } else {
        quote! { () }
    };
    let idx: Vec<usize> = (0..stripped.len()).collect();

    #[allow(
        clippy::items_after_statements,
        reason = "used only by the match below"
    )]
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
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
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
                            -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError>,
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
pub(crate) fn gen_async_method_registration(self_ty: &Type, method: &BindMethod) -> TokenStream {
    let name = &method.name;
    let ident = &method.ident;

    let extractions = gen_param_extractions(&method.params);
    let call_args = gen_call_args(&method.params);

    let result_expr = gen_result_expr(method, &quote! { __t.#ident(#(#call_args),*).await });

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

    let recv_prep = if method.receiver == ReceiverShape::Owned {
        quote! { let __t: #self_ty = __recv.into_owned(); }
    } else {
        quote! { let __t: &#self_ty = &__recv; }
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

/// Registration for a computed property: sync accessors go through
/// `property_get`/`property_set`; async ones through the `ScriptCow` /
/// borrowed-mutable future channels (`property_get_async` /
/// `property_set_async`), mirroring the async-method receiver rules.
pub fn gen_property_registration(
    self_ty: &Type,
    name: &str,
    get_ident: &Ident,
    get_async: bool,
    get_ret: &Type,
    setter: Option<(Ident, bool, Type)>,
) -> TokenStream {
    // Reference-returning getters (`&str`, `&f64`) convert through ToOwned.
    let get_value = if is_reference(get_ret) {
        quote! { ::std::borrow::ToOwned::to_owned(__t.#get_ident()) }
    } else {
        quote! { __t.#get_ident() }
    };
    let get_value_async = if is_reference(get_ret) {
        quote! { ::std::borrow::ToOwned::to_owned(__t.#get_ident().await) }
    } else {
        quote! { __t.#get_ident().await }
    };
    let get_reg = if get_async {
        quote! {
            __b.property_get_async(
                #name,
                (|__recv: ::haphe::ScriptCow<'_, #self_ty>| -> ::haphe::ScriptCallFuture<'_> {
                    ::std::boxed::Box::pin(async move {
                        let __t: &#self_ty = &__recv;
                        ::core::result::Result::Ok(::haphe::IntoScript::into_script(
                            #get_value_async,
                        ))
                    })
                }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>) -> ::haphe::ScriptCallFuture<'a>,
            )?;
        }
    } else {
        quote! {
            __b.property_get(
                #name,
                (|__t: &#self_ty| ::haphe::IntoScript::into_script(#get_value))
                    as fn(&#self_ty) -> ::haphe::ScriptValue,
            )?;
        }
    };
    let set_reg = match setter {
        None => TokenStream::new(),
        Some((set_ident, set_async, param_ty)) => {
            let stripped = strip_ref(&param_ty);
            let arg = if is_reference(&param_ty) {
                quote! { &__v }
            } else {
                quote! { __v }
            };
            if set_async {
                quote! {
                    __b.property_set_async(
                        #name,
                        (|__t: &mut #self_ty, __value: ::haphe::ScriptValue| -> ::haphe::ScriptCallFuture<'_> {
                            ::std::boxed::Box::pin(async move {
                                let __v = <#stripped as ::haphe::FromScript>::from_script(__value)?;
                                __t.#set_ident(#arg).await;
                                ::core::result::Result::Ok(::haphe::ScriptValue::Unit)
                            })
                        }) as for<'a> fn(&'a mut #self_ty, ::haphe::ScriptValue) -> ::haphe::ScriptCallFuture<'a>,
                    )?;
                }
            } else {
                quote! {
                    __b.property_set(
                        #name,
                        (|__t: &mut #self_ty, __value: ::haphe::ScriptValue| -> ::core::result::Result<(), ::haphe::ScriptConvertError> {
                            let __v = <#stripped as ::haphe::FromScript>::from_script(__value)?;
                            __t.#set_ident(#arg);
                            ::core::result::Result::Ok(())
                        }) as fn(&mut #self_ty, ::haphe::ScriptValue) -> ::core::result::Result<(), ::haphe::ScriptConvertError>,
                    )?;
                }
            }
        }
    };
    quote! { #get_reg #set_reg }
}

/// Registration for an `async` constructor: the boxed future borrows the
/// argument slice and resolves to the constructed value.
pub(crate) fn gen_async_constructor_registration(self_ty: &Type, ctor: &BindMethod) -> TokenStream {
    let name = &ctor.name;
    let ident = &ctor.ident;
    let extractions = gen_param_extractions(&ctor.params);
    let call_args = gen_call_args(&ctor.params);
    let produce = gen_ctor_expr(ctor, quote! { <#self_ty>::#ident(#(#call_args),*).await });
    quote! {
        __b.constructor_async(
            #name,
            (|__args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCtorFuture<'_, #self_ty> {
                ::std::boxed::Box::pin(async move {
                    #(#extractions)*
                    ::core::result::Result::Ok(#produce)
                })
            }) as for<'a> fn(&'a [::haphe::ScriptValue]) -> ::haphe::ScriptCtorFuture<'a, #self_ty>,
        )?;
    }
}

/// The expression producing the constructed value: a fallible constructor's
/// `Err` maps into [`ScriptCallError::Host`], like [`gen_result_expr`].
fn gen_ctor_expr(ctor: &BindMethod, call: TokenStream) -> TokenStream {
    if !ctor.fallible {
        return call;
    }
    let kind = if let Some(kind) = &ctor.error_kind {
        quote! { ::core::option::Option::Some(#kind) }
    } else {
        quote! { ::core::option::Option::None }
    };
    quote! {
        match #call {
            ::core::result::Result::Ok(__v) => __v,
            ::core::result::Result::Err(__e) => {
                return ::core::result::Result::Err(::haphe::ScriptCallError::Host {
                    message: ::std::string::ToString::to_string(&__e),
                    kind: #kind,
                });
            }
        }
    }
}

pub(crate) fn gen_method_registration(self_ty: &Type, method: &BindMethod) -> TokenStream {
    let name = &method.name;
    let ident = &method.ident;

    let extractions = gen_param_extractions(&method.params);
    let call_args = gen_call_args(&method.params);

    let result_expr = gen_result_expr(method, &quote! { __t.#ident(#(#call_args),*) });

    match method.receiver {
        ReceiverShape::Ref => {
            quote! {
                __b.method(
                    #name,
                    |__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
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
                    |__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
        ReceiverShape::Owned => {
            quote! {
                __b.method(
                    #name,
                    |__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
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
            let result_expr =
                gen_result_expr(method, &quote! { <#self_ty>::#ident(#(#call_args),*) });
            quote! {
                __b.method(
                    #name,
                    |_: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                        #(#extractions)*
                        ::core::result::Result::Ok(#result_expr)
                    },
                )?;
            }
        }
    }
}

/// Registrations for a generic method: one monomorphized wrapper per
/// declared instantiation. `dyn` dispatch hands each candidate to the
/// descriptor-carrying `method_dyn*` channel matching the receiver and
/// asyncness (the descriptor is materialized as a block-local `static` so
/// candidates share one `&'static` ranking source); static dispatch hands
/// the same wrappers to `method_generic*`, keyed on `(name, type_args)`.
#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
pub fn gen_generic_method_registration(self_ty: &Type, dm: &GenericBindMethod) -> TokenStream {
    let descriptor = &dm.descriptor;
    let ident = &dm.method.ident;
    // The self type's generic identity, resolved per monomorph at bind time
    // (`<T as HapheType>::DESCRIPTOR` const-promotes in the generic bind
    // impl).
    let self_inst = if dm.self_params.is_empty() {
        quote! { ::haphe::SelfInstantiation::NONE }
    } else {
        let names: Vec<String> = dm
            .self_params
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let params = &dm.self_params;
        quote! {
            ::haphe::SelfInstantiation {
                params: &[#(
                    ::haphe::GenericParam {
                        name: #names,
                        bounds: &[],
                        default: ::core::option::Option::None,
                    }
                ),*],
                args: &[#( <#params as ::haphe::HapheType>::DESCRIPTOR ),*],
            }
        }
    };
    let regs: Vec<TokenStream> = dm
        .instantiations
        .iter()
        .map(|(types, span)| {
            let subst: std::collections::HashMap<String, Type> = dm
                .type_params
                .iter()
                .map(std::string::ToString::to_string)
                .zip(types.iter().cloned())
                .collect();
            let params: Vec<(Ident, Type)> = dm
                .method
                .params
                .iter()
                .map(|(n, t)| (n.clone(), substitute_type_params(t, &subst)))
                .collect();
            let extractions = gen_param_extractions(&params);
            let call_args = gen_call_args(&params);
            let type_args =
                quote! { &[#( <#types as ::haphe::HapheType>::DESCRIPTOR ),*] };
            let call = if dm.method.receiver == ReceiverShape::None { quote! { <#self_ty>::#ident::<#(#types),*>(#(#call_args),*) } } else { quote! { __t.#ident::<#(#types),*>(#(#call_args),*) } };
            let sync_result = gen_result_expr(&dm.method, &call);
            let async_result = gen_result_expr(&dm.method, &quote! { #call.await });
            let recv_prep = match dm.method.receiver {
                ReceiverShape::Owned => quote! { let __t: #self_ty = __recv.into_owned(); },
                ReceiverShape::Ref => quote! { let __t: &#self_ty = &__recv; },
                _ => TokenStream::new(),
            };
            if !dm.dyn_dispatch {
                let name = &dm.method.name;
                return match (dm.is_async, dm.method.receiver) {
                    (false, ReceiverShape::RefMut) => quote::quote_spanned! {*span=>
                        __b.method_generic_mut(
                            #name,
                            #type_args,
                            (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                                #(#extractions)*
                                ::core::result::Result::Ok(#sync_result)
                            }) as fn(&mut #self_ty, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError>,
                        )?;
                    },
                    (false, _) => quote::quote_spanned! {*span=>
                        __b.method_generic(
                            #name,
                            #type_args,
                            (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                                #recv_prep
                                #(#extractions)*
                                ::core::result::Result::Ok(#sync_result)
                            }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError>,
                        )?;
                    },
                    (true, ReceiverShape::RefMut) => quote::quote_spanned! {*span=>
                        __b.method_generic_async_mut(
                            #name,
                            #type_args,
                            (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                                ::std::boxed::Box::pin(async move {
                                    #(#extractions)*
                                    ::core::result::Result::Ok(#async_result)
                                })
                            }) as for<'a> fn(&'a mut #self_ty, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                        )?;
                    },
                    (true, _) => quote::quote_spanned! {*span=>
                        __b.method_generic_async(
                            #name,
                            #type_args,
                            (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                                ::std::boxed::Box::pin(async move {
                                    #recv_prep
                                    #(#extractions)*
                                    ::core::result::Result::Ok(#async_result)
                                })
                            }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                        )?;
                    },
                };
            }
            match (dm.is_async, dm.method.receiver) {
                (false, ReceiverShape::RefMut) => quote::quote_spanned! {*span=>
                    __b.method_dyn_mut(
                        &__HAPHE_DYN_DESC,
                        #type_args,
                        #self_inst,
                        (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                            #(#extractions)*
                            ::core::result::Result::Ok(#sync_result)
                        }) as fn(&mut #self_ty, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError>,
                    )?;
                },
                (false, _) => quote::quote_spanned! {*span=>
                    __b.method_dyn(
                        &__HAPHE_DYN_DESC,
                        #type_args,
                        #self_inst,
                        (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError> {
                            #recv_prep
                            #(#extractions)*
                            ::core::result::Result::Ok(#sync_result)
                        }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &[::haphe::ScriptValue]) -> ::core::result::Result<::haphe::ScriptValue, ::haphe::ScriptCallError>,
                    )?;
                },
                (true, ReceiverShape::RefMut) => quote::quote_spanned! {*span=>
                    __b.method_dyn_async_mut(
                        &__HAPHE_DYN_DESC,
                        #type_args,
                        #self_inst,
                        (|__t: &mut #self_ty, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                            ::std::boxed::Box::pin(async move {
                                #(#extractions)*
                                ::core::result::Result::Ok(#async_result)
                            })
                        }) as for<'a> fn(&'a mut #self_ty, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                    )?;
                },
                (true, _) => quote::quote_spanned! {*span=>
                    __b.method_dyn_async(
                        &__HAPHE_DYN_DESC,
                        #type_args,
                        #self_inst,
                        (|__recv: ::haphe::ScriptCow<'_, #self_ty>, __args: &[::haphe::ScriptValue]| -> ::haphe::ScriptCallFuture<'_> {
                            ::std::boxed::Box::pin(async move {
                                #recv_prep
                                #(#extractions)*
                                ::core::result::Result::Ok(#async_result)
                            })
                        }) as for<'a> fn(::haphe::ScriptCow<'a, #self_ty>, &'a [::haphe::ScriptValue]) -> ::haphe::ScriptCallFuture<'a>,
                    )?;
                },
            }
        })
        .collect();
    let shared_desc = if dm.dyn_dispatch {
        quote! { static __HAPHE_DYN_DESC: ::haphe::FunctionDescriptor<'static> = #descriptor; }
    } else {
        TokenStream::new()
    };
    quote! {
        {
            #shared_desc
            #(#regs)*
        }
    }
}

pub(crate) fn gen_constructor_registration(self_ty: &Type, ctor: &BindMethod) -> TokenStream {
    let name = &ctor.name;
    let ident = &ctor.ident;

    let extractions = gen_param_extractions(&ctor.params);
    let call_args = gen_call_args(&ctor.params);

    let produce = gen_ctor_expr(ctor, quote! { <#self_ty>::#ident(#(#call_args),*) });
    quote! {
        __b.constructor(
            #name,
            |__args: &[::haphe::ScriptValue]| -> ::core::result::Result<#self_ty, ::haphe::ScriptCallError> {
                #(#extractions)*
                ::core::result::Result::Ok(#produce)
            },
        )?;
    }
}
