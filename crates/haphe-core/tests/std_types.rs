//! Bridge coverage for commonly used std types: descriptors and value
//! roundtrips, grouped like the `std_types` module.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::{NonZeroI64, NonZeroU8};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use haphe_core::bridge::{FromScript, ScriptValue};
use haphe_core::haphe_type::HapheType;
use haphe_core::types::{PrimitiveType, TypeDescriptor};

fn roundtrip<T>(value: T) -> T
where
    T: FromScript + Clone,
    ScriptValue: From<T>,
{
    T::from_script(ScriptValue::from(value)).unwrap()
}

#[test]
fn pointer_width_integers() {
    assert_eq!(roundtrip(42usize), 42);
    assert_eq!(roundtrip(-42isize), -42);
    assert!(matches!(ScriptValue::from(7usize), ScriptValue::I64(7)));
}

#[test]
fn wide_integers_convert_inbound_only() {
    assert_eq!(i128::from_script(ScriptValue::I64(-5)).unwrap(), -5);
    assert_eq!(u128::from_script(ScriptValue::I64(5)).unwrap(), 5);
    let err = u128::from_script(ScriptValue::I64(-1)).unwrap_err();
    assert_eq!(err.got, "negative integer");
}

#[test]
fn non_zero_integers() {
    assert_eq!(roundtrip(NonZeroI64::new(9).unwrap()).get(), 9);
    assert_eq!(
        NonZeroU8::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::U8)
    );
    let err = NonZeroI64::from_script(ScriptValue::I64(0)).unwrap_err();
    assert_eq!(err.got, "zero");
    assert_eq!(err.expected, "NonZeroI64");
}

#[test]
fn ordered_and_unordered_collections() {
    let mut btree = BTreeMap::new();
    btree.insert("a".to_string(), 1i64);
    btree.insert("b".to_string(), 2i64);
    assert_eq!(roundtrip(btree.clone()), btree);

    let set: HashSet<i64> = [3, 1, 2].into_iter().collect();
    assert_eq!(roundtrip(set.clone()), set);

    let bset: BTreeSet<i64> = [3, 1, 2].into_iter().collect();
    assert!(matches!(
        ScriptValue::from(bset.clone()),
        ScriptValue::List(items) if matches!(items[0], ScriptValue::I64(1))
    ));
    assert_eq!(roundtrip(bset.clone()), bset);

    let deque: VecDeque<i64> = [1, 2, 3].into_iter().collect();
    assert_eq!(roundtrip(deque.clone()), deque);

    assert_eq!(
        HashSet::<i64>::DESCRIPTOR,
        TypeDescriptor::List(&TypeDescriptor::Primitive(PrimitiveType::I64))
    );
}

#[test]
fn fixed_arrays_check_length() {
    assert_eq!(roundtrip([1i64, 2, 3]), [1, 2, 3]);
    let err = <[i64; 3]>::from_script(ScriptValue::List(vec![ScriptValue::I64(1)])).unwrap_err();
    assert_eq!(err.got, "list of mismatched length");
}

#[test]
fn slices_convert_outbound() {
    let slice: &[i64] = &[4, 5];
    assert!(matches!(
        ScriptValue::from(slice),
        ScriptValue::List(items) if items.len() == 2
    ));
}

#[test]
fn cow_is_described_as_borrowed() {
    use haphe_core::dispatch::{GenericSubst, MatchQuality, value_matches_descriptor};
    assert_eq!(
        <std::borrow::Cow<'_, str>>::DESCRIPTOR,
        TypeDescriptor::Borrowed {
            lifetime: None,
            inner: &TypeDescriptor::String,
        }
    );
    // The lifetime never changes what value shape matches.
    let desc = TypeDescriptor::Borrowed {
        lifetime: Some("a"),
        inner: &TypeDescriptor::String,
    };
    let subst = GenericSubst {
        params: &[],
        args: &[],
    };
    assert_eq!(
        value_matches_descriptor(&ScriptValue::String("x".into()), &desc, &subst),
        MatchQuality::Exact
    );
    assert_eq!(
        value_matches_descriptor(&ScriptValue::I64(1), &desc, &subst),
        MatchQuality::No
    );
}

