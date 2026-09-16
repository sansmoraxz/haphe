//! haphe's operator traits — the default bounds behind `traits(...)`
//! operator declarations.
//!
//! Each trait here is an *extension* of its standard-library counterpart: a
//! blanket implementation covers every type implementing the `core::ops`
//! trait of the same shape, so implementing the std trait is all a type
//! needs. The traits exist so operators without a std equivalent ([`Pow`],
//! [`IDiv`], [`Mod`]) fit the same mold.
//!
//! A type gets each operator from exactly one source: either its
//! `core::ops` implementation (through the blanket) or a direct
//! implementation of the trait here — implementing both for the same
//! operand type would overlap the blanket and is rejected by coherence.
//! Prefer the std trait when it exists (it also gives you the Rust-side
//! operator syntax); implement the haphe trait directly when there is no
//! std counterpart ([`Pow`], [`IDiv`], [`Mod`] — all three provided for the
//! standard numeric types) or to forward a third-party implementation in
//! one line — always coherent, never toggled by a cargo feature.

macro_rules! extend_binary {
    ($(#[$doc:meta])* $name:ident, $method:ident) => {
        $(#[$doc])*
        pub trait $name<Rhs = Self> {
            /// The result type of the operation.
            type Output;

            /// Applies the operation.
            fn $method(self, rhs: Rhs) -> Self::Output;
        }

        impl<T, Rhs> $name<Rhs> for T
        where
            T: ::core::ops::$name<Rhs>,
        {
            type Output = <T as ::core::ops::$name<Rhs>>::Output;

            fn $method(self, rhs: Rhs) -> Self::Output {
                ::core::ops::$name::$method(self, rhs)
            }
        }
    };
}

extend_binary!(
    /// Addition (`+`); extends [`core::ops::Add`].
    Add, add
);
extend_binary!(
    /// Subtraction (`-`); extends [`core::ops::Sub`].
    Sub, sub
);
extend_binary!(
    /// Multiplication (`*`); extends [`core::ops::Mul`].
    Mul, mul
);
extend_binary!(
    /// Division (`/`); extends [`core::ops::Div`].
    Div, div
);
extend_binary!(
    /// Remainder (`%`); extends [`core::ops::Rem`].
    Rem, rem
);
extend_binary!(
    /// Bitwise AND (`&`); extends [`core::ops::BitAnd`].
    BitAnd, bitand
);
extend_binary!(
    /// Bitwise OR (`|`); extends [`core::ops::BitOr`].
    BitOr, bitor
);
extend_binary!(
    /// Bitwise XOR (`^`); extends [`core::ops::BitXor`].
    BitXor, bitxor
);
extend_binary!(
    /// Left shift (`<<`); extends [`core::ops::Shl`].
    Shl, shl
);
extend_binary!(
    /// Right shift (`>>`); extends [`core::ops::Shr`].
    Shr, shr
);

macro_rules! extend_unary {
    ($(#[$doc:meta])* $name:ident, $method:ident) => {
        $(#[$doc])*
        pub trait $name {
            /// The result type of the operation.
            type Output;

            /// Applies the operation.
            fn $method(self) -> Self::Output;
        }

        impl<T> $name for T
        where
            T: ::core::ops::$name,
        {
            type Output = <T as ::core::ops::$name>::Output;

            fn $method(self) -> Self::Output {
                ::core::ops::$name::$method(self)
            }
        }
    };
}

extend_unary!(
    /// Negation (unary `-`); extends [`core::ops::Neg`].
    Neg, neg
);
extend_unary!(
    /// Bitwise NOT (unary `!` / `~`); extends [`core::ops::Not`].
    Not, not
);

/// Indexed read (`container[index]`); extends [`core::ops::Index`].
pub trait Index<Idx> {
    /// The element type.
    type Output: ?Sized;

    /// Returns the element at `index`, panicking if absent.
    fn index(&self, index: Idx) -> &Self::Output;
}

impl<T, Idx> Index<Idx> for T
where
    T: ::core::ops::Index<Idx> + ?Sized,
{
    type Output = <T as ::core::ops::Index<Idx>>::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        ::core::ops::Index::index(self, index)
    }
}

/// Indexed write (`container[index] = value`); extends
/// [`core::ops::IndexMut`].
pub trait IndexMut<Idx>: Index<Idx> {
    /// Returns a mutable reference to the element at `index`, panicking if
    /// absent.
    fn index_mut(&mut self, index: Idx) -> &mut Self::Output;
}

impl<T, Idx> IndexMut<Idx> for T
where
    T: ::core::ops::IndexMut<Idx> + ?Sized,
{
    fn index_mut(&mut self, index: Idx) -> &mut Self::Output {
        ::core::ops::IndexMut::index_mut(self, index)
    }
}

/// Callable values (`value(...)` invocation).
///
/// The standard library's `Fn*` traits cannot back an exposable call
/// operator (they are not implementable on stable Rust), so this trait
/// fills the role: `Args` is the parameter list as a tuple (`()`, `(i64,)`,
/// `(i64, String)`, ...), invoked through a shared reference so calling
/// never consumes the value. No blanket extension and no std-type
/// implementations — implement it directly.
pub trait Call<Args> {
    /// The result type of the invocation.
    type Output;

    /// Invokes the value with `args`.
    fn call(&self, args: Args) -> Self::Output;
}

/// Asynchronously callable values (`value(...)` invocation awaiting a
/// result).
///
/// The async sibling of [`Call`], for hosts whose call bodies suspend.
/// Declared with `traits(AsyncCall(args = ..., output = ...))`; the type
/// must declare `thread_safety` explicitly, and binding is gated by the
/// backend's async capability.
pub trait AsyncCall<Args> {
    /// The result type of the invocation.
    type Output;

    /// Invokes the value with `args`.
    fn call_async(&self, args: Args) -> impl ::core::future::Future<Output = Self::Output>;
}

/// Floor division (Lua's `//`-style integer division).
///
/// The standard library defines no floor-division trait, so this one has no
/// blanket extension: implement it directly. haphe provides it for the
/// standard numeric types with true floor semantics — the quotient is
/// rounded toward negative infinity (`-7.idiv(2) == -4`), matching
/// scripting-language integer division rather than Rust's truncating `/`
/// or `div_euclid`.
pub trait IDiv<Rhs = Self> {
    /// The result type of the operation.
    type Output;

    /// Floor-divides `self` by `rhs`.
    fn idiv(self, rhs: Rhs) -> Self::Output;
}

macro_rules! idiv_int_impls {
    ($($t:ty),*) => {$(
        impl IDiv for $t {
            type Output = $t;
            #[allow(unused_comparisons)]
            fn idiv(self, rhs: $t) -> $t {
                let q = self / rhs;
                let r = self % rhs;
                // Truncation and floor differ when the remainder is nonzero
                // and the operands' signs differ.
                if r != 0 && ((r < 0) != (rhs < 0)) { q - 1 } else { q }
            }
        }
    )*};
}

idiv_int_impls!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! idiv_float_impls {
    ($($t:ty),*) => {$(
        impl IDiv for $t {
            type Output = $t;
            fn idiv(self, rhs: $t) -> $t {
                (self / rhs).floor()
            }
        }
    )*};
}

idiv_float_impls!(f32, f64);

/// Floor modulo (Lua's `%` semantics).
///
/// The result takes the divisor's sign, satisfying `a == a.idiv(b) * b +
/// a.modulo(b)` with [`IDiv`]'s floor quotient — `(-7).modulo(2) == 1`.
/// This differs from Rust's truncated `%` (`Rem`, result takes the
/// dividend's sign) and from `rem_euclid` (always non-negative). No std
/// counterpart, so no blanket extension: implement it directly (haphe
/// provides it for the standard numeric types). The method is named
/// `modulo` because `mod` is a Rust keyword.
pub trait Mod<Rhs = Self> {
    /// The result type of the operation.
    type Output;

    /// Floor-modulo of `self` by `rhs`.
    fn modulo(self, rhs: Rhs) -> Self::Output;
}

macro_rules! mod_int_impls {
    ($($t:ty),*) => {$(
        impl Mod for $t {
            type Output = $t;
            #[allow(unused_comparisons)]
            fn modulo(self, rhs: $t) -> $t {
                let r = self % rhs;
                // Truncated and floor modulo differ when the remainder is
                // nonzero and the operands' signs differ.
                if r != 0 && ((r < 0) != (rhs < 0)) { r + rhs } else { r }
            }
        }
    )*};
}

mod_int_impls!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! mod_float_impls {
    ($($t:ty),*) => {$(
        impl Mod for $t {
            type Output = $t;
            fn modulo(self, rhs: $t) -> $t {
                let r = self % rhs;
                if r != 0.0 && (r < 0.0) != (rhs < 0.0) { r + rhs } else { r }
            }
        }
    )*};
}

mod_float_impls!(f32, f64);

/// Exponentiation.
///
/// The standard library defines no power trait, so this one has no blanket
/// extension: implement it directly. haphe provides it for the standard
/// numeric types, mirroring their inherent power methods — integers raise
/// by a `u32` exponent, floats by a float (`powf`) or an `i32` (`powi`). A
/// type whose exponentiation lives in a third-party trait (e.g.
/// `num_traits::Pow`) forwards it in a one-line implementation.
pub trait Pow<Rhs = Self> {
    /// The result type of the operation.
    type Output;

    /// Raises `self` to the power of `rhs`.
    fn pow(self, rhs: Rhs) -> Self::Output;
}

macro_rules! pow_int_impls {
    ($($t:ty),*) => {$(
        impl Pow<u32> for $t {
            type Output = $t;
            fn pow(self, rhs: u32) -> $t {
                <$t>::pow(self, rhs)
            }
        }
    )*};
}

pow_int_impls!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! pow_float_impls {
    ($($t:ty),*) => {$(
        impl Pow for $t {
            type Output = $t;
            fn pow(self, rhs: $t) -> $t {
                <$t>::powf(self, rhs)
            }
        }

        impl Pow<i32> for $t {
            type Output = $t;
            fn pow(self, rhs: i32) -> $t {
                <$t>::powi(self, rhs)
            }
        }
    )*};
}

pow_float_impls!(f32, f64);
