//! Procedural macros for haphe. Use through the `haphe` crate.

use proc_macro::TokenStream;

mod bind;
mod derive;
mod foreign;
mod freefn;
mod imp;
mod registry;
mod verify;

/// Derive macro generating scripting descriptors for a struct or enum.
///
/// The derived descriptor is a compile-time constant, available through the
/// `ScriptStruct` or `ScriptEnum` trait.
///
/// # Container attributes
///
/// `#[script(...)]`, placed after the derive:
///
/// - `rename = "Name"` — the exposed name. The type id keeps the Rust path.
/// - `thread_safety = none | send | send_sync` — thread-safety claim,
///   verified at compile time. Defaults to `none`; required for types with
///   `async` methods.
///
/// On generic types, `thread_safety` and `traits(...)` claims become bounds
/// on the descriptor and are verified at each exposed instantiation. In
/// generic field positions, bare container names (`Vec<T>`, `Option<T>`, ...)
/// are assumed to be the standard-library types.
///
/// # Newtypes
///
/// A single-field tuple struct derives as a type alias
/// (`TypeAliasDescriptor`), listed under `type_aliases:` in [`registry!`].
/// With `#[script(transparent)]` it is described as its inner type wherever
/// it appears; without, it is a distinct named type. A single-field *named*
/// struct marked `transparent` is also an alias (unmarked, it stays an
/// ordinary struct). Aliases take no `traits(...)`, `thread_safety`, or
/// `methods`.
///
/// ```
/// use haphe::{HapheType, Script, TypeDescriptor};
///
/// /// A distance in meters.
/// #[derive(Script)]
/// #[script(transparent)]
/// struct Meters(f64);
///
/// assert_eq!(<Meters as HapheType>::DESCRIPTOR, <f64 as HapheType>::DESCRIPTOR);
/// ```
/// - `traits(...)` — standard traits to expose, verified at compile time:
///   `Display`, `Debug`, `Hash`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`,
///   `Clone`, `Default`, `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `Index`,
///   `IndexMut`, `Iterator`, `IntoIterator`. Operator traits accept named
///   type arguments (e.g. `Add(rhs = f64, output = Self)`), defaulting to
///   `Self`; `Index`/`IndexMut` require `index` and `output`;
///   `Iterator`/`IntoIterator` require `item`.
/// - `methods` — include the type's `#[script] impl` block.
///
/// # Field attributes
///
/// - `rename = "name"` — the exposed field name.
/// - `skip` — hide the field.
/// - `readonly` — expose the field without script-side writes.
/// - `bytes` — describe a byte-slice-shaped field (`Vec<u8>`, `&[u8]`,
///   `[u8; N]`, `Box<[u8]>`) as a byte string instead of a list of integers.
///
/// # Example
///
/// ```
/// use haphe::{Script, ScriptStruct};
///
/// /// A 2D point.
/// #[derive(Script)]
/// #[script(thread_safety = send_sync)]
/// struct Point {
///     x: f64,
///     #[script(readonly)]
///     y: f64,
/// }
///
/// static DESC: haphe::StructDescriptor<'static> = <Point as ScriptStruct>::DESCRIPTOR;
/// assert_eq!(DESC.name, "Point");
/// assert!(DESC.fields[1].readonly);
/// ```
#[proc_macro_derive(Script, attributes(script))]
pub fn derive_script(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    derive::expand(input).into()
}

