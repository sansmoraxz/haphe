//! Builds `FunctionDescriptor` literals from function signatures. Shared by
//! the impl-block and free-function paths of `#[script]`.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{FnArg, Pat, ReceiverKind, ReturnType, Safety, Signature};

use crate::attrs::{
    Errors, FnArgs, extract_doc, option_str_tokens, parse_param_args, strip_script_attrs,
};
use crate::ty_map::{TyCtx, descriptor_expr, ownership_expr, substitute_self};
use crate::verify;

/// The receiver shape of an exposed function.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReceiverShape {
    None,
    Ref,
    RefMut,
    Owned,
}

impl ReceiverShape {
    pub fn tokens(self) -> TokenStream {
        match self {
            Self::None => quote! { ::core::option::Option::None },
            Self::Ref => quote! { ::core::option::Option::Some(::haphe::Receiver::Ref) },
            Self::RefMut => quote! { ::core::option::Option::Some(::haphe::Receiver::RefMut) },
            Self::Owned => quote! { ::core::option::Option::Some(::haphe::Receiver::Owned) },
        }
    }
}

/// Everything extracted from one function signature.
pub struct FnInfo {
    /// `FunctionDescriptor { ... }` literal.
    pub descriptor: TokenStream,
    pub receiver: ReceiverShape,
    pub is_async: bool,
    /// Number of parameters (excluding the receiver).
    pub param_count: usize,
    /// Return type after `Self` substitution, `None` for `()`.
    pub return_ty: Option<syn::Type>,
    /// The single parameter's type after `Self` substitution, when there is
    /// exactly one parameter.
    pub single_param_ty: Option<syn::Type>,
    /// Exposed name (rename or the Rust identifier).
    pub name: String,
    /// Doc string, if any.
    pub doc: Option<String>,
}

/// Removes `#[script(...)]` attributes from every parameter of a signature.
/// Used on functions that are skipped or hit an error before descriptor
/// construction, so leftover helper attributes don't re-expand.
pub fn strip_param_script_attrs(sig: &mut Signature) {
    for input in &mut sig.inputs {
        match input {
            FnArg::Typed(pat_ty) => strip_script_attrs(&mut pat_ty.attrs),
            FnArg::Receiver(recv) => strip_script_attrs(&mut recv.attrs),
        }
    }
}

