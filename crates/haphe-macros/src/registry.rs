//! Expansion of `registry! { ... }`: assembles a `static TypeRegistry` from
//! derived types and `#[script]` functions.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Ident, Lit, LitStr, Path, Token, Type, Visibility, braced, bracketed};

use crate::attrs::{extract_doc, option_str_tokens};

pub struct RegistryInput {
    attrs: Vec<Attribute>,
    vis: Visibility,
    name: Ident,
    structs: Option<Vec<Type>>,
    enums: Option<Vec<Type>>,
    type_aliases: Option<Vec<Type>>,
    modules: Option<Vec<ModuleInput>>,
    foreign: Option<Vec<Type>>,
}

struct ModuleInput {
    name: Ident,
    doc: Option<LitStr>,
    functions: Option<Vec<FnEntry>>,
    types: Option<Vec<Type>>,
    constants: Option<Vec<ConstantInput>>,
    modules: Option<Vec<ModuleInput>>,
}

struct ConstantInput {
    doc: Option<String>,
    name: Ident,
    ty: Type,
    value: Lit,
}

fn bracketed_list<T: Parse>(input: ParseStream) -> syn::Result<Vec<T>> {
    let content;
    bracketed!(content in input);
    Ok(Punctuated::<T, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect())
}

fn duplicate_section(key: &Ident) -> syn::Error {
    syn::Error::new(key.span(), format!("duplicate `{key}` section"))
}

/// One entry of a module's `functions: [...]` list: a path to a `#[script]`
/// fn, or an inline closure (`double: |x: i64| -> i64 { x * 2 }`) the macro
/// expands into an equivalent free fn at the registry's scope.
enum FnEntry {
    Named(Path),
    Closure(ClosureInput),
}

struct ClosureInput {
    attrs: Vec<Attribute>,
    name: Ident,
    closure: syn::ExprClosure,
}

impl Parse for FnEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        // A lone ident followed by a single `:` names a closure; `::` is a
        // path separator and stays on the named branch.
        if input.peek(Ident) && input.peek2(Token![:]) && !input.peek2(Token![::]) {
            let name: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let closure: syn::ExprClosure = input.parse()?;
            if let Some(kw) = &closure.capture {
                return Err(syn::Error::new(
                    kw.span(),
                    "`move` has no meaning here: registry closures cannot capture — \
                     state belongs in a described type",
                ));
            }
            if let Some(lts) = &closure.lifetimes {
                return Err(syn::Error::new(
                    lts.span(),
                    "registry closures do not take `for<...>` binders",
                ));
            }
            if let Some(kw) = &closure.constness {
                return Err(syn::Error::new(
                    kw.span(),
                    "`const` closures cannot become registry functions",
                ));
            }
            for pat in &closure.inputs {
                if !matches!(pat, syn::Pat::Type(_)) {
                    return Err(syn::Error::new(
                        pat.span(),
                        "registry closure parameters need explicit types, e.g. `|x: i64|` \
                         (descriptors cannot infer them)",
                    ));
                }
            }
            return Ok(Self::Closure(ClosureInput {
                attrs,
                name,
                closure,
            }));
        }
        if let Some(attr) = attrs.first() {
            return Err(syn::Error::new(
                attr.span(),
                "attributes go on the function's own declaration; only closure entries \
                 (`name: |...| ...`) take them here",
            ));
        }
        Ok(Self::Named(input.parse()?))
    }
}

