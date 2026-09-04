//! Shared parser for `#[script(...)]` arguments across all macro sites.
//!
//! Each site (container, field, variant, function, parameter) accepts its own
//! key set; everything funnels through [`parse_script_attrs`] so unknown keys
//! get uniform "did you mean" suggestions and errors are collected per item
//! instead of stopping at the first one.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Ident, LitStr, Type};

/// `thread_safety = ...` values.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThreadSafetyKind {
    None,
    Send,
    SendSync,
}

/// One entry inside `traits(...)`.
pub struct TraitDecl {
    pub name: Ident,
    /// Named type arguments, e.g. `rhs = f64`. Empty for marker traits.
    pub args: Vec<(Ident, Type)>,
}

/// Arguments accepted on the container (`struct` / `enum`).
#[derive(Default)]
pub struct ContainerArgs {
    pub rename: Option<LitStr>,
    pub thread_safety: Option<(ThreadSafetyKind, Span)>,
    pub traits: Vec<TraitDecl>,
    pub methods: Option<Span>,
    /// Newtypes only: expose as equivalent to the inner type.
    pub transparent: Option<Span>,
}

/// Arguments accepted on fields (and enum-variant fields).
#[derive(Default)]
pub struct FieldArgs {
    pub rename: Option<LitStr>,
    pub skip: Option<Span>,
    pub readonly: Option<Span>,
    pub bytes: Option<Span>,
}

/// Arguments accepted on enum variants.
#[derive(Default)]
pub struct VariantArgs {
    pub rename: Option<LitStr>,
    pub skip: Option<Span>,
}

/// Arguments accepted on functions (impl-block methods and free functions).
#[derive(Default)]
pub struct FnArgs {
    pub rename: Option<LitStr>,
    pub skip: Option<Span>,
    pub constructor: Option<Span>,
    pub getter: Option<Span>,
    /// `setter` flag or `setter = "property_name"` override.
    pub setter: Option<(Option<LitStr>, Span)>,
    pub error_kind: Option<LitStr>,
}

/// Arguments accepted on function parameters.
#[derive(Default)]
pub struct ParamArgs {
    pub clone: Option<Span>,
    pub bytes: Option<Span>,
}

/// Accumulates errors so every mistake on an item is reported at once.
#[derive(Default)]
pub struct Errors(Option<syn::Error>);

impl Errors {
    pub fn push(&mut self, err: syn::Error) {
        match &mut self.0 {
            Some(existing) => existing.combine(err),
            None => self.0 = Some(err),
        }
    }

    pub fn spanned(&mut self, span: Span, msg: impl std::fmt::Display) {
        self.push(syn::Error::new(span, msg));
    }

