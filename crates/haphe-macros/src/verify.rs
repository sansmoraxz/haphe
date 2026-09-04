//! Emission of compile-time verification probes.

use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use syn::Type;

/// Probe asserting a declared `thread_safety = send`/`send_sync` claim.
pub fn thread_safety_probe(self_ty: &Type, span: Span, sync: bool) -> TokenStream {
    if sync {
        quote_spanned! {span=>
            const _: () = {
                const fn __c<T: ::haphe::__verify::DeclaredSendSync + ?Sized>() {}
                __c::<#self_ty>()
            };
        }
    } else {
        quote_spanned! {span=>
            const _: () = {
                const fn __c<T: ::haphe::__verify::DeclaredSend + ?Sized>() {}
                __c::<#self_ty>()
            };
        }
    }
}

/// Probe asserting `#[script(bytes)]` is applied to a byte-slice-shaped type.
/// Emitted inline inside a block expression, so no trailing `const` item.
pub fn bytes_probe(ty: &Type, span: Span) -> TokenStream {
    quote_spanned! {span=>
        ::haphe::__verify::assert_bytes_like::<#ty>();
    }
}
