//! The macro side's knowledge of standard-library types, in one place:
//! which idents are bridge primitives, semantic value types, containers, and
//! transparent carriers. The generic machinery in `bind::gates` and `ty_map`
//! consumes these tables; nothing outside this module names a std type.
//!
//! Everything here is judged syntactically, so types are recognized by their
//! unqualified spelling or an explicit `std`/`alloc`/`core` path.

use syn::Type;

/// Primitives with direct `FromScript`/`IntoScript` impls.
pub(crate) const BRIDGE_PRIMITIVES: &[&str] = &[
    "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "char", "String",
    "usize", "isize",
];

/// Bare-ident std types with string/tuple bridge representations (paths,
/// network addresses, time types, `NonZero*`).
pub(crate) const SEMANTIC_VALUE_TYPES: &[&str] = &[
    "PathBuf",
    "Duration",
    "SystemTime",
    "IpAddr",
    "Ipv4Addr",
    "Ipv6Addr",
    "SocketAddr",
    "SocketAddrV4",
    "SocketAddrV6",
    "NonZeroI8",
    "NonZeroI16",
    "NonZeroI32",
    "NonZeroI64",
    "NonZeroIsize",
    "NonZeroU8",
    "NonZeroU16",
    "NonZeroU32",
    "NonZeroU64",
    "NonZeroUsize",
];

/// Single-type-argument containers carried as lists (or their own shape).
pub(crate) const UNARY_CONTAINERS: &[&str] = &["Vec", "Option", "HashSet", "BTreeSet", "VecDeque"];

/// String-keyed map containers.
pub(crate) const STRING_MAPS: &[&str] = &["HashMap", "BTreeMap"];

/// Transparent carriers: erased at the bridge boundary, the carried type
/// crosses (descriptors delegate, conversions unwrap).
pub(crate) const TRANSPARENT_CARRIERS: &[&str] = &["Box", "Rc", "Arc", "Cow"];

/// The subset of carriers that carry a LIFETIME along with the value; their
/// descriptors fold to [`TypeDescriptor::Borrowed`], keeping the declared
/// lifetime for backends to interpret.
pub(crate) const LIFETIME_CARRIERS: &[&str] = &["Cow"];

/// Whether a path is plausibly the std item of that name: a bare name, or an
/// explicit `std`/`alloc`/`core` path. Another crate's same-named type
/// cannot be told apart syntactically, so namespaced paths never match.
pub(crate) fn is_std_path(path: &syn::Path) -> bool {
    path.segments.len() == 1
        || matches!(
            path.segments.first().map(|seg| seg.ident.to_string()),
            Some(ref first) if matches!(first.as_str(), "std" | "alloc" | "core")
        )
}

/// A transparent carrier's parts: its declared lifetime argument (if any)
/// and the single carried type. `None` when `ty` is not a recognized std
/// carrier.
pub(crate) fn carrier_parts(ty: &Type) -> Option<(CarrierKind, Option<syn::Lifetime>, &Type)> {
    let Type::Path(p) = ty else { return None };
    if p.qself.is_some() || !is_std_path(&p.path) {
        return None;
    }
    let last = p.path.segments.last()?;
    let name = last.ident.to_string();
    if !TRANSPARENT_CARRIERS.contains(&name.as_str()) {
        return None;
    }
    let kind = if LIFETIME_CARRIERS.contains(&name.as_str()) {
        CarrierKind::Lifetime
    } else {
        CarrierKind::Plain
    };
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let mut lifetime = None;
    let mut inner = None;
    for arg in &args.args {
        match arg {
            syn::GenericArgument::Lifetime(lt) => lifetime = Some(lt.clone()),
            syn::GenericArgument::Type(t) => {
                if inner.replace(t).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    inner.map(|t| (kind, lifetime, t))
}

/// Whether the carrier erases completely (`Box`) or carries its lifetime
/// into the descriptor (`Cow`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CarrierKind {
    Plain,
    Lifetime,
}

/// The owned form an unsized borrowed std type converts through
/// (`str` → `String`); other types are their own owned form.
pub(crate) fn owned_form(ty: &Type) -> Type {
    if let Type::Path(p) = ty
        && p.path.is_ident("str")
    {
        return syn::parse_quote!(String);
    }
    ty.clone()
}
