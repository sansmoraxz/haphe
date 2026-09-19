//! Unit-only enums cross the bridge as `ScriptValue::Enum` carrying the
//! declared case name, matched exactly; backends translate native spellings
//! at their own boundary.

#![cfg(feature = "macros")]

use haphe::{FromScript, Script, ScriptValue};

/// A named color.
#[derive(Script, Debug, PartialEq)]
enum Color {
    Red,
    DarkBlue,
    #[script(rename = "grey")]
    Gray,
}

#[test]
fn into_script_emits_exposed_case_names() {
    assert!(matches!(
        ScriptValue::from(Color::Red),
        ScriptValue::Enum { case, .. } if case == "Red"
    ));
    assert!(matches!(
        ScriptValue::from(Color::DarkBlue),
        ScriptValue::Enum { case, .. } if case == "DarkBlue"
    ));
    assert!(matches!(
        ScriptValue::from(Color::Gray),
        ScriptValue::Enum { case, .. } if case == "grey"
    ));
}

#[test]
fn from_script_matches_declared_names_exactly() {
    let v = ScriptValue::Enum {
        discriminant: None,
        case: "DarkBlue".to_string(),
        payload: vec![],
    };
    assert_eq!(Color::from_script(v).unwrap(), Color::DarkBlue);
    // Plain strings work too (dynamic backends deliver declared names).
    let v = ScriptValue::String("Red".to_string());
    assert_eq!(Color::from_script(v).unwrap(), Color::Red);
    // Renames are the exposed identity.
    let v = ScriptValue::String("grey".to_string());
    assert_eq!(Color::from_script(v).unwrap(), Color::Gray);
    // Native spellings are a backend concern: core matches exactly.
    let v = ScriptValue::String("dark-blue".to_string());
    assert!(Color::from_script(v).is_err());
}

#[test]
fn from_script_rejects_unknown_cases_and_shapes() {
    let err = Color::from_script(ScriptValue::String("magenta".to_string())).unwrap_err();
    assert_eq!(err.expected, "Color");
    assert_eq!(err.got, "unknown enum case");
    let err = Color::from_script(ScriptValue::I64(0)).unwrap_err();
    assert_eq!(err.got, "i64");
}

#[test]
fn tuples_round_trip_as_lists() {
    let v = ScriptValue::from((1i64, "two".to_string(), Color::Red));
    let ScriptValue::List(ref items) = v else {
        panic!("expected list");
    };
    assert_eq!(items.len(), 3);
    let back: (i64, String, Color) = FromScript::from_script(v).unwrap();
    assert_eq!(back, (1, "two".to_string(), Color::Red));

    let err = <(i64, i64)>::from_script(ScriptValue::List(vec![ScriptValue::I64(1)])).unwrap_err();
    assert_eq!(err.got, "list of mismatched length");
}

// Numeric enums: the Rust `#[repr]` integer type propagates exactly, explicit
// and implicit discriminants are recorded, and plain integers convert.
#[derive(haphe::Script, Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub enum Level {
    Low = 1,
    Mid, // implicit 2
    High = 10,
}

#[test]
fn numeric_enum_descriptor_and_roundtrip() {
    use haphe::{PrimitiveType, ScriptEnum};
    let desc = <Level as ScriptEnum>::DESCRIPTOR;
    assert_eq!(desc.repr, Some(PrimitiveType::U8));
    let discs: Vec<Option<i64>> = desc.variants.iter().map(|v| v.discriminant).collect();
    assert_eq!(discs, vec![Some(1), Some(2), Some(10)]);

    // Outbound carries the discriminant.
    assert!(matches!(
        ScriptValue::from(Level::Mid),
        ScriptValue::Enum { ref case, discriminant: Some(2), .. } if case == "Mid"
    ));
    // Inbound accepts name, Enum, and plain integer.
    assert_eq!(
        Level::from_script(ScriptValue::I64(10)).unwrap(),
        Level::High
    );
    assert_eq!(
        Level::from_script(ScriptValue::String("Low".into())).unwrap(),
        Level::Low
    );
    assert!(Level::from_script(ScriptValue::I64(3)).is_err());
}

#[test]
fn string_enum_carries_no_discriminant() {
    // The pre-existing Color enum has no #[repr] — string representation.
    use haphe::ScriptEnum;
    let desc = <Color as ScriptEnum>::DESCRIPTOR;
    assert_eq!(desc.repr, None);
    assert!(desc.variants.iter().all(|v| v.discriminant.is_none()));
    assert!(matches!(
        ScriptValue::from(Color::Red),
        ScriptValue::Enum {
            discriminant: None,
            ..
        }
    ));
    // Plain integers do NOT convert for string enums.
    assert!(Color::from_script(ScriptValue::I64(0)).is_err());
}

// ── Payload enums: cases cross with their carried values — tuple variants
// positionally, struct variants' fields in declaration order.

#[derive(Script, Debug, PartialEq, Clone)]
enum Shape {
    Empty,
    Circle(f64),
    Rect { width: f64, height: f64 },
    Label(String, i64),
}

#[test]
fn payload_cases_cross_with_their_values() {
    match ScriptValue::from(Shape::Circle(2.5)) {
        ScriptValue::Enum {
            case,
            discriminant,
            payload,
        } => {
            assert_eq!(case, "Circle");
            assert_eq!(discriminant, None);
            assert!(matches!(payload[..], [ScriptValue::F64(v)] if v == 2.5));
        }
        other => panic!("expected enum, got {other:?}"),
    }
    match ScriptValue::from(Shape::Rect {
        width: 3.0,
        height: 4.0,
    }) {
        ScriptValue::Enum { case, payload, .. } => {
            assert_eq!(case, "Rect");
            assert!(matches!(
                payload[..],
                [ScriptValue::F64(w), ScriptValue::F64(h)] if w == 3.0 && h == 4.0
            ));
        }
        other => panic!("expected enum, got {other:?}"),
    }
}

#[test]
fn payload_cases_roundtrip() {
    for shape in [
        Shape::Empty,
        Shape::Circle(1.5),
        Shape::Rect {
            width: 2.0,
            height: 3.0,
        },
        Shape::Label("tag".into(), 7),
    ] {
        let back = Shape::from_script(ScriptValue::from(shape.clone())).unwrap();
        assert_eq!(back, shape);
    }
}

#[test]
fn payload_conversion_errors_are_descriptive() {
    // Unit case with a payload.
    let err = Shape::from_script(ScriptValue::Enum {
        case: "Empty".into(),
        discriminant: None,
        payload: vec![ScriptValue::I64(1)],
    })
    .unwrap_err();
    assert_eq!(err.got, "payload on a unit case");
    // Payload case with the wrong arity.
    let err = Shape::from_script(ScriptValue::Enum {
        case: "Rect".into(),
        discriminant: None,
        payload: vec![ScriptValue::F64(1.0)],
    })
    .unwrap_err();
    assert_eq!(err.got, "payload of mismatched length");
    // A payload case named as a plain string has no payload.
    let err = Shape::from_script(ScriptValue::String("Circle".into())).unwrap_err();
    assert_eq!(err.got, "payload of mismatched length");
    // Field conversion failures surface.
    let err = Shape::from_script(ScriptValue::Enum {
        case: "Circle".into(),
        discriminant: None,
        payload: vec![ScriptValue::String("no".into())],
    })
    .unwrap_err();
    assert_eq!(err.expected, "f64");
}
