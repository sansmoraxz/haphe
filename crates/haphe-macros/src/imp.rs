//! Expansion of `#[script]` on `impl` blocks: collects methods, constructors,
//! and properties into a `ScriptImpl` implementation.

use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Attribute, ImplItem, ItemImpl, Type};

use crate::attrs::{Errors, option_str_tokens, parse_fn_args, strip_script_attrs};
use crate::fn_desc::{ReceiverShape, build_fn_info, strip_param_script_attrs};
use crate::ty_map::TyCtx;

struct Getter {
    doc: Option<String>,
    descriptor_ty: TokenStream,
    ident: syn::Ident,
    is_async: bool,
    ret_ty: Type,
}

struct Setter {
    doc: Option<String>,
    param_ty: Type,
    descriptor_ty: TokenStream,
    span: proc_macro2::Span,
    ident: syn::Ident,
    is_async: bool,
}

/// A descriptor entry gated by the function's `#[cfg(...)]` attributes, so
/// conditionally compiled methods are described only when they exist.
struct Entry {
    cfgs: Vec<Attribute>,
    descriptor: TokenStream,
}

impl Entry {
    fn tokens(&self) -> TokenStream {
        let (cfgs, descriptor) = (&self.cfgs, &self.descriptor);
        quote! { #(#cfgs)* #descriptor }
    }
}

fn cfg_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .cloned()
        .collect()
}

/// Strips every `#[script(...)]` helper from the block's functions and their
/// parameters, so error paths never re-emit attributes that would cascade.
pub(crate) fn strip_impl_script_attrs(item: &mut ItemImpl) {
    for impl_item in &mut item.items {
        if let ImplItem::Fn(func) = impl_item {
            strip_script_attrs(&mut func.attrs);
            strip_param_script_attrs(&mut func.sig);
        }
    }
}