#[test]
fn smart_pointers_erase_at_the_boundary() {
    assert_eq!(*roundtrip(Box::new(7i64)), 7);
    assert_eq!(*roundtrip(Rc::new("x".to_string())), "x");
    assert_eq!(*roundtrip(Arc::new(1.5f64)), 1.5);
    let shared = Rc::new(3i64);
    let _keep = Rc::clone(&shared);
    assert!(matches!(ScriptValue::from(shared), ScriptValue::I64(3)));
    assert!(matches!(ScriptValue::from("hi"), ScriptValue::String(s) if s == "hi"));
    let cow: std::borrow::Cow<'_, str> = std::borrow::Cow::Borrowed("c");
    assert!(matches!(ScriptValue::from(cow), ScriptValue::String(s) if s == "c"));
}

#[test]
fn paths_cross_as_strings() {
    assert_eq!(PathBuf::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(Path::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(roundtrip(PathBuf::from("/tmp/x")), PathBuf::from("/tmp/x"));
    assert!(matches!(
        ScriptValue::from(Path::new("/a/b")),
        ScriptValue::String(s) if s == "/a/b"
    ));
}

#[test]
fn network_addresses_cross_as_canonical_strings() {
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    assert_eq!(roundtrip(ip), ip);
    let v4: Ipv4Addr = "10.0.0.1".parse().unwrap();
    assert_eq!(roundtrip(v4), v4);
    let sock: SocketAddr = "[::1]:8080".parse().unwrap();
    assert_eq!(roundtrip(sock), sock);
    let err = IpAddr::from_script(ScriptValue::String("not-an-ip".into())).unwrap_err();
    assert_eq!(err.got, "malformed address string");
}

#[test]
fn durations_cross_as_secs_nanos_tuples() {
    assert_eq!(
        Duration::DESCRIPTOR,
        TypeDescriptor::Tuple(&[
            TypeDescriptor::Primitive(PrimitiveType::U64),
            TypeDescriptor::Primitive(PrimitiveType::U32),
        ])
    );
    let d = Duration::new(3, 500_000_000);
    assert!(matches!(
        ScriptValue::from(d),
        ScriptValue::List(ref items)
            if matches!(items[..], [ScriptValue::I64(3), ScriptValue::I64(500_000_000)])
    ));
    assert_eq!(roundtrip(d), d);
    let err = Duration::from_script(ScriptValue::from((1u64, 2_000_000_000u32))).unwrap_err();
    assert_eq!(err.got, "nanos out of range");
}

#[test]
fn system_time_is_signed_since_epoch() {
    let after = UNIX_EPOCH + Duration::new(10, 250);
    assert_eq!(roundtrip(after), after);
    let before = UNIX_EPOCH - Duration::new(10, 250);
    assert_eq!(roundtrip(before), before);
    // Pre-epoch values carry negative seconds with normalized nanos.
    match ScriptValue::from(UNIX_EPOCH - Duration::new(1, 1)) {
        ScriptValue::List(items) => {
            assert!(matches!(
                items[..],
                [ScriptValue::I64(-2), ScriptValue::I64(999_999_999)]
            ));
        }
        other => panic!("expected tuple, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Numeric boundary policy: narrow integers range-check; u64 keeps the
// bit-pattern policy; char never truncates.
// ---------------------------------------------------------------------------

#[test]
fn narrow_integers_range_check_instead_of_wrapping() {
    use haphe_core::{FromScript, ScriptValue};
    assert_eq!(u8::from_script(ScriptValue::I64(255)).unwrap(), 255);
    assert!(u8::from_script(ScriptValue::I64(300)).is_err());
    assert!(u8::from_script(ScriptValue::I64(-1)).is_err());
    assert!(i8::from_script(ScriptValue::I64(128)).is_err());
    assert_eq!(i32::from_script(ScriptValue::I64(-5)).unwrap(), -5);
    assert!(i32::from_script(ScriptValue::I64(i64::from(i32::MAX) + 1)).is_err());
    assert!(u32::from_script(ScriptValue::I64(i64::from(u32::MAX) + 1)).is_err());
}

#[test]
fn u64_keeps_the_bit_pattern_policy() {
    use haphe_core::{FromScript, ScriptValue};
    // u64 has no lossless i64 representation: it crosses as the bit pattern.
    let v = ScriptValue::from(u64::MAX);
    assert!(matches!(v, ScriptValue::I64(-1)));
    assert_eq!(u64::from_script(ScriptValue::I64(-1)).unwrap(), u64::MAX);
}

#[test]
fn char_conversion_never_truncates() {
    use haphe_core::{FromScript, ScriptValue};
    assert_eq!(
        char::from_script(ScriptValue::String("x".into())).unwrap(),
        'x'
    );
    assert!(char::from_script(ScriptValue::String("xy".into())).is_err());
    assert!(char::from_script(ScriptValue::String(String::new())).is_err());
}
