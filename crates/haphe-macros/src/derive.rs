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
            syn::GenericParam::Lifetime(_) => {}
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
pub(crate) fn stringify_bound(tokens: TokenStream) -> String {
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
        ("ToString", quote!(::std::string::ToString)),
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
            "Add" | "Sub" | "Mul" | "Div" | "Rem" | "IDiv" | "Mod" | "BitAnd" | "BitOr"
            | "BitXor" | "Shl" | "Shr" | "Pow" => &["rhs", "output"],
            "Neg" | "Not" => &["output"],
            "Index" | "IndexMut" => &["index", "output"],
            "Call" | "AsyncCall" => &["args", "output"],
            "Iterator" | "IntoIterator" => &["item"],
            other => {
                errors.spanned(
                    span,
                    format!(
                        "unsupported trait `{other}` (expected one of: Display, Debug, Hash, \
                         PartialEq, Eq, PartialOrd, Ord, Clone, Default, Add, Sub, Mul, Div, Rem, \
                         IDiv, Mod, Neg, BitAnd, BitOr, BitXor, Shl, Shr, Not, Pow, Call, \
                         AsyncCall, Index, IndexMut, Iterator, IntoIterator)"
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
            "Add" | "Sub" | "Mul" | "Div" | "Rem" | "IDiv" | "Mod" | "BitAnd" | "BitOr"
            | "BitXor" | "Shl" | "Shr" | "Pow" => {
                let rhs_ty = lookup(decl, self_ty, span, errors, "rhs", true)?;
                let out_ty = lookup(decl, self_ty, span, errors, "output", true)?;
                let (rhs, output) = (desc(&rhs_ty, errors), desc(&out_ty, errors));
                let (rhs_sub, out_sub) =
                    (substitute_self(&rhs_ty, ctx), substitute_self(&out_ty, ctx));
                expr = quote! { ::haphe::TraitImpl::#variant { rhs: #rhs, output: #output } };
                bound = quote! { ::haphe::ops::#variant<#rhs_sub, Output = #out_sub> };
            }
            "Neg" | "Not" => {
                let out_ty = lookup(decl, self_ty, span, errors, "output", true)?;
                let output = desc(&out_ty, errors);
                let out_sub = substitute_self(&out_ty, ctx);
                expr = quote! { ::haphe::TraitImpl::#variant { output: #output } };
                bound = quote! { ::haphe::ops::#variant<Output = #out_sub> };
            }
            "Index" | "IndexMut" => {
                let idx_ty = lookup(decl, self_ty, span, errors, "index", false)?;
                let out_ty = lookup(decl, self_ty, span, errors, "output", false)?;
                let (index, output) = (desc(&idx_ty, errors), desc(&out_ty, errors));
                let (idx_sub, out_sub) =
                    (substitute_self(&idx_ty, ctx), substitute_self(&out_ty, ctx));
                expr = quote! { ::haphe::TraitImpl::#variant { index: #index, output: #output } };
                bound = quote! { ::haphe::ops::#variant<#idx_sub, Output = #out_sub> };
            }
            "Call" | "AsyncCall" => {
                let args_ty = lookup(decl, self_ty, span, errors, "args", false)?;
                let out_ty = lookup(decl, self_ty, span, errors, "output", true)?;
                let Type::Tuple(args_tuple) = &args_ty else {
                    errors.spanned(
                        span,
                        format!(
                            "`{name_str}`'s `args` must be a tuple type like `(i64, String)` \
                             (use `()` for no parameters)"
                        ),
                    );
                    return None;
                };
                let arg_descs: Vec<TokenStream> =
                    args_tuple.elems.iter().map(|ty| desc(ty, errors)).collect();
                let output = desc(&out_ty, errors);
                let (args_sub, out_sub) = (
                    substitute_self(&args_ty, ctx),
                    substitute_self(&out_ty, ctx),
                );
                expr = quote! {
                    ::haphe::TraitImpl::#variant { args: &[#(*#arg_descs),*], output: #output }
                };
                bound = quote! { ::haphe::ops::#variant<#args_sub, Output = #out_sub> };
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

    // Async callability implies suspension points; mirror the async-methods
    // rule and demand an explicit thread-safety claim.
    if container.thread_safety.is_none()
        && let Some(decl) = container.traits.iter().find(|t| t.name == "AsyncCall")
    {
        errors.spanned(
            decl.name.span(),
            "types declaring `AsyncCall` must declare #[script(thread_safety = ...)] explicitly",
        );
    }

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
            quote_spanned! {span=> <#self_ty as ::haphe::ScriptImpl>::METHODS },
            quote_spanned! {span=> <#self_ty as ::haphe::ScriptImpl>::CONSTRUCTORS },
            quote_spanned! {span=> <#self_ty as ::haphe::ScriptImpl>::PROPERTIES },
        ),
        None => (quote! { &[] }, quote! { &[] }, quote! { &[] }),
    };
    let mut handshake = TokenStream::new();
    if let Some(span) = container.methods {
        handshake.extend(quote_spanned! {span=>
            #[automatically_derived]
            impl #impl_g ::haphe::__verify::HasScriptMethods for #ident #ty_g #where_c {}
        });
        if container.thread_safety.is_none() && input.generics.params.is_empty() {
            handshake.extend(quote_spanned! {span=>
                const _: () = ::core::assert!(
                    !<#self_ty as ::haphe::ScriptImpl>::HAS_ASYNC,
                    "types with async methods must declare #[script(thread_safety = ...)] explicitly"
                );
            });
        }
    }

    let body = match &input.data {
        Data::Struct(data) => {
            if let Some(span) = container.flags {
                errors.spanned(span, "`#[script(flags)]` is only valid on enums");
            }
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
            let is_flags = container.flags.is_some();
            // Numeric script representation propagates the EXACT Rust
            // `#[repr]` integer type; without one, the enum crosses as its
            // declared case names.
            let repr_prim: Option<&str> = input.attrs.iter().find_map(|attr| {
                if !attr.path().is_ident("repr") {
                    return None;
                }
                let mut found = None;
                let _ = attr.parse_nested_meta(|meta| {
                    if let Some(ident) = meta.path.get_ident() {
                        let name = ident.to_string();
                        if matches!(
                            name.as_str(),
                            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"
                        ) {
                            found = Some(match name.as_str() {
                                "i8" => "I8",
                                "i16" => "I16",
                                "i32" => "I32",
                                "i64" => "I64",
                                "u8" => "U8",
                                "u16" => "U16",
                                "u32" => "U32",
                                _ => "U64",
                            });
                        }
                    }
                    Ok(())
                });
                found
            });
            let numeric = repr_prim.is_some() && !is_flags;
            let repr_expr = match (repr_prim, is_flags) {
                (Some(prim), false) => {
                    let ident = Ident::new(prim, proc_macro2::Span::call_site());
                    quote! { ::core::option::Option::Some(::haphe::PrimitiveType::#ident) }
                }
                _ => quote! { ::core::option::Option::None },
            };
            let mut next_discriminant: i64 = 0;
            let mut variant_exprs = Vec::new();
            let mut unit_cases: Vec<(syn::Ident, String, Option<i64>)> = Vec::new();
            let mut payload_cases: Vec<(syn::Ident, String, Fields)> = Vec::new();
            let mut any_skipped = false;
            for variant in &data.variants {
                // Explicit discriminants must be integer literals so the IR
                // can record them; the implicit chain follows Rust's rules.
                let explicit: Option<i64> = match &variant.discriminant {
                    Some((_, expr)) => match expr {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Int(lit),
                            ..
                        }) => match lit.base10_parse::<i64>() {
                            Ok(v) => Some(v),
                            Err(_) => {
                                errors.spanned(
                                    lit.span(),
                                    "script enum discriminants must fit in 64 bits (i64)",
                                );
                                None
                            }
                        },
                        syn::Expr::Unary(syn::ExprUnary {
                            op: syn::UnOp::Neg(_),
                            expr,
                            ..
                        }) => match expr.as_ref() {
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Int(lit),
                                ..
                            }) => lit.base10_parse::<i64>().ok().map(|v| -v),
                            _ => {
                                errors.spanned(
                                    expr.span(),
                                    "script enums support integer-literal discriminants only",
                                );
                                None
                            }
                        },
                        other => {
                            errors.spanned(
                                other.span(),
                                "script enums support integer-literal discriminants only",
                            );
                            None
                        }
                    },
                    None => None,
                };
                let discriminant: Option<i64> = if numeric {
                    let value = explicit.unwrap_or(next_discriminant);
                    next_discriminant = value.wrapping_add(1);
                    Some(value)
                } else {
                    if let Some(value) = explicit {
                        next_discriminant = value.wrapping_add(1);
                    } else {
                        next_discriminant = next_discriminant.wrapping_add(1);
                    }
                    None
                };
                let args = parse_variant_args(&variant.attrs, &mut errors);
                if args.skip.is_some() {
                    any_skipped = true;
                    continue;
                }
                // Case names cross backend boundaries: renames must stay
                // identifier-shaped (Rust conventions) so each backend can
                // derive its own optimal native spelling deterministically.
                if let Some(rename) = &args.rename {
                    let value = rename.value();
                    let mut chars = value.chars();
                    let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
                        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if !valid {
                        errors.spanned(
                            rename.span(),
                            "enum variant renames must follow Rust identifier conventions \
                             (ASCII letters, digits, and `_`, not starting with a digit)",
                        );
                    }
                }
                let vname = args
                    .rename
                    .as_ref()
                    .map(|r| r.value())
                    .unwrap_or_else(|| variant.ident.unraw().to_string());
                if matches!(variant.fields, Fields::Unit) {
                    unit_cases.push((variant.ident.clone(), vname.clone(), discriminant));
                } else {
                    payload_cases.push((
                        variant.ident.clone(),
                        vname.clone(),
                        variant.fields.clone(),
                    ));
                }
                let vdoc = doc_tokens(&extract_doc(&variant.attrs));
                if container.flags.is_some() && !matches!(variant.fields, Fields::Unit) {
                    errors.spanned(
                        variant.span(),
                        "`#[script(flags)]` enums must have only unit variants; \
                         bitflags cases cannot carry payloads",
                    );
                }
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
                let disc_expr = match discriminant {
                    Some(v) => quote! { ::core::option::Option::Some(#v) },
                    None => quote! { ::core::option::Option::None },
                };
                variant_exprs.push(quote! {
                    ::haphe::EnumVariant {
                        name: #vname,
                        doc: #vdoc,
                        kind: #kind,
                        discriminant: #disc_expr,
                    }
                });
            }
            // Non-flags, non-generic enums with no skipped variants get value
            // conversions when every payload field crosses the bridge: cases
            // cross as `ScriptValue::Enum` carrying the declared name
            // (matched exactly — backends translate native spellings at
            // their own boundary) plus the case's values (tuple positional;
            // struct fields in declaration order).
            let payload_bridgeable = payload_cases.iter().all(|(_, _, fields)| {
                fields.iter().all(|f| {
                    !matches!(&f.ty, Type::Reference(_)) && crate::bind::is_bridge_value_type(&f.ty)
                })
            });
            let case_conversions = if !any_skipped
                && !is_flags
                && !is_generic
                && payload_bridgeable
                && !(unit_cases.is_empty() && payload_cases.is_empty())
            {
                let vidents: Vec<&syn::Ident> = unit_cases.iter().map(|(i, _, _)| i).collect();
                let vdiscs: Vec<TokenStream> = unit_cases
                    .iter()
                    .map(|(_, _, d)| match d {
                        Some(v) => quote! { ::core::option::Option::Some(#v) },
                        None => quote! { ::core::option::Option::None },
                    })
                    .collect();
                let expected = ident_str.clone();
                // Unit-case From arms plus one arm per payload case
                // deconstructing its fields into the payload vector.
                let mut from_arms: Vec<TokenStream> = unit_cases
                    .iter()
                    .zip(&vdiscs)
                    .map(|((vi, name, _), disc)| {
                        quote! {
                            #ident::#vi => (#name, #disc, ::std::vec::Vec::new()),
                        }
                    })
                    .collect();
                // Case-matching FromScript arms.
                let mut into_arms: Vec<TokenStream> = unit_cases
                    .iter()
                    .map(|(vi, name, _)| {
                        quote! {
                            if __case == #name {
                                return if __payload.is_empty() {
                                    ::core::result::Result::Ok(#ident::#vi)
                                } else {
                                    ::core::result::Result::Err(::haphe::ScriptConvertError {
                                        expected: #expected,
                                        got: "payload on a unit case",
                                    })
                                };
                            }
                        }
                    })
                    .collect();
                for (vi, name, fields) in &payload_cases {
                    match fields {
                        Fields::Unnamed(unnamed) => {
                            let binds: Vec<syn::Ident> = (0..unnamed.unnamed.len())
                                .map(|i| quote::format_ident!("__f{i}"))
                                .collect();
                            let len = binds.len();
                            let tys: Vec<&Type> = unnamed.unnamed.iter().map(|f| &f.ty).collect();
                            from_arms.push(quote! {
                                #ident::#vi(#(#binds),*) => (
                                    #name,
                                    ::core::option::Option::None,
                                    <[_]>::into_vec(::std::boxed::Box::new([
                                        #(::haphe::IntoScript::into_script(#binds)),*
                                    ])),
                                ),
                            });
                            into_arms.push(quote! {
                                if __case == #name {
                                    if __payload.len() != #len {
                                        return ::core::result::Result::Err(
                                            ::haphe::ScriptConvertError {
                                                expected: #expected,
                                                got: "payload of mismatched length",
                                            },
                                        );
                                    }
                                    let mut __it = __payload.into_iter();
                                    return ::core::result::Result::Ok(#ident::#vi(
                                        #( <#tys as ::haphe::FromScript>::from_script(
                                            __it.next().expect("length checked")
                                        )? ),*
                                    ));
                                }
                            });
                        }
                        Fields::Named(named) => {
                            let fids: Vec<&syn::Ident> = named
                                .named
                                .iter()
                                .map(|f| f.ident.as_ref().expect("named field"))
                                .collect();
                            let len = fids.len();
                            let tys: Vec<&Type> = named.named.iter().map(|f| &f.ty).collect();
                            from_arms.push(quote! {
                                #ident::#vi { #(#fids),* } => (
                                    #name,
                                    ::core::option::Option::None,
                                    <[_]>::into_vec(::std::boxed::Box::new([
                                        #(::haphe::IntoScript::into_script(#fids)),*
                                    ])),
                                ),
                            });
                            into_arms.push(quote! {
                                if __case == #name {
                                    if __payload.len() != #len {
                                        return ::core::result::Result::Err(
                                            ::haphe::ScriptConvertError {
                                                expected: #expected,
                                                got: "payload of mismatched length",
                                            },
                                        );
                                    }
                                    let mut __it = __payload.into_iter();
                                    return ::core::result::Result::Ok(#ident::#vi {
                                        #( #fids: <#tys as ::haphe::FromScript>::from_script(
                                            __it.next().expect("length checked")
                                        )? ),*
                                    });
                                }
                            });
                        }
                        Fields::Unit => unreachable!("unit variants collect separately"),
                    }
                }
                // Numeric enums additionally accept their discriminant as
                // a plain integer (unit cases only — payload cases have no
                // discriminant).
                let numeric_arm = if numeric {
                    let discs: Vec<i64> =
                        unit_cases.iter().map(|(_, _, d)| d.unwrap_or(0)).collect();
                    quote! {
                        if let ::haphe::ScriptValue::I64(__n) = &value {
                            #(
                                if *__n == #discs {
                                    return ::core::result::Result::Ok(#ident::#vidents);
                                }
                            )*
                            return ::core::result::Result::Err(::haphe::ScriptConvertError {
                                expected: #expected,
                                got: "unknown enum discriminant",
                            });
                        }
                    }
                } else {
                    TokenStream::new()
                };
                quote! {
                    #[automatically_derived]
                    impl ::core::convert::From<#ident> for ::haphe::ScriptValue {
                        fn from(value: #ident) -> Self {
                            let (case, discriminant, payload) = match value {
                                #(#from_arms)*
                            };
                            ::haphe::ScriptValue::Enum {
                                case: ::std::string::String::from(case),
                                discriminant,
                                payload,
                            }
                        }
                    }
                    #[automatically_derived]
                    impl ::haphe::FromScript for #ident {
                        fn from_script(
                            value: ::haphe::ScriptValue,
                        ) -> ::core::result::Result<Self, ::haphe::ScriptConvertError> {
                            #numeric_arm
                            let (__case, __payload) = match value {
                                ::haphe::ScriptValue::Enum { case, payload, .. } => {
                                    (case, payload)
                                }
                                ::haphe::ScriptValue::String(s) => (s, ::std::vec::Vec::new()),
                                other => {
                                    return ::core::result::Result::Err(
                                        ::haphe::ScriptConvertError {
                                            expected: #expected,
                                            got: other.variant_name(),
                                        },
                                    );
                                }
                            };
                            #(#into_arms)*
                            ::core::result::Result::Err(::haphe::ScriptConvertError {
                                expected: #expected,
                                got: "unknown enum case",
                            })
                        }
                    }
                }
            } else {
                TokenStream::new()
            };
            let enum_asserts = container.methods.map(|span| {
                quote_spanned! {span=>
                    const _: () = ::core::assert!(
                        <#self_ty as ::haphe::ScriptImpl>::CONSTRUCTORS.is_empty(),
                        "enums cannot declare #[script(constructor)] functions"
                    );
                    const _: () = ::core::assert!(
                        <#self_ty as ::haphe::ScriptImpl>::PROPERTIES.is_empty(),
                        "enums cannot declare #[script(getter)]/#[script(setter)] properties"
                    );
                }
            });
            quote! {
                #enum_asserts
                #case_conversions
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
                        repr: #repr_expr,
                        is_flags: #is_flags,
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

    let bind_codegen = if let Data::Struct(data) = &input.data {
        let bind_fields: Vec<crate::bind::BindField> = match &data.fields {
            Fields::Named(named) => named
                .named
                .iter()
                .filter_map(|f| {
                    let args = parse_field_args(&f.attrs, &mut Errors::default());
                    if args.skip.is_some() {
                        return None;
                    }
                    let field_ident = f.ident.as_ref()?.clone();
                    let name = args
                        .rename
                        .as_ref()
                        .map(|r| r.value())
                        .unwrap_or_else(|| field_ident.unraw().to_string());
                    Some(crate::bind::BindField {
                        ident: field_ident,
                        name,
                        ty: f.ty.clone(),
                        readonly: args.readonly.is_some(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        crate::bind::gen_derive_bind(
            ident,
            &self_ty,
            &bind_fields,
            &container.traits,
            container.methods.is_some(),
            container.methods.is_some()
                && container.thread_safety.is_none()
                && !input.generics.params.is_empty(),
            &input.generics,
        )
    } else {
        crate::bind::gen_derive_bind(
            ident,
            &self_ty,
            &[],
            &container.traits,
            container.methods.is_some(),
            container.methods.is_some()
                && container.thread_safety.is_none()
                && !input.generics.params.is_empty(),
            &input.generics,
        )
    };

    // For generic types, `HapheType` references carry the concrete type
    // arguments so backends can monomorphize; the erased descriptor itself is
    // unchanged. Requires each type param to be `HapheType` (fresh clone of
    // the input generics — `desc_generics` carries claim predicates that
    // would over-constrain field-position uses).
    let haphe_type_impl = if is_generic {
        let mut ht_generics = input.generics.clone();
        let param_idents: Vec<_> = input
            .generics
            .type_params()
            .map(|p| p.ident.clone())
            .collect();
        for param in &param_idents {
            ht_generics
                .make_where_clause()
                .predicates
                .push(syn::parse_quote! { #param: ::haphe::HapheType });
        }
        let (ht_impl_g, ht_ty_g, ht_where_c) = ht_generics.split_for_impl();
        quote! {
            #[automatically_derived]
            impl #ht_impl_g ::haphe::HapheType for #ident #ht_ty_g #ht_where_c {
                const DESCRIPTOR: ::haphe::TypeDescriptor<'static> =
                    ::haphe::TypeDescriptor::Instance {
                        id: <Self as ::haphe::ScriptType>::ID,
                        args: &[#( <#param_idents as ::haphe::HapheType>::DESCRIPTOR ),*],
                    };
            }
        }
    } else {
        quote! {
            #[automatically_derived]
            impl #impl_g ::haphe::HapheType for #ident #ty_g #where_c {
                const DESCRIPTOR: ::haphe::TypeDescriptor<'static> =
                    ::haphe::TypeDescriptor::Ref(<Self as ::haphe::ScriptType>::ID);
            }
        }
    };

    Ok(quote! {
        #haphe_type_impl
        #[automatically_derived]
        impl #impl_g ::haphe::ScriptType for #ident #ty_g #where_c {
            const ID: ::haphe::TypeId<'static> = #id_expr;
        }
        #body
        #handshake
        #(#probes)*
        #bind_codegen
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

    if let Some(span) = container.flags {
        errors.spanned(span, "`#[script(flags)]` is only valid on enums");
    }
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

    // Transparent newtypes over value-convertible types cross the bridge as
    // the inner value itself, so runtimes see their native primitive — e.g.
    // a `bool` newtype participates in Lua truthiness. The conversions
    // delegate through the inner type's own `From`/`FromScript`, so chains
    // of transparent newtypes recurse down to the primitive; a transparent
    // newtype over a non-convertible inner is a compile error. Opaque
    // newtypes stay distinct named types with no generated conversions.
    let is_value_convertible = match &field.ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.arguments.is_none()),
        _ => false,
    };
    let conversions = if transparent && is_value_convertible && field_args.bytes.is_none() {
        let inner_ty = &field.ty;
        let (access, construct) = match &field.ident {
            Some(fid) => (quote! { __value.#fid }, quote! { Self { #fid: __inner } }),
            None => (quote! { __value.0 }, quote! { Self(__inner) }),
        };
        quote! {
            #[automatically_derived]
            impl ::core::convert::From<#ident> for ::haphe::ScriptValue {
                fn from(__value: #ident) -> Self {
                    ::haphe::ScriptValue::from(#access)
                }
            }
            #[automatically_derived]
            impl ::haphe::FromScript for #ident {
                fn from_script(
                    __v: ::haphe::ScriptValue,
                ) -> ::core::result::Result<Self, ::haphe::ScriptConvertError> {
                    let __inner = <#inner_ty as ::haphe::FromScript>::from_script(__v)?;
                    ::core::result::Result::Ok(#construct)
                }
            }
        }
    } else {
        TokenStream::new()
    };

    Ok(quote! {
        #conversions
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
