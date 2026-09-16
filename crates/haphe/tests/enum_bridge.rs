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

/// Payload variants keep opaque behavior: no generated conversions.
#[allow(dead_code)]
#[derive(Script)]
enum Shape {
    Circle(f64),
    Point,
}

#[test]
fn into_script_emits_exposed_case_names() {
    assert!(matches!(
        ScriptValue::from(Color::Red),
        ScriptValue::Enum { case } if case == "Red"
    ));
    assert!(matches!(
        ScriptValue::from(Color::DarkBlue),
        ScriptValue::Enum { case } if case == "DarkBlue"
    ));
    assert!(matches!(
        ScriptValue::from(Color::Gray),
        ScriptValue::Enum { case } if case == "grey"
    ));
}

#[test]
fn from_script_matches_declared_names_exactly() {
    let v = ScriptValue::Enum {
        case: "DarkBlue".to_string(),
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