pub fn expand(mut item: ItemImpl) -> TokenStream {
    let mut errors = Errors::default();

    if let Some((path, _)) = &item.trait_ {
        errors.spanned(
            path.span(),
            "`#[script]` goes on inherent impl blocks, not trait impls",
        );
    }
    let self_ty = (*item.self_ty).clone();
    if !matches!(self_ty, Type::Path(_)) {
        errors.spanned(self_ty.span(), "`#[script]` requires a named self type");
    }
    if let Err(err) = errors.finish() {
        strip_impl_script_attrs(&mut item);
        let compile_error = err.to_compile_error();
        return quote! { #item #compile_error };
    }
    let mut errors = Errors::default();

    let impl_generic_names: Vec<String> = item
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(tp.ident.to_string()),
            _ => None,
        })
        .collect();
    let has_type_params = !impl_generic_names.is_empty();
    let ctx = TyCtx {
        generic_params: &impl_generic_names,
        self_ty: Some(&self_ty),
    };

    let mut methods: Vec<Entry> = Vec::new();
    let mut constructors: Vec<Entry> = Vec::new();
    let mut getters: BTreeMap<String, Getter> = BTreeMap::new();
    let mut setters: BTreeMap<String, Setter> = BTreeMap::new();
    let mut probes = Vec::new();

    // Bridge: collect method/constructor info for ScriptBind generation.
    let mut bind_methods: Vec<crate::bind::BindMethod> = Vec::new();
    let mut async_methods: Vec<crate::bind::BindMethod> = Vec::new();
    let mut dispatch_methods: Vec<crate::bind::BindMethod> = Vec::new();
    let mut bind_constructors: Vec<crate::bind::BindMethod> = Vec::new();
    let mut async_constructors: Vec<crate::bind::BindMethod> = Vec::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(func) = impl_item else {
            continue;
        };
        let fn_args = parse_fn_args(&func.attrs, &mut errors, "an impl-block function");
        if let Some(span) = fn_args.dyn_dispatch {
            errors.spanned(
                span,
                "`dyn` dispatch is not supported on impl-block functions yet; \
                 it applies to generic free functions",
            );
        }
        strip_script_attrs(&mut func.attrs);
        if let Some((_, span)) = fn_args.instantiate.first() {
            errors.spanned(
                *span,
                "`instantiate` is not supported on impl-block functions",
            );
        }
        if fn_args.skip.is_some() {
            strip_param_script_attrs(&mut func.sig);
            continue;
        }
        let Some(info) = build_fn_info(&mut func.sig, &fn_args, &func.attrs, &ctx, &mut errors)
        else {
            continue;
        };
        let cfgs = cfg_attrs(&func.attrs);
        let is_accessor = fn_args.getter.is_some() || fn_args.setter.is_some();
        if is_accessor {
            if let Some(kind) = &fn_args.error_kind {
                errors.spanned(kind.span(), "properties cannot carry `error_kind`");
                continue;
            }
            if !cfgs.is_empty() {
                errors.spanned(
                    cfgs[0].span(),
                    "`#[cfg]` on property accessors is not supported; \
                     gate the whole `impl` block instead",
                );
                continue;
            }
        }

        if let Some(span) = fn_args.constructor {
            if info.receiver != ReceiverShape::None {
                errors.spanned(span, "constructors cannot take a `self` receiver");
                continue;
            }
            // Constructors must produce the type: accept `Self` and
            // `Result<Self, E>`, checked through the compiler so aliases work.
            let Some(ret_ty) = &info.return_ty else {
                errors.spanned(span, "constructors must return `Self`");
                continue;
            };
            let target = constructor_success_type(ret_ty);
            if !has_type_params {
                probes.push(quote_spanned! {target.span()=>
                    const _: () = {
                        fn __haphe_constructor_returns_self(value: #target) -> #self_ty {
                            value
                        }
                    };
                });
            }
            // Bridge: register non-async, non-cfg-gated, infallible constructors.
            // Fallible constructors (-> Result<Self, E>) need error mapping
            // which is deferred to a future iteration.
            let is_fallible = info.return_ty.as_ref().is_some_and(|ty| {
                if let Type::Path(p) = ty {
                    p.path.segments.last().is_some_and(|s| s.ident == "Result")
                } else {
                    false
                }
            });
            if cfgs.is_empty() && !is_fallible {
                if info.is_async {
                    async_constructors.push(extract_bind_method(func, &info));
                } else {
                    bind_constructors.push(extract_bind_method(func, &info));
                }
            }
            constructors.push(Entry {
                cfgs,
                descriptor: info.descriptor,
            });
        } else if let Some(span) = fn_args.getter {
            if info.receiver != ReceiverShape::Ref {
                errors.spanned(span, "getters must take `&self`");
                continue;
            }
            if info.param_count != 0 {
                errors.spanned(span, "getters cannot take parameters besides `&self`");
                continue;
            }
            let Some(ret_ty) = info.return_ty else {
                errors.spanned(span, "getters must return a value");
                continue;
            };
            let descriptor_ty = match crate::ty_map::descriptor_expr(&ret_ty, &ctx) {
                Ok(expr) => expr,
                Err(err) => {
                    errors.push(err);
                    continue;
                }
            };
            if getters
                .insert(
                    info.name.clone(),
                    Getter {
                        doc: info.doc,
                        descriptor_ty,
                        ident: func.sig.ident.clone(),
                        is_async: info.is_async,
                        ret_ty,
                    },
                )
                .is_some()
            {
                errors.spanned(
                    span,
                    format!("duplicate getter for property `{}`", info.name),
                );
            }
        } else if let Some((rename, span)) = &fn_args.setter {
            if info.receiver != ReceiverShape::RefMut {
                errors.spanned(*span, "setters must take `&mut self`");
                continue;
            }
            if info.return_ty.is_some() {
                errors.spanned(*span, "setters cannot return a value");
                continue;
            }
            let Some(param_ty) = info.single_param_ty else {
                errors.spanned(*span, "setters must take exactly one parameter");
                continue;
            };
            let prop_name = match rename {
                Some(name) => name.value(),
                None => match info.name.strip_prefix("set_") {
                    Some(stripped) => stripped.to_string(),
                    None => {
                        errors.spanned(
                            *span,
                            "setter names must start with `set_` (or use `setter = \"name\"`)",
                        );
                        continue;
                    }
                },
            };
            let descriptor_ty = match crate::ty_map::descriptor_expr(&param_ty, &ctx) {
                Ok(expr) => expr,
                Err(err) => {
                    errors.push(err);
                    continue;
                }
            };
            if setters
                .insert(
                    prop_name.clone(),
                    Setter {
                        doc: info.doc,
                        param_ty,
                        descriptor_ty,
                        span: *span,
                        ident: func.sig.ident.clone(),
                        is_async: info.is_async,
                    },
                )
                .is_some()
            {
                errors.spanned(
                    *span,
                    format!("duplicate setter for property `{prop_name}`"),
                );
            }
        } else {
            // Bridge: register non-async, non-cfg-gated methods with
            // bridge-compatible signatures. Generic impls bypass the
            // compatibility check — bounds are enforced at monomorphization.
            if cfgs.is_empty() {
                if info.is_async {
                    // Async methods bridge through the ScriptCow receiver
                    // path (borrowed guard or backend-acquired value);
                    // `&mut self` uses the dedicated borrowed-mutable
                    // channel, so mutation writes back in place. The
                    // syntactic whitelist is the criterion (the dispatch
                    // machinery stays sync-only for now).
                    if !has_type_params && is_bind_compatible(func, &info) {
                        async_methods.push(extract_bind_method(func, &info));
                    }
                } else if has_type_params || is_bind_compatible(func, &info) {
                    bind_methods.push(extract_bind_method(func, &info));
                } else if !has_type_params && is_dispatch_eligible(func, &info) {
                    // Types the whitelist can't judge (e.g. transparent
                    // primitive newtypes): registration is decided by
                    // compile-time trait-presence dispatch instead.
                    dispatch_methods.push(extract_bind_method(func, &info));
                }
            }
            methods.push(Entry {
                cfgs,
                descriptor: info.descriptor,
            });
        }
    }

    // Pair getters and setters into properties. The getter's and setter's
    // types must describe the same script type (checked structurally, so an
    // `&str` getter pairs with a `String` setter).
    let mut properties = Vec::new();
    let mut property_regs: Vec<TokenStream> = Vec::new();
    let mut has_async_props = false;
    for (name, getter) in &getters {
        let setter = setters.remove(name);
        let readonly = setter.is_none();
        // Properties bind only when their script types pass the bridge
        // whitelist (mirroring methods); others stay descriptor-only.
        let bindable = crate::bind::is_bridge_compatible_type(&getter.ret_ty)
            && setter
                .as_ref()
                .is_none_or(|st| crate::bind::is_bridge_compatible_type(&st.param_ty));
        if bindable {
            has_async_props |= getter.is_async || setter.as_ref().is_some_and(|st| st.is_async);
            property_regs.push(crate::bind::gen_property_registration(
                &self_ty,
                name,
                getter.ident.clone(),
                getter.is_async,
                &getter.ret_ty,
                setter
                    .as_ref()
                    .map(|st| (st.ident.clone(), st.is_async, st.param_ty.clone())),
            ));
        }
        if let Some(setter) = &setter {
            let (get_desc, set_desc) = (&getter.descriptor_ty, &setter.descriptor_ty);
            let message =
                format!("property `{name}`: the getter and setter describe different script types");
            probes.push(quote_spanned! {setter.param_ty.span()=>
                const _: () = ::core::assert!((#get_desc).const_eq(&#set_desc), #message);
            });
        }
        // The getter's doc names the property; a doc on the setter is the
        // fallback when the getter has none.
        let doc = option_str_tokens(
            &getter
                .doc
                .clone()
                .or_else(|| setter.as_ref().and_then(|s| s.doc.clone())),
        );
        let ty = &getter.descriptor_ty;
        properties.push(quote! {
            ::haphe::PropertyDescriptor {
                name: #name,
                doc: #doc,
                ty: &#ty,
                readonly: #readonly,
            }
        });
    }
    for (name, setter) in &setters {
        errors.spanned(
            setter.span,
            format!("setter for property `{name}` has no matching `#[script(getter)]`"),
        );
    }
    let _ = getters;

    let (impl_g, _ty_g, where_c) = item.generics.split_for_impl();

    if let Err(err) = errors.finish() {
        let compile_error = err.to_compile_error();
        let bind_stub = if let Type::Path(p) = &self_ty
            && let Some(seg) = p.path.segments.last()
        {
            let mod_ident = crate::bind::hidden_mod_ident(&seg.ident);
            quote! {
                #[automatically_derived]
                impl #impl_g #mod_ident::__BindMethods for #self_ty #where_c {
                    fn __bind_methods<__B: ::haphe::TypeBinder<Self>>(
                        __b: &mut __B,
                    ) -> ::core::result::Result<(), __B::Error> {
                        ::core::result::Result::Ok(())
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        return quote! {
            #item
            #compile_error
            #[automatically_derived]
            impl #impl_g ::haphe::ScriptImpl for #self_ty #where_c {
                const METHODS: &'static [::haphe::FunctionDescriptor<'static>] = &[];
                const CONSTRUCTORS: &'static [::haphe::FunctionDescriptor<'static>] = &[];
                const PROPERTIES: &'static [::haphe::PropertyDescriptor<'static>] = &[];
                const HAS_ASYNC: bool = false;
            }
            #bind_stub
        };
    }

    let reverse_probe = if item.generics.params.is_empty() {
        quote_spanned! {self_ty.span()=>
            const _: () = {
                const fn __c<T: ::haphe::__verify::HasScriptMethods + ?Sized>() {}
                __c::<#self_ty>()
            };
        }
    } else {
        TokenStream::new()
    };

    // Bridge: emit `impl __BindMethods` with methods + constructors.
    let bind_codegen = if let Type::Path(p) = &self_ty
        && let Some(seg) = p.path.segments.last()
    {
        crate::bind::gen_impl_bind_methods(crate::bind::BindImplInput {
            ident: &seg.ident,
            self_ty: &self_ty,
            methods: &bind_methods,
            async_methods: &async_methods,
            dispatch_methods: &dispatch_methods,
            constructors: &bind_constructors,
            async_constructors: &async_constructors,
            property_regs: &property_regs,
            generics: &item.generics,
        })
    } else {
        TokenStream::new()
    };

    let methods = methods.iter().map(Entry::tokens);
    let constructors = constructors.iter().map(Entry::tokens);

    quote! {
        #item

        #[automatically_derived]
        impl #impl_g ::haphe::ScriptImpl for #self_ty #where_c {
            const METHODS: &'static [::haphe::FunctionDescriptor<'static>] = &[#(#methods),*];
            const CONSTRUCTORS: &'static [::haphe::FunctionDescriptor<'static>] = &[#(#constructors),*];
            const PROPERTIES: &'static [::haphe::PropertyDescriptor<'static>] = &[#(#properties),*];
            const HAS_ASYNC: bool = ::haphe::any_async(Self::METHODS)
                || ::haphe::any_async(Self::CONSTRUCTORS)
                || #has_async_props;
        }
        #reverse_probe
        #(#probes)*
        #bind_codegen
    }
}

