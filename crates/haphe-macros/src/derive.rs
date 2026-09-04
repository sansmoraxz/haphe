//! Expansion of `#[derive(Script)]` for structs and enums.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, Ident, Type};

use crate::attrs::{
    ContainerArgs, Errors, ThreadSafetyKind, TraitDecl, extract_doc, parse_container_args,
    parse_field_args, parse_variant_args,
};
use crate::ty_map::{TyCtx, descriptor_expr, substitute_self};
use crate::verify;

pub fn expand(input: DeriveInput) -> TokenStream {
    match expand_inner(input) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error(),
    }
}

fn doc_tokens(doc: &Option<String>) -> TokenStream {
    crate::attrs::option_str_tokens(doc)
}

/// Declared generic type parameters, after rejecting the unsupported kinds.
fn generic_param_names(input: &DeriveInput, errors: &mut Errors) -> Vec<String> {
    let mut names = Vec::new();
    for param in &input.generics.params {
        match param {
            syn::GenericParam::Type(tp) => names.push(tp.ident.to_string()),
            syn::GenericParam::Lifetime(lt) => errors.spanned(
                lt.span(),
                "types with lifetime parameters cannot derive `Script`",
            ),
            syn::GenericParam::Const(cp) => errors.spanned(
                cp.span(),
                "types with const generic parameters cannot derive `Script`",
            ),
        }
    }
    names
}

