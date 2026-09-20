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

#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
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
    let impl_type_param_idents: Vec<syn::Ident> = item
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(tp) => Some(tp.ident.clone()),
            _ => None,
        })
        .collect();
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
    let mut generic_methods: Vec<crate::bind::GenericBindMethod> = Vec::new();
    let mut bind_constructors: Vec<crate::bind::BindMethod> = Vec::new();
    let mut async_constructors: Vec<crate::bind::BindMethod> = Vec::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(func) = impl_item else {
            continue;
        };
        let mut fn_args = parse_fn_args(&func.attrs, &mut errors, "an impl-block function");
        strip_script_attrs(&mut func.attrs);
        if fn_args.skip.is_some() {
            strip_param_script_attrs(&mut func.sig);
            continue;
        }
        // A function's own generic parameters are supported on plain methods
        // only, in either dispatch mode: static (declared `instantiate(...)`
        // monomorphs keyed on (name, type_args)) or `dyn` (runtime candidate
        // scan). Bare `dyn` on a single-parameter generic gets the default
        // candidate set, like free functions.
        let fn_type_params: Vec<syn::Ident> = func
            .sig
            .generics
            .type_params()
            .map(|tp| tp.ident.clone())
            .collect();
        let has_fn_generics = !fn_type_params.is_empty();
        if has_fn_generics {
            let span = func.sig.generics.span();
            if fn_args.constructor.is_some() || fn_args.getter.is_some() || fn_args.setter.is_some()
            {
                errors.spanned(
                    span,
                    "generic parameters are not supported on constructors or property accessors",
                );
                strip_param_script_attrs(&mut func.sig);
                continue;
            }
            // Static monomorphs on a generic self type key on (name,
            // type_args) like anywhere else: the registration surface is
            // per-monomorph (each bound instantiation has its own metatable/
            // resource), so the self identity is implicit.
            if fn_args.dyn_dispatch.is_none() && fn_args.instantiate.is_empty() {
                // Static dispatch: monomorphs keyed on (name, type_args),
                // like static generic free functions — each use must be
                // declared.
                errors.spanned(
                    span,
                    "generic impl-block functions need `instantiate(...)` declarations \
                     (or `dyn` for runtime dispatch)",
                );
                strip_param_script_attrs(&mut func.sig);
                continue;
            }
            fn_args.inject_bare_dyn_defaults(fn_type_params.len());
        }
        // Generic methods see the impl's parameters plus their own.
        let fn_generic_names: Vec<String> = impl_generic_names
            .iter()
            .cloned()
            .chain(fn_type_params.iter().map(std::string::ToString::to_string))
            .collect();
        let fn_ctx = TyCtx {
            generic_params: &fn_generic_names,
            self_ty: Some(&self_ty),
        };
        let info_ctx = if has_fn_generics { &fn_ctx } else { &ctx };
        // A `Result<T, E>` return is fallible: the descriptor and the bridge
        // see `T` (mirroring foreign methods — the host receives the value
        // or a Callee error carrying `E` intact, tagged with the declared
        // `error_kind`), so `E` needs no bridge representation — only
        // `std::error::Error + Send + Sync`.
        let fallible = matches!(
            &func.sig.output,
            syn::ReturnType::Type(_, ret) if crate::fn_desc::result_types(ret).is_some()
        );
        let info = if fallible {
            let mut desc_sig = func.sig.clone();
            if let syn::ReturnType::Type(_, ret) = &func.sig.output
                && let Some((ok, err)) = crate::fn_desc::result_types(ret)
            {
                // The error crosses intact: `E: Error + Send + Sync` is the
                // requirement, checked here so the failure points at the
                // declared error type.
                if !has_fn_generics && !has_type_params {
                    let err = crate::ty_map::substitute_self(err, info_ctx);
                    probes.push(quote_spanned! {err.span()=>
                        const _: () = {
                            fn __haphe_fallible_error_bound<__E: ::std::error::Error + ::core::marker::Send + ::core::marker::Sync + 'static>() {}
                            let _ = __haphe_fallible_error_bound::<#err>;
                        };
                    });
                }
                let ok = ok.clone();
                // `Result<(), E>` is a unit return on the ok path.
                desc_sig.output = if matches!(&ok, syn::Type::Tuple(t) if t.elems.is_empty()) {
                    syn::ReturnType::Default
                } else {
                    syn::parse_quote! { -> #ok }
                };
            }
            let info = build_fn_info(
                &mut desc_sig,
                &fn_args,
                &func.attrs,
                info_ctx,
                true,
                &mut errors,
            );
            strip_param_script_attrs(&mut func.sig);
            info
        } else {
            build_fn_info(
                &mut func.sig,
                &fn_args,
                &func.attrs,
                info_ctx,
                false,
                &mut errors,
            )
        };
        let Some(info) = info else {
            continue;
        };
        let cfgs = cfg_attrs(&func.attrs);
        let is_accessor = fn_args.getter.is_some() || fn_args.setter.is_some();
        if is_accessor {
            if fallible {
                let span = fn_args.getter.or(fn_args.setter.as_ref().map(|(_, s)| *s));
                errors.spanned(
                    span.expect("accessor implies a getter/setter span"),
                    "property accessors cannot return `Result`; expose a fallible method instead",
                );
                continue;
            }
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
            // Bridge: register non-cfg-gated constructors; a fallible one
            // maps its `Err` into a Callee error inside the wrapper.
            if cfgs.is_empty() {
                let bind = extract_bind_method(func, &info, fallible, &fn_args);
                if info.is_async {
                    async_constructors.push(bind);
                } else {
                    bind_constructors.push(bind);
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
                None => {
                    if let Some(stripped) = info.name.strip_prefix("set_") {
                        stripped.to_string()
                    } else {
                        errors.spanned(
                            *span,
                            "setter names must start with `set_` (or use `setter = \"name\"`)",
                        );
                        continue;
                    }
                }
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
            if has_fn_generics {
                // Generic methods: each declared instantiation must produce a
                // bridgeable signature — an unbindable candidate is a
                // compile error, never a silent drop.
                let method = extract_bind_method(func, &info, fallible, &fn_args);
                let render = |t: &Type| {
                    quote::quote!(#t)
                        .to_string()
                        .replace(" :: ", "::")
                        .replace("< ", "<")
                        .replace(" >", ">")
                };
                for (types, span) in &fn_args.instantiate {
                    // Substitute the method's OWN parameters; the impl's stay
                    // (they resolve at the self type's monomorphization, with
                    // FromScript/IntoScript bounds on the bind impl).
                    let subst: std::collections::HashMap<String, Type> = fn_type_params
                        .iter()
                        .map(std::string::ToString::to_string)
                        .zip(types.iter().cloned())
                        .collect();
                    for (_, ty) in &method.params {
                        let t = crate::bind::strip_ref(&crate::bind::substitute_type_params(
                            ty, &subst,
                        ));
                        if crate::bind::is_generic_type_param(&t, &impl_generic_names) {
                            continue;
                        }
                        if !crate::bind::is_bridge_value_type(&t) {
                            errors.spanned(
                                *span,
                                format!(
                                    "this instantiation gives parameter type `{}`, which \
                                     cannot cross the bridge",
                                    render(&t)
                                ),
                            );
                        }
                    }
                    if let Some(ret) = &method.return_ty {
                        let t = crate::bind::substitute_type_params(ret, &subst);
                        if crate::bind::is_generic_type_param(&t, &impl_generic_names) {
                            continue;
                        }
                        if matches!(t, Type::Reference(_)) || !crate::bind::is_bridge_value_type(&t)
                        {
                            errors.spanned(
                                *span,
                                format!(
                                    "this instantiation gives return type `{}`, which \
                                     cannot cross the bridge",
                                    render(&t)
                                ),
                            );
                        }
                    }
                }
                if cfgs.is_empty() {
                    generic_methods.push(crate::bind::GenericBindMethod {
                        method,
                        self_params: impl_type_param_idents.clone(),
                        type_params: fn_type_params.clone(),
                        instantiations: fn_args.instantiate.clone(),
                        is_async: info.is_async,
                        dyn_dispatch: fn_args.dyn_dispatch.is_some(),
                        descriptor: info.descriptor.clone(),
                    });
                }
            } else if cfgs.is_empty() {
                if info.is_async {
                    // Async methods bridge through the ScriptCow receiver
                    // path (borrowed guard or backend-acquired value);
                    // `&mut self` uses the dedicated borrowed-mutable
                    // channel, so mutation writes back in place. Types the
                    // whitelist can't judge fall back to trait-presence
                    // dispatch, like sync methods.
                    if !has_type_params && is_bind_compatible(func, &info) {
                        async_methods.push(extract_bind_method(func, &info, fallible, &fn_args));
                    } else if !has_type_params && is_dispatch_eligible(func, &info) {
                        dispatch_methods.push(extract_bind_method(func, &info, fallible, &fn_args));
                    } else if !has_type_params {
                        reject_nested_references(func, &info, &mut errors);
                    }
                } else if has_type_params || is_bind_compatible(func, &info) {
                    bind_methods.push(extract_bind_method(func, &info, fallible, &fn_args));
                } else if !has_type_params && is_dispatch_eligible(func, &info) {
                    // Types the whitelist can't judge (e.g. transparent
                    // primitive newtypes): registration is decided by
                    // compile-time trait-presence dispatch instead.
                    dispatch_methods.push(extract_bind_method(func, &info, fallible, &fn_args));
                } else if !has_type_params {
                    reject_nested_references(func, &info, &mut errors);
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
        // whitelist (mirroring methods, full value-type gate); others stay
        // descriptor-only.
        let bridgeable = |ty: &Type| {
            crate::bind::is_bridge_compatible_type(ty)
                || crate::bind::is_bridge_value_type(&crate::bind::strip_ref(ty))
        };
        let bindable =
            bridgeable(&getter.ret_ty) && setter.as_ref().is_none_or(|st| bridgeable(&st.param_ty));
        if !bindable {
            for ty in std::iter::once(&getter.ret_ty).chain(setter.as_ref().map(|st| &st.param_ty))
            {
                if let Some(span) = crate::bind::nested_reference_span(ty) {
                    errors.spanned(
                        span,
                        "references inside composite types cannot cross the bridge; \
                         use owned element types (e.g. `Vec<String>` instead of `Vec<&str>`)",
                    );
                    break;
                }
            }
        }
        if bindable {
            has_async_props |= getter.is_async || setter.as_ref().is_some_and(|st| st.is_async);
            property_regs.push(crate::bind::gen_property_registration(
                &self_ty,
                name,
                &getter.ident,
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
            getter
                .doc
                .as_deref()
                .or_else(|| setter.as_ref().and_then(|s| s.doc.as_deref())),
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
        crate::bind::gen_impl_bind_methods(&crate::bind::BindImplInput {
            ident: &seg.ident,
            self_ty: &self_ty,
            methods: &bind_methods,
            async_methods: &async_methods,
            dispatch_methods: &dispatch_methods,
            generic_methods: &generic_methods,
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
/// the return type is either whitelisted or an owned bare-ident path type.
/// Receiver-less fns are eligible like methods; they register through the
/// `associated` channels.
fn is_dispatch_eligible(func: &syn::ImplItemFn, info: &crate::fn_desc::FnInfo) -> bool {
    use crate::bind::{is_bridge_compatible_type, is_dispatchable_path};
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
    use crate::bind::{is_bridge_compatible_type, is_bridge_value_type, is_reference, strip_ref};
    for input in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_ty) = input
            && !is_bridge_compatible_type(&pat_ty.ty)
            // The full value-type whitelist (containers, std semantic types),
            // judged like the wrapper converts: one reference stripped. Free
            // functions use the same gate — methods must not be narrower.
            && !is_bridge_value_type(&strip_ref(&pat_ty.ty))
        {
            return false;
        }
    }
    if let Some(ret) = &info.return_ty
        && !is_bridge_compatible_type(ret)
        && (is_reference(ret) || !is_bridge_value_type(ret))
    {
        return false;
    }
    true
}

/// The one unbindable cause we can name precisely: a reference nested inside
/// a composite type has no bridge conversion at any surface — error at the
/// declaration instead of leaving the method silently descriptor-only.
fn reject_nested_references(
    func: &syn::ImplItemFn,
    info: &crate::fn_desc::FnInfo,
    errors: &mut Errors,
) {
    let tys = func
        .sig
        .inputs
        .iter()
        .filter_map(|input| match input {
            syn::FnArg::Typed(pat_ty) => Some(&*pat_ty.ty),
            syn::FnArg::Receiver(_) => None,
        })
        .chain(info.return_ty.as_ref());
    for ty in tys {
        if let Some(span) = crate::bind::nested_reference_span(ty) {
            errors.spanned(
                span,
                "references inside composite types cannot cross the bridge; \
                 use owned element types (e.g. `Vec<String>` instead of `Vec<&str>`)",
            );
            return;
        }
    }
}

/// Extracts bind-relevant info from a processed function.
fn extract_bind_method(
    func: &syn::ImplItemFn,
    info: &crate::fn_desc::FnInfo,
    fallible: bool,
    fn_args: &crate::attrs::FnArgs,
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
        fallible,
        error_kind: fn_args.error_kind.as_ref().map(syn::LitStr::value),
        is_async: info.is_async,
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