/// Checks whether a method's param and return types are all bridge-compatible
/// (primitives that implement IntoScript/FromScript).
/// A signature the syntactic whitelist rejected but whose bridgeability can
/// be decided by trait presence: every param (stripped of one reference) and
/// the return type is either whitelisted or an owned bare-ident path type,
/// and the method has a receiver.
fn is_dispatch_eligible(func: &syn::ImplItemFn, info: &crate::fn_desc::FnInfo) -> bool {
    use crate::bind::{is_bridge_compatible_type, is_dispatchable_path};
    let has_receiver = func
        .sig
        .inputs
        .first()
        .is_some_and(|a| matches!(a, syn::FnArg::Receiver(_)));
    if !has_receiver {
        return false;
    }
    for input in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_ty) = input
            && !is_bridge_compatible_type(&pat_ty.ty)
            && !is_dispatchable_path(&pat_ty.ty)
        {
            return false;
        }
    }
    if let Some(ret) = &info.return_ty
        && !is_bridge_compatible_type(ret)
        && !is_dispatchable_path(ret)
    {
        return false;
    }
    true
}

fn is_bind_compatible(func: &syn::ImplItemFn, info: &crate::fn_desc::FnInfo) -> bool {
    use crate::bind::is_bridge_compatible_type;
    for input in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_ty) = input
            && !is_bridge_compatible_type(&pat_ty.ty)
        {
            return false;
        }
    }
    if let Some(ret) = &info.return_ty
        && !is_bridge_compatible_type(ret)
    {
        return false;
    }
    true
}

