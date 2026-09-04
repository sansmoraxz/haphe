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
}

struct Setter {
    doc: Option<String>,
    param_ty: Type,
    descriptor_ty: TokenStream,
    span: proc_macro2::Span,
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
    if !item.generics.params.is_empty() {
        errors.spanned(
            item.generics.span(),
            "`#[script]` on generic impl blocks is not supported yet",
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

    let ctx = TyCtx {
        generic_params: &[],
        self_ty: Some(&self_ty),
    };

    let mut methods: Vec<Entry> = Vec::new();
    let mut constructors: Vec<Entry> = Vec::new();
    let mut getters: BTreeMap<String, Getter> = BTreeMap::new();
    let mut setters: BTreeMap<String, Setter> = BTreeMap::new();
    let mut probes = Vec::new();

    for impl_item in &mut item.items {
        let ImplItem::Fn(func) = impl_item else {
            continue;
        };
        let fn_args = parse_fn_args(&func.attrs, &mut errors, "an impl-block function");
        strip_script_attrs(&mut func.attrs);
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
            probes.push(quote_spanned! {target.span()=>
                const _: () = {
                    fn __haphe_constructor_returns_self(value: #target) -> #self_ty {
                        value
                    }
                };
            });
            constructors.push(Entry {
                cfgs,
                descriptor: info.descriptor,
            });
        } else if let Some(span) = fn_args.getter {
            if info.is_async {
                errors.spanned(span, "getters cannot be `async`");
                continue;
            }
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
            if info.is_async {
                errors.spanned(*span, "setters cannot be `async`");
                continue;
            }
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
    for (name, getter) in &getters {
        let setter = setters.remove(name);
        let readonly = setter.is_none();
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

    if let Err(err) = errors.finish() {
        let compile_error = err.to_compile_error();
        // Best-effort `ScriptImpl` so the real mistake doesn't cascade into a
        // "no `#[script] impl` block was found" error on the derive.
        return quote! {
            #item
            #compile_error
            #[automatically_derived]
            impl ::haphe::ScriptImpl for #self_ty {
                const METHODS: &'static [::haphe::FunctionDescriptor<'static>] = &[];
                const CONSTRUCTORS: &'static [::haphe::FunctionDescriptor<'static>] = &[];
                const PROPERTIES: &'static [::haphe::PropertyDescriptor<'static>] = &[];
                const HAS_ASYNC: bool = false;
            }
        };
    }

    let reverse_probe = quote_spanned! {self_ty.span()=>
        const _: () = {
            const fn __c<T: ::haphe::__verify::HasScriptMethods + ?Sized>() {}
            __c::<#self_ty>()
        };
    };
    let methods = methods.iter().map(Entry::tokens);
    let constructors = constructors.iter().map(Entry::tokens);

    quote! {
        #item

        #[automatically_derived]
        impl ::haphe::ScriptImpl for #self_ty {
            const METHODS: &'static [::haphe::FunctionDescriptor<'static>] = &[#(#methods),*];
            const CONSTRUCTORS: &'static [::haphe::FunctionDescriptor<'static>] = &[#(#constructors),*];
            const PROPERTIES: &'static [::haphe::PropertyDescriptor<'static>] = &[#(#properties),*];
            // Derived from the descriptor slices, so `#[cfg]`-gated methods
            // count only when compiled in.
            const HAS_ASYNC: bool = ::haphe::any_async(Self::METHODS)
                || ::haphe::any_async(Self::CONSTRUCTORS);
        }
        #reverse_probe
        #(#probes)*
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