/// The free fn a closure entry expands to: the closure's annotated signature
/// and body as a plain `fn` named by the entry, carrying the entry's
/// attributes (docs, `#[script(...)]` options). A body referencing outer
/// locals fails with rustc's own can't-capture error at the offending
/// identifier.
fn closure_item_fn(closure: &ClosureInput) -> syn::ItemFn {
    let mut inputs: Punctuated<syn::FnArg, Token![,]> = Punctuated::new();
    for pat in &closure.closure.inputs {
        if let syn::Pat::Type(pt) = pat {
            inputs.push(syn::FnArg::Typed(pt.clone()));
        }
    }
    let body: syn::Block = match closure.closure.body.as_ref() {
        syn::Expr::Block(b) if b.attrs.is_empty() && b.label.is_none() => b.block.clone(),
        expr => syn::parse_quote!({ #expr }),
    };
    syn::ItemFn {
        attrs: closure.attrs.clone(),
        // Items sit inside the generated mirror module; the module carries
        // the registry's visibility, the items are pub through it.
        vis: syn::parse_quote!(pub),
        modifiers: syn::FnModifiers::default(),
        sig: syn::Signature {
            constness: None,
            asyncness: closure.closure.asyncness,
            safety: syn::Safety::Default,
            abi: None,
            fn_token: syn::token::Fn(closure.name.span()),
            ident: closure.name.clone(),
            generics: syn::Generics::default(),
            paren_token: syn::token::Paren::default(),
            inputs,
            variadic: None,
            output: closure.closure.output.clone(),
        },
        block: Box::new(body),
    }
}

impl Parse for ConstantInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let doc = extract_doc(&attrs);
        if let Some(unknown) = attrs.iter().find(|a| !a.path().is_ident("doc")) {
            return Err(syn::Error::new(
                unknown.span(),
                "constants accept only doc comments here",
            ));
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty: Type = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Lit = input.parse()?;
        Ok(Self {
            doc,
            name,
            ty,
            value,
        })
    }
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Token![mod]>()?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut module = ModuleInput {
            name,
            doc: None,
            functions: None,
            types: None,
            constants: None,
            modules: None,
        };
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "doc" => {
                    if module.doc.replace(content.parse()?).is_some() {
                        return Err(duplicate_section(&key));
                    }
                }
                "functions" => {
                    if module
                        .functions
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                "types" => {
                    if module.types.replace(bracketed_list(&content)?).is_some() {
                        return Err(duplicate_section(&key));
                    }
                }
                "constants" => {
                    if module
                        .constants
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                "modules" => {
                    if module.modules.replace(bracketed_list(&content)?).is_some() {
                        return Err(duplicate_section(&key));
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown module section `{other}` (expected: doc, functions, \
                             types, constants, modules)"
                        ),
                    ));
                }
            }
            if !content.is_empty() {
                content.parse::<Token![,]>()?;
            }
        }
        Ok(module)
    }
}