/// Extracts bind-relevant info from a processed function.
fn extract_bind_method(
    func: &syn::ImplItemFn,
    info: &crate::fn_desc::FnInfo,
) -> crate::bind::BindMethod {
    let params: Vec<(syn::Ident, Type)> = func
        .sig
        .inputs
        .iter()
        .filter_map(|input| {
            let syn::FnArg::Typed(pat_ty) = input else {
                return None;
            };
            let syn::Pat::Ident(pi) = pat_ty.pat.as_ref() else {
                return None;
            };
            Some((pi.ident.clone(), (*pat_ty.ty).clone()))
        })
        .collect();

    crate::bind::BindMethod {
        ident: func.sig.ident.clone(),
        name: info.name.clone(),
        receiver: info.receiver,
        params,
        has_return: info.return_ty.is_some(),
        return_ty: info.return_ty.clone(),
    }
}

/// The type a constructor's success path produces: `T` for `-> T`, the `T` in
/// `-> Result<T, E>`.
fn constructor_success_type(ret: &Type) -> &Type {
    if let Type::Path(p) = ret
        && p.qself.is_none()
        && let Some(last) = p.path.segments.last()
        && last.ident == "Result"
        && let syn::PathArguments::AngleBracketed(args) = &last.arguments
        && let Some(syn::GenericArgument::Type(ok_ty)) = args.args.first()
    {
        return ok_ty;
    }
    ret
}
