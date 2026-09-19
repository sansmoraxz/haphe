//! Metamethod registration from `traits(...)` declarations.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Type};

use crate::attrs::TraitDecl;
use crate::ty_map::substitute_self;

// ---------------------------------------------------------------------------
// Metamethod registration
// ---------------------------------------------------------------------------

pub(crate) fn gen_metamethod_registrations(self_ty: &Type, traits: &[TraitDecl]) -> TokenStream {
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
                        )) as fn(&[::haphe::ScriptValue]) -> ::core::result::Result<#self_ty, ::haphe::ScriptCallError>,
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