/// Attribute macro exposing an `impl` block or a free function to scripting
/// runtimes.
///
/// On an `impl` block, every function is exposed unless marked
/// `#[script(skip)]`. The type's `#[derive(Script)]` must declare
/// `#[script(methods)]`. Function attributes:
///
/// - `constructor` — a `fn(...) -> Self` without a receiver.
/// - `getter` / `setter` — a computed property: a `&self` getter, and a
///   `&mut self` setter named `set_<property>` (or `setter = "name"`).
/// - `rename = "name"` — the exposed name.
/// - `error_kind = "Kind"` — error-class hint for backend exception mapping.
/// - `skip` — hide the function.
///
/// Parameters accept `#[script(clone)]` for clone-at-boundary ownership and
/// `#[script(bytes)]` for byte-slice parameters. Functions gated by `#[cfg]`
/// are described only when compiled in. Getters take no parameters besides
/// `&self`; setters take exactly one and return nothing — the two must
/// describe the same script type (an `&str` getter pairs with a `String`
/// setter).
///
/// On a free function, the same options are given on the attribute itself
/// (e.g. `#[script(rename = "name")]`).
///
/// # Generic functions
///
/// A generic free function declares each concrete use with a repeatable
/// `instantiate(...)` option; the descriptor records the parameters and
/// instantiations, and binding registers one monomorphized wrapper per
/// instantiation (each identified by its type arguments):
///
/// ```
/// use haphe::script;
///
/// #[script(instantiate(i64), instantiate(String))]
/// fn echo<T>(value: T) -> T { value }
/// ```
///
/// Whether a backend accepts generic functions is governed by its generics
/// capability.
///
/// # Foreign traits
///
/// `#[script(foreign)]` on a trait declares functions Rust calls *out* to,
/// with bodies supplied by the embedding host. The macro re-emits the trait
/// and generates a `{Trait}Handle` struct implementing it: each method
/// converts its arguments, dispatches through a boxed `ForeignCaller`
/// (constructed via `ForeignHandle::from_caller`), and converts the result
/// back. The handle carries the interface descriptor via `ScriptForeign` and
/// is what `foreign:` entries in [`registry!`] name.
///
/// Attribute options: `rename = "Name"` and `thread_safety = none` (the only
/// supported value — handles are not thread-safe). Traits with `async`
/// methods must declare `thread_safety` explicitly. Method attributes accept
/// `rename` and `error_kind`.
///
/// Methods take `&self` and owned parameters. A `Result<T, E>` return
/// surfaces host failures through `E: From<ForeignError>` and is described
/// to the host as returning `T`; a non-`Result` method panics if the host
/// call fails.
///
/// Traits may declare type parameters (`trait Store<T>`); signatures
/// reference them and the descriptor stays erased. Each concrete use is
/// listed in [`registry!`] (`foreign: [StoreHandle<i32>]`), recorded as an
/// instantiation for backends that monomorphize, and gated by the backend's
/// generics capability.
///
/// Methods may also declare their own type parameters, with concrete uses
/// listed via `instantiate(...)` on the method. Dispatch passes the type
/// arguments' descriptors alongside the call, and the emitted trait method
/// gains the bounds the handle needs (`HapheType + FromScript`, plus
/// `ScriptValue: From<T>`). Lifetimes, const generics, and parameter
/// defaults are not supported.
///
/// ```
/// use haphe::{ForeignHandle, ScriptForeign, script};
///
/// #[script(foreign)]
/// pub trait HostHooks {
///     fn log(&self, message: String);
/// }
///
/// assert_eq!(<HostHooksHandle as ScriptForeign>::DESCRIPTOR.name, "HostHooks");
/// ```
///
/// # Example
///
/// ```
/// use haphe::{Script, ScriptImpl, script};
///
/// #[derive(Script)]
/// #[script(methods)]
/// struct Counter {
///     value: i64,
/// }
///
/// #[script]
/// impl Counter {
///     #[script(constructor)]
///     fn new() -> Self {
///         Counter { value: 0 }
///     }
///
///     fn increment(&mut self, by: i64) -> i64 {
///         self.value += by;
///         self.value
///     }
/// }
///
/// /// Doubles a number.
/// #[script]
/// fn double(n: i64) -> i64 {
///     n * 2
/// }
///
/// assert_eq!(<Counter as ScriptImpl>::CONSTRUCTORS[0].name, "new");
/// assert_eq!(<Counter as ScriptImpl>::METHODS[0].name, "increment");
/// ```
#[proc_macro_attribute]
pub fn script(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = proc_macro2::TokenStream::from(args);
    match syn::parse_macro_input!(item as syn::Item) {
        syn::Item::Impl(mut item) => {
            if args.is_empty() {
                imp::expand(item).into()
            } else {
                imp::strip_impl_script_attrs(&mut item);
                // On impl blocks, options belong on the functions inside.
                let error = syn::Error::new_spanned(
                    args,
                    "`#[script]` takes no arguments on an `impl` block; per-function \
                     options go on the functions inside (e.g. `#[script(constructor)]`)",
                )
                .to_compile_error();
                quote::quote! { #item #error }.into()
            }
        }
        syn::Item::Fn(mut item) => {
            // On free functions the outer attribute is the only place for
            // options: re-inject them so the shared parser sees them.
            if !args.is_empty() {
                item.attrs.push(syn::parse_quote! { #[script(#args)] });
            }
            freefn::expand(item).into()
        }
        syn::Item::Trait(mut item) => {
            // Same re-injection as free functions: the outer attribute is
            // where `foreign`, `rename`, and `thread_safety` live.
            if !args.is_empty() {
                item.attrs.push(syn::parse_quote! { #[script(#args)] });
            }
            foreign::expand(item).into()
        }
        other => syn::Error::new_spanned(
            &other,
            "`#[script]` applies to `impl` blocks, free functions, and traits",
        )
        .to_compile_error()
        .into(),
    }
}

/// Assembles a `static TypeRegistry` from derived types and `#[script]`
/// functions.
///
/// All sections are optional: `structs`, `enums`, `type_aliases` (newtypes
/// derived with `Script`), `modules`, and `foreign` (handle types generated
/// by `#[script(foreign)]` on traits). A module block accepts `doc`,
/// `functions`, `types`, `constants` (`NAME: Type = literal`, with optional
/// doc comments; literal values are checked against the declared type), and
/// nested `modules`. Functions and types are resolved through their traits,
/// so imports, re-exports, and concrete instantiations of generic types all
/// work. Doc comments and attributes before the `static` are kept on it.
///
/// # Example
///
/// ```
/// use haphe::{Script, script};
///
/// #[derive(Script)]
/// struct Point {
///     x: f64,
///     y: f64,
/// }
///
/// #[script]
/// fn origin_distance(x: f64, y: f64) -> f64 {
///     (x * x + y * y).sqrt()
/// }
///
/// haphe::registry! {
///     pub static REGISTRY = {
///         structs: [Point],
///         modules: [
///             mod geometry {
///                 doc: "Geometry utilities",
///                 functions: [origin_distance],
///                 types: [Point],
///                 constants: [
///                     /// The circle constant.
///                     PI: f64 = 3.14159265358979,
///                 ],
///             },
///         ],
///     };
/// }
///
/// let validated = REGISTRY.validate().unwrap();
/// # let _ = validated;
/// ```
#[proc_macro]
pub fn registry(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as registry::RegistryInput);
    registry::expand(input).into()
}
mod attrs;
mod fn_desc;
mod ty_map;
