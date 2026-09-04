use crate::function::FunctionDescriptor;

/// Unique identifier for a type registered in a [`crate::TypeRegistry`].
///
/// Two types with the same `TypeId` are considered the same type. Use
/// [`TypeDescriptor::Ref`] to reference a registered type by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId<'a>(&'a str);

impl<'a> TypeId<'a> {
    pub const fn new(id: &'a str) -> Self {
        Self(id)
    }

    pub const fn as_str(&self) -> &'a str {
        self.0
    }

    /// Equality usable in `const` contexts.
    pub const fn const_eq(&self, other: &TypeId<'_>) -> bool {
        const_str_eq(self.0, other.0)
    }
}

const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

impl std::fmt::Display for TypeId<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Language-agnostic description of a Rust type.
///
/// The data-shape variants (Primitive through Ref/Unit) are adapted from
/// Specta's `DataType`, extended with behavioral constructs (`Callback`) for
/// scripting-engine binding generation.
///
/// Nests arbitrarily — e.g. `Option<Vec<Result<MyStruct, String>>>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypeDescriptor<'a> {
    /// A primitive numeric, boolean, or character type.
    Primitive(PrimitiveType),
    /// Rust `String` / `&str`.
    String,
    /// Byte slice (`Vec<u8>` / `&[u8]`).
    Bytes,
    /// `Option<T>` — a value that may be absent.
    Option(&'a TypeDescriptor<'a>),
    /// `Vec<T>` — an ordered, variable-length collection.
    List(&'a TypeDescriptor<'a>),
    /// `[T; N]` — a fixed-size array.
    Array(&'a TypeDescriptor<'a>, usize),
    /// `HashMap<K, V>` — a key-value mapping.
    Map(&'a TypeDescriptor<'a>, &'a TypeDescriptor<'a>),
    /// A fixed-size heterogeneous sequence, e.g. `(i32, String)`.
    Tuple(&'a [TypeDescriptor<'a>]),
    /// `Result<T, E>` — a fallible return type.
    Result(&'a TypeDescriptor<'a>, &'a TypeDescriptor<'a>),
    /// A callback / closure, e.g. `Fn(i32, String) -> bool`.
    Callback {
        /// Parameter types of the callback.
        params: &'a [TypeDescriptor<'a>],
        /// Return type of the callback.
        return_type: &'a TypeDescriptor<'a>,
    },
    /// Reference to a user-defined type registered in the [`crate::TypeRegistry`].
    Ref(TypeId<'a>),
    /// The unit type `()`.
    Unit,
    /// A reference to a generic type parameter by name (e.g. `"T"`).
    ///
    /// Only valid inside a [`StructDescriptor`] or [`EnumDescriptor`] that
    /// declares a [`GenericParam`] with a matching name.
    GenericParam(&'a str),
}

impl TypeDescriptor<'_> {
    /// Structural equality usable in `const` contexts.
    ///
    /// Mirrors the `PartialEq` implementation; two descriptors are equal when
    /// they describe the same script-visible type.
    pub const fn const_eq(&self, other: &TypeDescriptor<'_>) -> bool {
        use TypeDescriptor as T;
        match (self, other) {
            (T::Primitive(a), T::Primitive(b)) => *a as u8 == *b as u8,
            (T::String, T::String) | (T::Bytes, T::Bytes) | (T::Unit, T::Unit) => true,
            (T::Option(a), T::Option(b)) | (T::List(a), T::List(b)) => a.const_eq(b),
            (T::Array(a, n), T::Array(b, m)) => *n == *m && a.const_eq(b),
            (T::Map(ka, va), T::Map(kb, vb)) | (T::Result(ka, va), T::Result(kb, vb)) => {
                ka.const_eq(kb) && va.const_eq(vb)
            }
            (T::Tuple(a), T::Tuple(b)) => const_slice_eq(a, b),
            (
                T::Callback {
                    params: pa,
                    return_type: ra,
                },
                T::Callback {
                    params: pb,
                    return_type: rb,
                },
            ) => const_slice_eq(pa, pb) && ra.const_eq(rb),
            (T::Ref(a), T::Ref(b)) => a.const_eq(b),
            (T::GenericParam(a), T::GenericParam(b)) => const_str_eq(a, b),
            _ => false,
        }
    }
}