    /// Returns the collected error, if any.
    pub fn finish(self) -> Result<(), syn::Error> {
        match self.0 {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// Extracts the doc string from `///` comments, or `None` if there are none.
///
/// Strips the single space rustdoc conventionally puts after `///` but
/// preserves deeper indentation (code blocks and nested lists).
pub fn extract_doc(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(lit) = &nv.value
            && let syn::Lit::Str(s) = &lit.lit
        {
            let text = s.value();
            lines.push(text.strip_prefix(' ').unwrap_or(&text).to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        Some(lines.join("\n"))
    }
}

/// `Option<String>` → `Some("...")` / `None` tokens.
pub fn option_str_tokens(value: &Option<String>) -> TokenStream {
    match value {
        Some(text) => quote! { ::core::option::Option::Some(#text) },
        None => quote! { ::core::option::Option::None },
    }
}

/// Edit distance for "did you mean" suggestions.
fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn unknown_key_error(key: &Ident, allowed: &[&str], site: &str) -> syn::Error {
    let key_str = key.to_string();
    let suggestion = allowed
        .iter()
        .map(|k| (levenshtein(&key_str, k), k))
        .min()
        .filter(|(d, _)| *d <= 2)
        .map(|(_, k)| format!("; did you mean `{k}`?"))
        .unwrap_or_default();
    syn::Error::new(
        key.span(),
        format!(
            "unknown `#[script]` attribute `{key_str}` on {site} (expected one of: {}){suggestion}",
            allowed.join(", ")
        ),
    )
}

fn duplicate_key_error(key: &Ident) -> syn::Error {
    syn::Error::new(
        key.span(),
        format!("duplicate `#[script]` attribute `{key}`"),
    )
}

/// Parses all `#[script(...)]` attributes on an item, dispatching each key to
/// `handle`. `handle` returns `Ok(false)` for keys it doesn't recognize.
/// Returns parse errors for the caller to merge (keeps `handle` free to
/// capture the caller's error accumulator mutably).
fn parse_script_attrs(
    attrs: &[Attribute],
    mut handle: impl FnMut(&syn::meta::ParseNestedMeta<'_>) -> syn::Result<bool>,
    allowed: &[&str],
    site: &str,
) -> Vec<syn::Error> {
    let mut parse_errors = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("script") {
            continue;
        }
        let result = attr.parse_nested_meta(|meta| {
            let Some(key) = meta.path.get_ident().cloned() else {
                return Err(syn::Error::new(
                    meta.path.span(),
                    "expected a `#[script]` attribute name",
                ));
            };
            if handle(&meta)? {
                Ok(())
            } else {
                Err(unknown_key_error(&key, allowed, site))
            }
        });
        if let Err(err) = result {
            parse_errors.push(err);
        }
    }
    parse_errors
}

macro_rules! set_once {
    ($errors:expr, $slot:expr, $key:expr, $value:expr) => {
        if $slot.is_some() {
            $errors.push(duplicate_key_error($key));
        } else {
            $slot = Some($value);
        }
    };
}

pub fn parse_container_args(attrs: &[Attribute], errors: &mut Errors) -> ContainerArgs {
    const ALLOWED: &[&str] = &[
        "rename",
        "thread_safety",
        "traits",
        "methods",
        "transparent",
        "crate",
    ];
    let mut args = ContainerArgs::default();
    let parse_errors = parse_script_attrs(
        attrs,
        |meta| {
            let key = meta.path.get_ident().cloned().expect("checked by caller");
            if key == "rename" {
                let value: LitStr = meta.value()?.parse()?;
                set_once!(errors, args.rename, &key, value);
            } else if key == "thread_safety" {
                let value: Ident = meta.value()?.parse()?;
                let kind = match value.to_string().as_str() {
                    "none" => ThreadSafetyKind::None,
                    "send" => ThreadSafetyKind::Send,
                    "send_sync" => ThreadSafetyKind::SendSync,
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!(
                                "invalid thread_safety `{other}` (expected `none`, `send`, or `send_sync`)"
                            ),
                        ));
                    }
                };
                set_once!(errors, args.thread_safety, &key, (kind, value.span()));
            } else if key == "traits" {
                meta.parse_nested_meta(|tm| {
                    let Some(name) = tm.path.get_ident().cloned() else {
                        return Err(syn::Error::new(tm.path.span(), "expected a trait name"));
                    };
                    let mut trait_args = Vec::new();
                    if tm.input.peek(syn::token::Paren) {
                        tm.parse_nested_meta(|am| {
                            let Some(arg_name) = am.path.get_ident().cloned() else {
                                return Err(syn::Error::new(
                                    am.path.span(),
                                    "expected a named trait argument like `rhs = f64`",
                                ));
                            };
                            let ty: Type = am.value()?.parse()?;
                            trait_args.push((arg_name, ty));
                            Ok(())
                        })?;
                    }
                    if args.traits.iter().any(|t| t.name == name) {
                        return Err(syn::Error::new(
                            name.span(),
                            format!("trait `{name}` is declared more than once"),
                        ));
                    }
                    args.traits.push(TraitDecl {
                        name,
                        args: trait_args,
                    });
                    Ok(())
                })?;
            } else if key == "methods" {
                set_once!(errors, args.methods, &key, key.span());
            } else if key == "transparent" {
                set_once!(errors, args.transparent, &key, key.span());
            } else if key == "crate" {
                return Err(syn::Error::new(
                    key.span(),
                    "`#[script(crate = ...)]` is reserved and not yet supported",
                ));
            } else {
                return Ok(false);
            }
            Ok(true)
        },
        ALLOWED,
        "a type",
    );
    for err in parse_errors {
        errors.push(err);
    }
    args
}

pub fn parse_field_args(attrs: &[Attribute], errors: &mut Errors) -> FieldArgs {
    const ALLOWED: &[&str] = &["rename", "skip", "readonly", "bytes"];
    let mut args = FieldArgs::default();
    let parse_errors = parse_script_attrs(
        attrs,
        |meta| {
            let key = meta.path.get_ident().cloned().expect("checked by caller");
            if key == "rename" {
                let value: LitStr = meta.value()?.parse()?;
                set_once!(errors, args.rename, &key, value);
            } else if key == "skip" {
                set_once!(errors, args.skip, &key, key.span());
            } else if key == "readonly" {
                set_once!(errors, args.readonly, &key, key.span());
            } else if key == "bytes" {
                set_once!(errors, args.bytes, &key, key.span());
            } else {
                return Ok(false);
            }
            Ok(true)
        },
        ALLOWED,
        "a field",
    );
    for err in parse_errors {
        errors.push(err);
    }
    if args.skip.is_some() {
        let others = [
            (args.rename.as_ref().map(|r| r.span()), "rename"),
            (args.readonly, "readonly"),
            (args.bytes, "bytes"),
        ];
        for (span, name) in others {
            if let Some(span) = span {
                errors.spanned(span, format!("`{name}` has no effect on a `skip`ped field"));
            }
        }
    }
    args
}

pub fn parse_variant_args(attrs: &[Attribute], errors: &mut Errors) -> VariantArgs {
    const ALLOWED: &[&str] = &["rename", "skip"];
    let mut args = VariantArgs::default();
    let parse_errors = parse_script_attrs(
        attrs,
        |meta| {
            let key = meta.path.get_ident().cloned().expect("checked by caller");
            if key == "rename" {
                let value: LitStr = meta.value()?.parse()?;
                set_once!(errors, args.rename, &key, value);
            } else if key == "skip" {
                set_once!(errors, args.skip, &key, key.span());
            } else {
                return Ok(false);
            }
            Ok(true)
        },
        ALLOWED,
        "an enum variant",
    );
    for err in parse_errors {
        errors.push(err);
    }
    args
}

pub fn parse_fn_args(attrs: &[Attribute], errors: &mut Errors, site: &str) -> FnArgs {
    const ALLOWED: &[&str] = &[
        "rename",
        "skip",
        "constructor",
        "getter",
        "setter",
        "error_kind",
    ];
    let mut args = FnArgs::default();
    let parse_errors = parse_script_attrs(
        attrs,
        |meta| {
            let key = meta.path.get_ident().cloned().expect("checked by caller");
            if key == "rename" {
                let value: LitStr = meta.value()?.parse()?;
                set_once!(errors, args.rename, &key, value);
            } else if key == "skip" {
                set_once!(errors, args.skip, &key, key.span());
            } else if key == "constructor" {
                set_once!(errors, args.constructor, &key, key.span());
            } else if key == "getter" {
                set_once!(errors, args.getter, &key, key.span());
            } else if key == "setter" {
                let name = if meta.input.peek(syn::Token![=]) {
                    Some(meta.value()?.parse::<LitStr>()?)
                } else {
                    None
                };
                set_once!(errors, args.setter, &key, (name, key.span()));
            } else if key == "error_kind" {
                let value: LitStr = meta.value()?.parse()?;
                set_once!(errors, args.error_kind, &key, value);
            } else {
                return Ok(false);
            }
            Ok(true)
        },
        ALLOWED,
        site,
    );
    for err in parse_errors {
        errors.push(err);
    }
    let roles = [
        args.constructor.map(|s| ("constructor", s)),
        args.getter.map(|s| ("getter", s)),
        args.setter.as_ref().map(|(_, s)| ("setter", *s)),
    ];
    let mut set_roles = roles.iter().flatten();
    if let (Some(first), Some(second)) = (set_roles.next(), set_roles.next()) {
        errors.spanned(
            second.1,
            format!(
                "`{}` conflicts with `{}` — a function has one role",
                second.0, first.0
            ),
        );
    }
    args
}

pub fn parse_param_args(attrs: &[Attribute], errors: &mut Errors) -> ParamArgs {
    const ALLOWED: &[&str] = &["clone", "bytes"];
    let mut args = ParamArgs::default();
    let parse_errors = parse_script_attrs(
        attrs,
        |meta| {
            let key = meta.path.get_ident().cloned().expect("checked by caller");
            if key == "clone" {
                set_once!(errors, args.clone, &key, key.span());
            } else if key == "bytes" {
                set_once!(errors, args.bytes, &key, key.span());
            } else {
                return Ok(false);
            }
            Ok(true)
        },
        ALLOWED,
        "a parameter",
    );
    for err in parse_errors {
        errors.push(err);
    }
    args
}

/// Removes `#[script(...)]` attributes from an attribute list (used when
/// re-emitting user code that carried our markers).
pub fn strip_script_attrs(attrs: &mut Vec<Attribute>) {
    attrs.retain(|attr| !attr.path().is_ident("script"));
}
