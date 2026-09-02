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
}

struct ModuleInput {
    name: Ident,
    doc: Option<LitStr>,
    functions: Option<Vec<Path>>,
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
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown registry section `{other}` (expected: structs, enums, \
                             type_aliases, modules)"
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

fn module_expr(module: &ModuleInput) -> TokenStream {
    let name = module.name.unraw().to_string();
    let doc = match &module.doc {
        Some(text) => quote! { ::core::option::Option::Some(#text) },
        None => quote! { ::core::option::Option::None },
    };
    // Functions resolve through the `ScriptFunction` trait on the hidden type
    // `#[script]` emits next to each free fn; the type shares the fn's name,
    // so imports and re-exports work and unannotated fns get a guided error.
    let functions: Vec<_> = module
        .functions
        .iter()
        .flatten()
        .map(|path: &Path| {
            quote_spanned! {path.span()=> <#path as ::haphe::ScriptFunction>::DESCRIPTOR }
        })
        .collect();
    let type_ids: Vec<_> = module
        .types
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptType>::ID })
        .collect();
    let submodules: Vec<_> = module.modules.iter().flatten().map(module_expr).collect();
    let constants: Vec<_> = module
        .constants
        .iter()
        .flatten()
        .map(|c| {
            let name = c.name.unraw().to_string();
            let doc = option_str_tokens(&c.doc);
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
                    (lit.to_token_stream().to_string(), ty_check)
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
        }
    }
}

pub fn expand(input: RegistryInput) -> TokenStream {
    let RegistryInput {
        attrs,
        vis,
        name,
        structs,
        enums,
        type_aliases,
        modules,
    } = &input;
    let struct_descs: Vec<_> = structs
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptStruct>::DESCRIPTOR })
        .collect();
    let enum_descs: Vec<_> = enums
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptEnum>::DESCRIPTOR })
        .collect();
    let alias_descs: Vec<_> = type_aliases
        .iter()
        .flatten()
        .map(|ty| quote_spanned! {ty.span()=> <#ty as ::haphe::ScriptAlias>::DESCRIPTOR })
        .collect();
    let module_descs: Vec<_> = modules.iter().flatten().map(module_expr).collect();
    quote! {
        #(#attrs)*
        #vis static #name: ::haphe::TypeRegistry<'static> = ::haphe::TypeRegistry::new(
            &[#(#struct_descs),*],
            &[#(#enum_descs),*],
            &[#(#alias_descs),*],
            &[#(#module_descs),*],
        );
    }
}