const fn const_slice_eq(a: &[TypeDescriptor<'_>], b: &[TypeDescriptor<'_>]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if !a[i].const_eq(&b[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// Rust primitive types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrimitiveType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Char,
}

/// A standard Rust trait implemented by a described type.
///
/// Backends map these to native constructs: `Display` → pyo3 `__str__`,
/// mlua `__tostring`, JS `toString()`; `Add` → pyo3 `__add__`, mlua
/// `MetaMethod::Add`, rhai `+` operator; etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraitImpl<'a> {
    // Formatting / comparison (no associated types)
    Display,
    Debug,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Clone,
    Default,

    // Arithmetic operators (with associated types)
    Add {
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    Sub {
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    Mul {
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    Div {
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    Rem {
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    Neg {
        output: &'a TypeDescriptor<'a>,
    },

    // Indexing
    Index {
        index: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    IndexMut {
        index: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },

    // Iteration
    Iterator {
        item: &'a TypeDescriptor<'a>,
    },
    IntoIterator {
        item: &'a TypeDescriptor<'a>,
    },
}

/// Thread-safety markers for a type.
///
/// Backends use these to decide wrapping strategies: e.g. some requires
/// `Send` for types shared across threads, some both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSafety {
    /// Whether the type implements `Send`.
    pub is_send: bool,
    /// Whether the type implements `Sync`.
    pub is_sync: bool,
}

impl ThreadSafety {
    /// Both `Send` and `Sync`.
    pub const SEND_SYNC: Self = Self {
        is_send: true,
        is_sync: true,
    };
    /// `Send` only.
    pub const SEND: Self = Self {
        is_send: true,
        is_sync: false,
    };
    /// Neither `Send` nor `Sync`.
    pub const NONE: Self = Self {
        is_send: false,
        is_sync: false,
    };
}

/// A generic type parameter declared on a struct or enum.
///
/// Bounds are represented as string slices of trait names (e.g. `"Display"`,
/// `"Clone"`). Backends match on well-known trait names to decide
/// monomorphization or wrapper-generation strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericParam<'a> {
    /// Parameter name, e.g. `"T"`, `"K"`, `"V"`.
    pub name: &'a str,
    /// Trait bounds on this parameter.
    pub bounds: &'a [&'a str],
    /// Default type, if any (e.g. `T = i32`).
    pub default: Option<&'a TypeDescriptor<'a>>,
}

/// A computed property (getter/setter) on a type, distinct from a direct
/// struct field.
///
/// Backends generate accessor methods: pyo3 → `#[getter]`/`#[setter]`,
/// rhai → `register_get`/`register_set`, mlua → index metamethods, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyDescriptor<'a> {
    /// Property name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The property's type.
    pub ty: &'a TypeDescriptor<'a>,
    /// Whether the property is read-only (getter only, no setter).
    pub readonly: bool,
}

/// A Rust struct exposed to scripting languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructDescriptor<'a> {
    /// Unique identifier used for cross-references via [`TypeDescriptor::Ref`].
    pub id: TypeId<'a>,
    /// The struct's Rust name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// Public fields to expose.
    pub fields: &'a [FieldDescriptor<'a>],
    /// Methods (`&self`, `&mut self`, or `self`) and associated functions
    /// (`receiver: None` = static method) to expose.
    pub methods: &'a [FunctionDescriptor<'a>],
    /// Constructor functions (no receiver, return `Self`).
    pub constructors: &'a [FunctionDescriptor<'a>],
    /// Computed properties backed by getter/setter methods.
    pub properties: &'a [PropertyDescriptor<'a>],
    /// Standard trait implementations this type provides.
    pub trait_impls: &'a [TraitImpl<'a>],
    /// Thread-safety markers (`Send` / `Sync`).
    pub thread_safety: ThreadSafety,
    /// Generic type parameters declared on this type.
    pub generic_params: &'a [GenericParam<'a>],
}

/// A single field within a struct or struct-variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDescriptor<'a> {
    /// Field name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The field's type.
    pub ty: &'a TypeDescriptor<'a>,
    /// Whether the field is read-only from the scripting side.
    pub readonly: bool,
}

/// A Rust enum exposed to scripting languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumDescriptor<'a> {
    /// Unique identifier used for cross-references via [`TypeDescriptor::Ref`].
    pub id: TypeId<'a>,
    /// The enum's Rust name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The enum's variants.
    pub variants: &'a [EnumVariant<'a>],
    /// Methods on the enum.
    pub methods: &'a [FunctionDescriptor<'a>],
    /// Standard trait implementations this enum provides.
    pub trait_impls: &'a [TraitImpl<'a>],
    /// Thread-safety markers (`Send` / `Sync`).
    pub thread_safety: ThreadSafety,
    /// Generic type parameters declared on this enum.
    pub generic_params: &'a [GenericParam<'a>],
}

/// A single variant of an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumVariant<'a> {
    /// Variant name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The shape of data this variant carries.
    pub kind: VariantKind<'a>,
}

/// The data shape of an enum variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantKind<'a> {
    /// Unit variant, e.g. `Color::Red`.
    Unit,
    /// Tuple variant, e.g. `Color::Rgb(u8, u8, u8)`.
    Tuple(&'a [TypeDescriptor<'a>]),
    /// Struct variant, e.g. `Color::Named { name: String }`.
    Struct(&'a [FieldDescriptor<'a>]),
}

/// A type alias or newtype wrapper exposed to scripting languages.
///
/// Covers both `type Meters = f64` (pure alias) and `struct Meters(f64)`
/// (newtype). The [`transparent`](TypeAliasDescriptor::transparent) flag tells
/// backends whether to expose the alias as the inner type (transparent) or
/// preserve it as a distinct named type in the target language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeAliasDescriptor<'a> {
    /// Unique identifier, usable via [`TypeDescriptor::Ref`].
    pub id: TypeId<'a>,
    /// The alias/newtype name (e.g. `"Meters"`).
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The underlying type being aliased or wrapped.
    pub inner: &'a TypeDescriptor<'a>,
    /// If `true`, backends should treat this as equivalent to `inner`.
    /// If `false`, backends should register it as a distinct type.
    pub transparent: bool,
}