impl Parse for RegistryInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis: Visibility = input.parse()?;
        input.parse::<Token![static]>()?;
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let content;
        braced!(content in input);
        let mut registry = RegistryInput {
            attrs,
            vis,
            name,
            structs: None,
            enums: None,
            type_aliases: None,
            modules: None,
            foreign: None,
        };
        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            match key.to_string().as_str() {
                "structs" => {
                    if registry
                        .structs
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                "enums" => {
                    if registry.enums.replace(bracketed_list(&content)?).is_some() {
                        return Err(duplicate_section(&key));
                    }
                }
                "type_aliases" => {
                    if registry
                        .type_aliases
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                "modules" => {
                    if registry
                        .modules
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                "foreign" => {
                    if registry
                        .foreign
                        .replace(bracketed_list(&content)?)
                        .is_some()
                    {
                        return Err(duplicate_section(&key));
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown registry section `{other}` (expected: structs, enums, \
                             type_aliases, modules, foreign)"
                        ),
                    ));
                }
            }
            if !content.is_empty() {
                content.parse::<Token![,]>()?;
            }
        }
        // Optional trailing semicolon after the closing brace.
        if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
        }
        Ok(registry)
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
/// Builds one module's descriptor expression, and — when the module (or a
/// submodule) declares closures — a mirroring Rust `mod` item holding their
/// expanded free fns, pushed to `closure_mods`. Each generated module opens
/// with a hidden `pub use super::*;` so closure bodies resolve names from
/// the registry's own scope.
fn module_expr(
    module: &ModuleInput,
    vis: &Visibility,
    mod_path: &[Ident],
    closure_mods: &mut Vec<TokenStream>,
) -> TokenStream {
    let name = module.name.unraw().to_string();
    let doc = if let Some(text) = &module.doc {
        quote! { ::core::option::Option::Some(#text) }
    } else {
        quote! { ::core::option::Option::None }
    };
    // Functions resolve through the `ScriptFunction` trait on the hidden type
    // `#[script]` emits next to each free fn; the type shares the fn's name,
    // so imports and re-exports work and unannotated fns get a guided error.
    // A mention with type arguments (`echo<i64>`) additionally records a
    // module-level instantiation; the erased descriptor dedupes by stripped
    // path across mentions.
    let mut seen_fn_paths = std::collections::HashSet::new();
    let mut seen_fn_insts = std::collections::HashSet::new();
    let mut functions = Vec::new();
    let mut fn_instantiations = Vec::new();
    let own_path: Vec<Ident> = mod_path
        .iter()
        .cloned()
        .chain([module.name.clone()])
        .collect();
    let mut own_closures = Vec::new();
    let mut seen_closures = std::collections::HashSet::new();
    for entry in module.functions.iter().flatten() {
        let path = match entry {
            FnEntry::Named(path) => path,
            FnEntry::Closure(closure) => {
                let name = &closure.name;
                if !seen_closures.insert(name.unraw().to_string()) {
                    functions.push(
                        syn::Error::new(
                            name.span(),
                            format!("closure `{name}` is declared twice in this module"),
                        )
                        .to_compile_error(),
                    );
                    continue;
                }
                own_closures.push(crate::freefn::expand(closure_item_fn(closure)));
                functions.push(quote_spanned! {name.span()=>
                    <#(#own_path::)*#name as ::haphe::ScriptFunction>::DESCRIPTOR
                });
                continue;
            }
        };
        let mut stripped: Path = path.clone();
        let args = stripped
            .segments
            .last_mut()
            .map_or(syn::PathArguments::None, |last| {
                std::mem::replace(&mut last.arguments, syn::PathArguments::None)
            });
        if seen_fn_paths.insert(stripped.to_token_stream().to_string()) {
            functions.push(quote_spanned! {path.span()=>
                <#stripped as ::haphe::ScriptFunction>::DESCRIPTOR
            });
        }
        let syn::PathArguments::AngleBracketed(bracketed) = &args else {
            continue;
        };
        if !seen_fn_insts.insert(path.to_token_stream().to_string()) {
            continue;
        }
        let mut arg_tys = Vec::new();
        let mut bad_arg = None;
        for arg in &bracketed.args {
            match arg {
                syn::GenericArgument::Type(ty) => arg_tys.push(ty),
                other => {
                    bad_arg = Some(syn::Error::new(
                        other.span(),
                        "function instantiations take type arguments only",
                    ));
                    break;
                }
            }
        }
        if let Some(err) = bad_arg {
            fn_instantiations.push(err.to_compile_error());
            continue;
        }
        let arity = arg_tys.len();
        let fn_name = stripped
            .segments
            .last()
            .map(|s| s.ident.unraw().to_string())
            .unwrap_or_default();
        let non_generic_msg =
            format!("function `{fn_name}` is not generic but is given type arguments");
        let arity_msg = format!(
            "wrong number of type arguments for function `{fn_name}` (does not match its declared generic parameters)"
        );
        fn_instantiations.push(quote_spanned! {path.span()=>
            {
                const _: () = {
                    const __PARAMS: usize =
                        <#stripped as ::haphe::ScriptFunction>::DESCRIPTOR.generic_params.len();
                    ::core::assert!(__PARAMS != 0, #non_generic_msg);
                    ::core::assert!(__PARAMS == #arity, #arity_msg);
                };
                ::haphe::FnInstantiation {
                    function: <#stripped as ::haphe::ScriptFunction>::DESCRIPTOR.name,
                    args: &[#(<#arg_tys as ::haphe::HapheType>::DESCRIPTOR),*],
                }
            }
        });
    }
    let type_ids: Vec<_> = module
        .types
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptType>::ID })
        .collect();
    let mut child_mods = Vec::new();
    let submodules: Vec<_> = module
        .modules
        .iter()
        .flatten()
        .map(|m| module_expr(m, vis, &own_path, &mut child_mods))
        .collect();
    if !own_closures.is_empty() || !child_mods.is_empty() {
        let mod_name = &module.name;
        closure_mods.push(quote! {
            #vis mod #mod_name {
                #[doc(hidden)]
                pub use super::*;
                #(#own_closures)*
                #(#child_mods)*
            }
        });
    }
    let constants: Vec<_> = module
        .constants
        .iter()
        .flatten()
        .map(|c| {
            let name = c.name.unraw().to_string();
            let doc = option_str_tokens(c.doc.as_deref());
            let ty = &c.ty;
            let (value, check) = match &c.value {
                // A string constant's value is the string itself (the declared
                // type must describe a script string); other literals are
                // stringified and type-checked against the declared type.
                Lit::Str(lit) => {
                    let name = c.name.unraw().to_string();
                    let message = format!(
                        "constant `{name}` has a string value but its declared type is not a string type"
                    );
                    let ty = &c.ty;
                    let desc_check = quote_spanned! {lit.span()=>
                        {
                            const _: () = ::core::assert!(
                                <#ty as ::haphe::HapheType>::DESCRIPTOR
                                    .const_eq(&::haphe::TypeDescriptor::String),
                                #message
                            );
                        }
                    };
                    (lit.value(), desc_check)
                }
                lit => {
                    let ty_check = quote_spanned! {lit.span()=>
                        { const _: #ty = #lit; }
                    };
                    // Canonical value, not source spelling: `1_000.5f64`
                    // carries as "1000.5" — backends `str::parse` it, and
                    // separators/suffixes would break the parse.
                    let rendered = match lit {
                        Lit::Int(i) => i.base10_digits().to_string(),
                        Lit::Float(f) => f.base10_digits().to_string(),
                        Lit::Bool(b) => b.value.to_string(),
                        Lit::Char(c) => c.value().to_string(),
                        other => other.to_token_stream().to_string(),
                    };
                    (rendered, ty_check)
                }
            };
            quote_spanned! {c.ty.span()=>
                {
                    #check
                    ::haphe::ConstantDescriptor {
                        name: #name,
                        doc: #doc,
                        ty: &<#ty as ::haphe::HapheType>::DESCRIPTOR,
                        value: #value,
                    }
                }
            }
        })
        .collect();
    quote! {
        ::haphe::ModuleDescriptor {
            name: #name,
            doc: #doc,
            functions: &[#(#functions),*],
            type_ids: &[#(#type_ids),*],
            submodules: &[#(#submodules),*],
            constants: &[#(#constants),*],
            function_instantiations: &[#(#fn_instantiations),*],
        }
    }
}

/// The type's path with generic arguments stripped from the last segment,
/// used to dedupe erased descriptors across instantiations.
fn stripped_path_key(ty: &Type) -> String {
    if let Type::Path(p) = ty {
        let mut stripped = p.clone();
        if let Some(last) = stripped.path.segments.last_mut() {
            last.arguments = syn::PathArguments::None;
        }
        stripped.to_token_stream().to_string()
    } else {
        ty.to_token_stream().to_string()
    }
}

/// Type arguments of the last path segment, if any — marks a generic
/// instantiation entry.
fn type_args(ty: &Type) -> Vec<&Type> {
    let Type::Path(p) = ty else { return Vec::new() };
    let Some(last) = p.path.segments.last() else {
        return Vec::new();
    };
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return Vec::new();
    };
    args.args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// Erased descriptors (deduped by stripped path) plus one instantiation
/// record per entry that carries type arguments.
fn descs_and_instantiations(
    types: Option<&[Type]>,
    script_trait: &TokenStream,
    id_expr: impl Fn(&Type) -> TokenStream,
    instantiations: &mut Vec<TokenStream>,
) -> Vec<TokenStream> {
    let mut seen = std::collections::HashSet::new();
    let mut descs = Vec::new();
    for ty in types.into_iter().flatten() {
        if seen.insert(stripped_path_key(ty)) {
            descs.push(quote_spanned! {ty.span()=> <#ty as ::haphe::#script_trait>::DESCRIPTOR });
        }
        let args = type_args(ty);
        if !args.is_empty() {
            let id = id_expr(ty);
            instantiations.push(quote_spanned! {ty.span()=>
                ::haphe::InstantiationDescriptor {
                    id: #id,
                    args: &[#( <#args as ::haphe::HapheType>::DESCRIPTOR ),*],
                }
            });
        }
    }
    descs
}

#[allow(
    clippy::too_many_lines,
    reason = "expansion drivers assemble one `quote!` output from many interdependent pieces; splitting them hurts locality more than length hurts readability"
)]
pub fn expand(input: &RegistryInput) -> TokenStream {
    let RegistryInput {
        attrs,
        vis,
        name,
        structs,
        enums,
        type_aliases,
        modules,
        foreign,
    } = &input;
    let mut instantiations = Vec::new();
    let script_type_id = |ty: &Type| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptType>::ID };
    let struct_descs = descs_and_instantiations(
        structs.as_deref(),
        &quote!(ScriptStruct),
        script_type_id,
        &mut instantiations,
    );
    let enum_descs = descs_and_instantiations(
        enums.as_deref(),
        &quote!(ScriptEnum),
        script_type_id,
        &mut instantiations,
    );
    let alias_descs: Vec<_> = type_aliases
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptAlias>::DESCRIPTOR })
        .collect();
    let mut closure_mods = Vec::new();
    let module_descs: Vec<_> = modules
        .iter()
        .flatten()
        .map(|m| module_expr(m, vis, &[], &mut closure_mods))
        .collect();
    let foreign_descs = descs_and_instantiations(
        foreign.as_deref(),
        &quote!(ScriptForeign),
        |ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptForeign>::DESCRIPTOR.id },
        &mut instantiations,
    );
    quote! {
        #(#closure_mods)*

        #(#attrs)*
        #vis static #name: ::haphe::TypeRegistry<'static> = ::haphe::TypeRegistry::new(
            &[#(#struct_descs),*],
            &[#(#enum_descs),*],
            &[#(#alias_descs),*],
            &[#(#module_descs),*],
            &[#(#instantiations),*],
            &[#(#foreign_descs),*],
        );
    }
}