/// `GenericParam` IR literals for the descriptor.
fn generic_param_exprs(input: &DeriveInput, ctx: &TyCtx, errors: &mut Errors) -> Vec<TokenStream> {
    let mut exprs = Vec::new();
    for param in &input.generics.params {
        let syn::GenericParam::Type(tp) = param else {
            continue;
        };
        let name = tp.ident.to_string();
        let bounds: Vec<String> = tp
            .bounds
            .iter()
            .filter_map(|b| match b {
                syn::TypeParamBound::Trait(t) => Some(stringify_bound(quote!(#t))),
                _ => None,
            })
            .collect();
        let default = match &tp.default {
            Some((_, ty)) => match descriptor_expr(ty, ctx) {
                Ok(desc) => quote! { ::core::option::Option::Some(&#desc) },
                Err(err) => {
                    errors.push(err);
                    quote! { ::core::option::Option::None }
                }
            },
            None => quote! { ::core::option::Option::None },
        };
        exprs.push(quote! {
            ::haphe::GenericParam {
                name: #name,
                bounds: &[#(#bounds),*],
                default: #default,
            }
        });
    }
    exprs
}

/// Renders a trait bound compactly, keeping only the whitespace that
/// separates words (`for<'a> PartialEq<&'a str>` — not `for < 'a > ...` and
/// not `for<'a>PartialEq<&'astr>`).
fn stringify_bound(tokens: TokenStream) -> String {
    let raw = tokens.to_string();
    let mut out = String::with_capacity(raw.len());
    let is_wordish = |c: char| c.is_alphanumeric() || c == '_' || c == '\'';
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' {
            if let (Some(prev), Some(&next)) = (out.chars().last(), chars.peek())
                && is_wordish(prev)
                && is_wordish(next)
                && prev != '\''
            {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn thread_safety_tokens(container: &ContainerArgs) -> TokenStream {
    match container.thread_safety {
        Some((ThreadSafetyKind::SendSync, _)) => quote! { ::haphe::ThreadSafety::SEND_SYNC },
        Some((ThreadSafetyKind::Send, _)) => quote! { ::haphe::ThreadSafety::SEND },
        Some((ThreadSafetyKind::None, _)) | None => quote! { ::haphe::ThreadSafety::NONE },
    }
}

/// Maps one field to a `FieldDescriptor` literal (or `None` if skipped).
fn field_expr(field: &syn::Field, ctx: &TyCtx, errors: &mut Errors) -> Option<TokenStream> {
    let args = parse_field_args(&field.attrs, errors);
    if args.skip.is_some() {
        return None;
    }
    let ident = field.ident.as_ref().expect("named field");
    let name = args
        .rename
        .as_ref()
        .map(|r| r.value())
        .unwrap_or_else(|| ident.unraw().to_string());
    let doc = doc_tokens(&extract_doc(&field.attrs));
    let readonly = args.readonly.is_some();
    let ty_expr = match args.bytes {
        Some(span) => {
            let probe = verify::bytes_probe(&field.ty, span);
            quote_spanned! {span=> { #probe ::haphe::TypeDescriptor::Bytes } }
        }
        None => match descriptor_expr(&field.ty, ctx) {
            Ok(expr) => expr,
            Err(err) => {
                errors.push(err);
                return None;
            }
        },
    };
    Some(quote! {
        ::haphe::FieldDescriptor {
            name: #name,
            doc: #doc,
            ty: &#ty_expr,
            readonly: #readonly,
        }
    })
}

/// How a declared claim (`traits(...)`, `thread_safety`) is verified.
///
/// Non-generic types get an immediate const probe with the error spanned at
/// the attribute. Generic types get a `where` predicate on the descriptor
/// impl instead, so the claim is checked at every instantiation that is
/// actually exposed.
enum Verifier {
    Probe(TokenStream),
    Predicate(TokenStream),
}

/// Maps a `traits(...)` declaration to its `TraitImpl` literal plus a
/// [`Verifier`] for the claim.
fn trait_impl_expr(
    decl: &TraitDecl,
    self_ty: &Type,
    ctx: &TyCtx,
    is_generic: bool,
    errors: &mut Errors,
) -> Option<(TokenStream, Verifier)> {
    let name = &decl.name;
    let name_str = name.to_string();
    let span = name.span();

    let markers: &[(&str, TokenStream)] = &[
        ("Display", quote!(::core::fmt::Display)),
        ("Debug", quote!(::core::fmt::Debug)),
        ("Hash", quote!(::core::hash::Hash)),
        ("PartialEq", quote!(::core::cmp::PartialEq)),
        ("Eq", quote!(::core::cmp::Eq)),
        ("PartialOrd", quote!(::core::cmp::PartialOrd)),
        ("Ord", quote!(::core::cmp::Ord)),
        ("Clone", quote!(::core::clone::Clone)),
        ("Default", quote!(::core::default::Default)),
    ];

    // The claimed trait, with its arguments, as verifiable bound tokens
    // (`::core::ops::Add<f64, Output = Point>`).
    let bound: TokenStream;
    let expr: TokenStream;

    if let Some((_, path)) = markers.iter().find(|(m, _)| *m == name_str) {
        if !decl.args.is_empty() {
            errors.spanned(span, format!("trait `{name_str}` takes no arguments"));
            return None;
        }
        let variant = Ident::new(&name_str, span);
        expr = quote! { ::haphe::TraitImpl::#variant };
        bound = path.clone();
    } else {
        // Resolves a named type argument, with an optional `Self` default.
        fn lookup(
            decl: &TraitDecl,
            self_ty: &Type,
            span: proc_macro2::Span,
            errors: &mut Errors,
            key: &str,
            default_self: bool,
        ) -> Option<Type> {
            for (arg_name, ty) in &decl.args {
                if arg_name == key {
                    return Some(ty.clone());
                }
            }
            if default_self {
                Some(self_ty.clone())
            } else {
                errors.spanned(
                    span,
                    format!("trait `{}` requires a `{key} = <type>` argument", decl.name),
                );
                None
            }
        }
        let known_args: &[&str] = match name_str.as_str() {
            "Add" | "Sub" | "Mul" | "Div" | "Rem" => &["rhs", "output"],
            "Neg" => &["output"],
            "Index" | "IndexMut" => &["index", "output"],
            "Iterator" | "IntoIterator" => &["item"],
            other => {
                errors.spanned(
                    span,
                    format!(
                        "unsupported trait `{other}` (expected one of: Display, Debug, Hash, \
                         PartialEq, Eq, PartialOrd, Ord, Clone, Default, Add, Sub, Mul, Div, Rem, \
                         Neg, Index, IndexMut, Iterator, IntoIterator)"
                    ),
                );
                return None;
            }
        };
        for (arg_name, _) in &decl.args {
            if !known_args.contains(&arg_name.to_string().as_str()) {
                errors.spanned(
                    arg_name.span(),
                    format!(
                        "unknown argument `{arg_name}` for trait `{name_str}` (expected: {})",
                        known_args.join(", ")
                    ),
                );
            }
        }

        let desc = |ty: &Type, errors: &mut Errors| -> TokenStream {
            match descriptor_expr(ty, ctx) {
                Ok(expr) => quote! { &#expr },
                Err(err) => {
                    errors.push(err);
                    quote! { &::haphe::TypeDescriptor::Unit }
                }
            }
        };
        let variant = Ident::new(&name_str, span);

        match name_str.as_str() {
            "Add" | "Sub" | "Mul" | "Div" | "Rem" => {
                let rhs_ty = lookup(decl, self_ty, span, errors, "rhs", true)?;
                let out_ty = lookup(decl, self_ty, span, errors, "output", true)?;
                let (rhs, output) = (desc(&rhs_ty, errors), desc(&out_ty, errors));
                let (rhs_sub, out_sub) =
                    (substitute_self(&rhs_ty, ctx), substitute_self(&out_ty, ctx));
                expr = quote! { ::haphe::TraitImpl::#variant { rhs: #rhs, output: #output } };
                bound = quote! { ::core::ops::#variant<#rhs_sub, Output = #out_sub> };
            }
            "Neg" => {
                let out_ty = lookup(decl, self_ty, span, errors, "output", true)?;
                let output = desc(&out_ty, errors);
                let out_sub = substitute_self(&out_ty, ctx);
                expr = quote! { ::haphe::TraitImpl::Neg { output: #output } };
                bound = quote! { ::core::ops::Neg<Output = #out_sub> };
            }
            "Index" | "IndexMut" => {
                let idx_ty = lookup(decl, self_ty, span, errors, "index", false)?;
                let out_ty = lookup(decl, self_ty, span, errors, "output", false)?;
                let (index, output) = (desc(&idx_ty, errors), desc(&out_ty, errors));
                let (idx_sub, out_sub) =
                    (substitute_self(&idx_ty, ctx), substitute_self(&out_ty, ctx));
                expr = quote! { ::haphe::TraitImpl::#variant { index: #index, output: #output } };
                bound = quote! { ::core::ops::#variant<#idx_sub, Output = #out_sub> };
            }
            "Iterator" | "IntoIterator" => {
                let item_ty = lookup(decl, self_ty, span, errors, "item", false)?;
                let item = desc(&item_ty, errors);
                let item_sub = substitute_self(&item_ty, ctx);
                expr = quote! { ::haphe::TraitImpl::#variant { item: #item } };
                bound = quote! { ::core::iter::#variant<Item = #item_sub> };
            }
            _ => unreachable!("filtered above"),
        }
    }

    let verifier = if is_generic {
        Verifier::Predicate(quote_spanned! {span=> #self_ty: #bound })
    } else {
        Verifier::Probe(quote_spanned! {span=>
            const _: () = { const fn __c<T: #bound + ?Sized>() {} __c::<#self_ty>() };
        })
    };
    Some((expr, verifier))
}

fn expand_inner(input: DeriveInput) -> syn::Result<TokenStream> {
    let mut errors = Errors::default();
    let container = parse_container_args(&input.attrs, &mut errors);

    // Newtypes are exposed as type aliases: single-field tuple structs
    // always, and single-field named structs when marked `transparent`.
    if let Data::Struct(data) = &input.data {
        match &data.fields {
            Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                return expand_newtype(&input, container, &unnamed.unnamed[0], errors);
            }
            Fields::Named(named) if named.named.len() == 1 && container.transparent.is_some() => {
                return expand_newtype(&input, container, &named.named[0], errors);
            }
            _ => {}
        }
    }
    if let Some(span) = container.transparent {
        errors.spanned(
            span,
            "`transparent` requires a struct with exactly one field (a newtype alias)",
        );
    }
    let doc = doc_tokens(&extract_doc(&input.attrs));

    let ident = &input.ident;
    let ident_str = ident.unraw().to_string();
    let display_name = container
        .rename
        .as_ref()
        .map(|r| r.value())
        .unwrap_or_else(|| ident_str.clone());

    let param_names = generic_param_names(&input, &mut errors);
    let is_generic = !param_names.is_empty();
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();
    let self_ty: Type = syn::parse_quote! { #ident #ty_g };
    let ctx = TyCtx {
        generic_params: &param_names,
        self_ty: Some(&self_ty),
    };

    if is_generic && let Some(span) = container.methods {
        errors.spanned(
            span,
            "`#[script(methods)]` is not supported on generic types yet",
        );
    }

    let id_expr = quote! {
        ::haphe::TypeId::new(::core::concat!(::core::module_path!(), "::", #ident_str))
    };

    // Trait impls, each with its claim verifier: a const probe for concrete
    // types, a where-predicate on the descriptor impl for generic ones.
    let mut trait_exprs = Vec::new();
    let mut probes = Vec::new();
    let mut predicates = Vec::new();
    for decl in &container.traits {
        if let Some((expr, verifier)) =
            trait_impl_expr(decl, &self_ty, &ctx, is_generic, &mut errors)
        {
            trait_exprs.push(expr);
            match verifier {
                Verifier::Probe(probe) => probes.push(probe),
                Verifier::Predicate(pred) => predicates.push(pred),
            }
        }
    }

    // Thread-safety claim verification, same split.
    match container.thread_safety {
        Some((ThreadSafetyKind::SendSync, span)) => {
            if is_generic {
                predicates.push(quote_spanned! {span=>
                    #self_ty: ::core::marker::Send + ::core::marker::Sync
                });
            } else {
                probes.push(verify::thread_safety_probe(&self_ty, span, true));
            }
        }
        Some((ThreadSafetyKind::Send, span)) => {
            if is_generic {
                predicates.push(quote_spanned! {span=> #self_ty: ::core::marker::Send });
            } else {
                probes.push(verify::thread_safety_probe(&self_ty, span, false));
            }
        }
        _ => {}
    }
    let thread_safety = thread_safety_tokens(&container);

    // The descriptor impl carries the claim predicates so a generic type's
    // claims are verified at every exposed instantiation.
    let mut desc_generics = input.generics.clone();
    for pred in &predicates {
        match syn::parse2::<syn::WherePredicate>(pred.clone()) {
            Ok(pred) => desc_generics.make_where_clause().predicates.push(pred),
            Err(err) => errors.push(err),
        }
    }
    let (desc_impl_g, desc_ty_g, desc_where_c) = desc_generics.split_for_impl();

    let generic_params = generic_param_exprs(&input, &ctx, &mut errors);

    // Methods handshake.
    let (methods, constructors, properties) = match container.methods {
        Some(span) => (
            quote_spanned! {span=> <#ident as ::haphe::ScriptImpl>::METHODS },
            quote_spanned! {span=> <#ident as ::haphe::ScriptImpl>::CONSTRUCTORS },
            quote_spanned! {span=> <#ident as ::haphe::ScriptImpl>::PROPERTIES },
        ),
        None => (quote! { &[] }, quote! { &[] }, quote! { &[] }),
    };
    let mut handshake = TokenStream::new();
    if let Some(span) = container.methods
        && !is_generic
    {
        handshake.extend(quote_spanned! {span=>
            #[automatically_derived]
            impl ::haphe::__verify::HasScriptMethods for #ident {}
        });
        // Async runtimes may or may not be multithreaded: a type with async
        // methods must make an explicit thread-safety claim.
        if container.thread_safety.is_none() {
            handshake.extend(quote_spanned! {span=>
                const _: () = ::core::assert!(
                    !<#ident as ::haphe::ScriptImpl>::HAS_ASYNC,
                    "types with async methods must declare #[script(thread_safety = ...)] explicitly"
                );
            });
        }
    }

    let body = match &input.data {
        Data::Struct(data) => {
            let field_exprs: Vec<TokenStream> = match &data.fields {
                Fields::Named(named) => named
                    .named
                    .iter()
                    .filter_map(|f| field_expr(f, &ctx, &mut errors))
                    .collect(),
                Fields::Unit => Vec::new(),
                Fields::Unnamed(_) => {
                    errors.spanned(
                        input.ident.span(),
                        "tuple structs with multiple fields cannot derive `Script`; \
                         use named fields (single-field newtypes are exposed as type aliases)",
                    );
                    Vec::new()
                }
            };
            quote! {
                #[automatically_derived]
                impl #desc_impl_g ::haphe::ScriptStruct for #ident #desc_ty_g #desc_where_c {
                    const DESCRIPTOR: ::haphe::StructDescriptor<'static> = ::haphe::StructDescriptor {
                        id: <Self as ::haphe::ScriptType>::ID,
                        name: #display_name,
                        doc: #doc,
                        fields: &[#(#field_exprs),*],
                        methods: #methods,
                        constructors: #constructors,
                        properties: #properties,
                        trait_impls: &[#(#trait_exprs),*],
                        thread_safety: #thread_safety,
                        generic_params: &[#(#generic_params),*],
                    };
                }
            }
        }
        Data::Enum(data) => {
            let mut variant_exprs = Vec::new();
            for variant in &data.variants {
                let args = parse_variant_args(&variant.attrs, &mut errors);
                if args.skip.is_some() {
                    continue;
                }
                let vname = args
                    .rename
                    .as_ref()
                    .map(|r| r.value())
                    .unwrap_or_else(|| variant.ident.unraw().to_string());
                let vdoc = doc_tokens(&extract_doc(&variant.attrs));
                let kind = match &variant.fields {
                    Fields::Unit => quote! { ::haphe::VariantKind::Unit },
                    Fields::Unnamed(unnamed) => {
                        let mut descs = Vec::new();
                        for field in &unnamed.unnamed {
                            if field.attrs.iter().any(|a| a.path().is_ident("script")) {
                                errors.spanned(
                                    field.span(),
                                    "`#[script]` attributes are not supported on tuple-variant fields",
                                );
                            }
                            match descriptor_expr(&field.ty, &ctx) {
                                Ok(expr) => descs.push(expr),
                                Err(err) => errors.push(err),
                            }
                        }
                        quote! { ::haphe::VariantKind::Tuple(&[#(#descs),*]) }
                    }
                    Fields::Named(named) => {
                        let fields: Vec<TokenStream> = named
                            .named
                            .iter()
                            .filter_map(|f| field_expr(f, &ctx, &mut errors))
                            .collect();
                        quote! { ::haphe::VariantKind::Struct(&[#(#fields),*]) }
                    }
                };
                variant_exprs.push(quote! {
                    ::haphe::EnumVariant { name: #vname, doc: #vdoc, kind: #kind }
                });
            }
            let enum_asserts = container.methods.map(|span| {
                quote_spanned! {span=>
                    const _: () = ::core::assert!(
                        <#ident as ::haphe::ScriptImpl>::CONSTRUCTORS.is_empty(),
                        "enums cannot declare #[script(constructor)] functions"
                    );
                    const _: () = ::core::assert!(
                        <#ident as ::haphe::ScriptImpl>::PROPERTIES.is_empty(),
                        "enums cannot declare #[script(getter)]/#[script(setter)] properties"
                    );
                }
            });
            quote! {
                #enum_asserts
                #[automatically_derived]
                impl #desc_impl_g ::haphe::ScriptEnum for #ident #desc_ty_g #desc_where_c {
                    const DESCRIPTOR: ::haphe::EnumDescriptor<'static> = ::haphe::EnumDescriptor {
                        id: <Self as ::haphe::ScriptType>::ID,
                        name: #display_name,
                        doc: #doc,
                        variants: &[#(#variant_exprs),*],
                        methods: #methods,
                        trait_impls: &[#(#trait_exprs),*],
                        thread_safety: #thread_safety,
                        generic_params: &[#(#generic_params),*],
                    };
                }
            }
        }
        Data::Union(u) => {
            return Err(syn::Error::new(
                u.union_token.span(),
                "unions cannot derive `Script`",
            ));
        }
    };

    errors.finish()?;

    Ok(quote! {
        #[automatically_derived]
        impl #impl_g ::haphe::HapheType for #ident #ty_g #where_c {
            const DESCRIPTOR: ::haphe::TypeDescriptor<'static> =
                ::haphe::TypeDescriptor::Ref(<Self as ::haphe::ScriptType>::ID);
        }
        #[automatically_derived]
        impl #impl_g ::haphe::ScriptType for #ident #ty_g #where_c {
            const ID: ::haphe::TypeId<'static> = #id_expr;
        }
        #body
        #handshake
        #(#probes)*
    })
}

/// Expansion for single-field tuple structs: a newtype exposed as a
/// [`TypeAliasDescriptor`](haphe_core::TypeAliasDescriptor) — transparent
/// (equivalent to the inner type) or a distinct named type.
fn expand_newtype(
    input: &DeriveInput,
    container: ContainerArgs,
    field: &syn::Field,
    mut errors: Errors,
) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let ident_str = ident.unraw().to_string();
    let display_name = container
        .rename
        .as_ref()
        .map(|r| r.value())
        .unwrap_or_else(|| ident_str.clone());
    let doc = doc_tokens(&extract_doc(&input.attrs));
    let transparent = container.transparent.is_some();

    if !input.generics.params.is_empty() {
        errors.spanned(
            input.generics.span(),
            "generic newtypes cannot derive `Script` yet",
        );
    }
    if let Some(span) = container.methods {
        errors.spanned(span, "type aliases cannot have `methods`");
    }
    if let Some(decl) = container.traits.first() {
        errors.spanned(
            decl.name.span(),
            "type aliases cannot declare `traits(...)`",
        );
    }
    if let Some((_, span)) = container.thread_safety {
        errors.spanned(span, "type aliases cannot declare `thread_safety`");
    }

    let field_args = parse_field_args(&field.attrs, &mut errors);
    for (key, span) in [
        ("rename", field_args.rename.as_ref().map(|r| r.span())),
        ("skip", field_args.skip),
        ("readonly", field_args.readonly),
    ] {
        if let Some(span) = span {
            errors.spanned(
                span,
                format!("`{key}` has no effect on a newtype's inner field"),
            );
        }
    }
    let ctx = TyCtx::default();
    let inner_desc = match field_args.bytes {
        Some(span) => {
            let probe = verify::bytes_probe(&field.ty, span);
            quote_spanned! {span=> { #probe ::haphe::TypeDescriptor::Bytes } }
        }
        None => match descriptor_expr(&field.ty, &ctx) {
            Ok(expr) => expr,
            Err(err) => {
                errors.push(err);
                quote! { ::haphe::TypeDescriptor::Unit }
            }
        },
    };

    errors.finish()?;

    let id_expr = quote! {
        ::haphe::TypeId::new(::core::concat!(::core::module_path!(), "::", #ident_str))
    };
    // Transparent newtypes describe as the inner type wherever they appear;
    // opaque ones as a reference to the registered alias.
    let haphe_type_desc = if transparent {
        quote! { #inner_desc }
    } else {
        quote! { ::haphe::TypeDescriptor::Ref(<Self as ::haphe::ScriptType>::ID) }
    };

    Ok(quote! {
        #[automatically_derived]
        impl ::haphe::HapheType for #ident {
            const DESCRIPTOR: ::haphe::TypeDescriptor<'static> = #haphe_type_desc;
        }
        #[automatically_derived]
        impl ::haphe::ScriptType for #ident {
            const ID: ::haphe::TypeId<'static> = #id_expr;
        }
        #[automatically_derived]
        impl ::haphe::ScriptAlias for #ident {
            const DESCRIPTOR: ::haphe::TypeAliasDescriptor<'static> = ::haphe::TypeAliasDescriptor {
                id: <Self as ::haphe::ScriptType>::ID,
                name: #display_name,
                doc: #doc,
                inner: &#inner_desc,
                transparent: #transparent,
            };
        }
    })
}