/// Builds a descriptor from a signature, stripping parameter-level
/// `#[script(...)]` attrs as it goes. Returns `None` (with errors recorded)
/// for unsupported shapes — parameter attrs are stripped even then.
pub fn build_fn_info(
    sig: &mut Signature,
    fn_args: &FnArgs,
    attrs: &[syn::Attribute],
    ctx: &TyCtx,
    errors: &mut Errors,
) -> Option<FnInfo> {
    // Parse and strip parameter attrs first: every early return below must
    // leave the re-emitted function free of helper attributes.
    let mut param_args = Vec::new();
    for input in &mut sig.inputs {
        if let FnArg::Typed(pat_ty) = input {
            param_args.push(parse_param_args(&pat_ty.attrs, errors));
        }
    }
    strip_param_script_attrs(sig);

    let mut fatal = false;
    for param in &sig.generics.params {
        // Lifetimes are fine — descriptors erase them.
        if matches!(
            param,
            syn::GenericParam::Type(_) | syn::GenericParam::Const(_)
        ) {
            errors.spanned(
                param.span(),
                "generic functions cannot be exposed to scripts",
            );
            fatal = true;
        }
    }
    if let Safety::Unsafe(token) = &sig.safety {
        errors.spanned(
            token.span(),
            "unsafe functions cannot be exposed to scripts",
        );
        fatal = true;
    }
    if let Some(abi) = &sig.abi {
        errors.spanned(abi.span(), "extern functions cannot be exposed to scripts");
        fatal = true;
    }
    if fatal {
        return None;
    }

    let mut receiver = ReceiverShape::None;
    let mut param_exprs = Vec::new();
    let mut param_tys = Vec::new();
    let mut param_args = param_args.into_iter();

    for input in &sig.inputs {
        match input {
            FnArg::Receiver(recv) => {
                receiver = match &recv.kind {
                    ReceiverKind::Value => ReceiverShape::Owned,
                    ReceiverKind::Reference(_, _, Some(_)) => ReceiverShape::RefMut,
                    ReceiverKind::Reference(_, _, None) => ReceiverShape::Ref,
                    ReceiverKind::Typed(..) => {
                        errors.spanned(
                            recv.span(),
                            "explicitly typed receivers (`self: Box<Self>`, ...) are not supported",
                        );
                        return None;
                    }
                    _ => {
                        errors.spanned(recv.span(), "unsupported receiver shape");
                        return None;
                    }
                };
            }
            FnArg::Typed(pat_ty) => {
                let args = param_args.next().unwrap_or_default();
                let name = match pat_ty.pat.as_ref() {
                    Pat::Ident(pi) => pi.ident.unraw().to_string(),
                    other => {
                        errors.spanned(
                            other.span(),
                            "exposed function parameters must have simple names",
                        );
                        continue;
                    }
                };
                if let Some(span) = args.clone
                    && matches!(pat_ty.ty.as_ref(), syn::Type::Reference(r) if r.mutability.is_some())
                {
                    errors.spanned(
                        span,
                        "`clone` on a `&mut` parameter would discard mutations; \
                         take `&T` or an owned value instead",
                    );
                    continue;
                }
                let ty_expr = match args.bytes {
                    Some(span) => {
                        let probe = verify::bytes_probe(&pat_ty.ty, span);
                        quote_spanned! {span=> { #probe ::haphe::TypeDescriptor::Bytes } }
                    }
                    None => match descriptor_expr(&pat_ty.ty, ctx) {
                        Ok(expr) => expr,
                        Err(err) => {
                            errors.push(err);
                            continue;
                        }
                    },
                };
                let ownership = ownership_expr(&pat_ty.ty, args.clone.is_some());
                param_tys.push(substitute_self(&pat_ty.ty, ctx));
                param_exprs.push(quote! {
                    ::haphe::ParamDescriptor {
                        name: #name,
                        ty: &#ty_expr,
                        ownership: #ownership,
                    }
                });
            }
        }
    }

    let (return_expr, return_ownership, return_ty) = match &sig.output {
        ReturnType::Default => (
            quote! { &::haphe::TypeDescriptor::Unit },
            quote! { ::haphe::Ownership::Owned },
            None,
        ),
        ReturnType::Type(_, ty) => {
            let expr = match descriptor_expr(ty, ctx) {
                Ok(expr) => quote! { &#expr },
                Err(err) => {
                    errors.push(err);
                    quote! { &::haphe::TypeDescriptor::Unit }
                }
            };
            (
                expr,
                ownership_expr(ty, false),
                Some(substitute_self(ty, ctx)),
            )
        }
    };

    let name = fn_args
        .rename
        .as_ref()
        .map(|r| r.value())
        .unwrap_or_else(|| sig.ident.unraw().to_string());
    let doc = extract_doc(attrs);
    let doc_tokens = option_str_tokens(&doc);
    let error_kind = match &fn_args.error_kind {
        Some(kind) => quote! { ::core::option::Option::Some(#kind) },
        None => quote! { ::core::option::Option::None },
    };
    let is_async = sig.asyncness.is_some();
    let receiver_tokens = receiver.tokens();

    let descriptor = quote! {
        ::haphe::FunctionDescriptor {
            name: #name,
            doc: #doc_tokens,
            receiver: #receiver_tokens,
            params: &[#(#param_exprs),*],
            return_type: #return_expr,
            return_ownership: #return_ownership,
            is_async: #is_async,
            error_kind: #error_kind,
        }
    };

    let param_count = param_tys.len();
    let single_param_ty = if param_count == 1 {
        param_tys.pop()
    } else {
        None
    };
    Some(FnInfo {
        descriptor,
        receiver,
        is_async,
        param_count,
        return_ty,
        single_param_ty,
        name,
        doc,
    })
}
