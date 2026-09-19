//! Matcher and resolver semantics for dyn-dispatched generics.

use haphe_core::dispatch::{
    DynCandidate, GenericSubst, MatchQuality, Resolution, resolve_dyn_candidate,
    value_matches_descriptor,
};
use haphe_core::{
    Dispatch, FunctionDescriptor, GenericParam, OpaqueUserData, Ownership, ParamDescriptor,
    PrimitiveType, ScriptValue, TypeDescriptor, TypeId,
};

const NO_SUBST: GenericSubst<'static> = GenericSubst {
    params: &[],
    args: &[],
};

fn q(value: &ScriptValue, desc: &TypeDescriptor<'_>) -> MatchQuality {
    value_matches_descriptor(value, desc, &NO_SUBST)
}

#[test]
fn matcher_policy_table() {
    use MatchQuality::{Coercible, Exact, No};
    use PrimitiveType as P;
    use TypeDescriptor as T;

    // Integers: width-erased Exact; floats coercible; float vs int No.
    assert_eq!(q(&ScriptValue::I64(1), &T::Primitive(P::U8)), Exact);
    assert_eq!(q(&ScriptValue::I64(1), &T::Primitive(P::I64)), Exact);
    assert_eq!(q(&ScriptValue::I64(1), &T::Primitive(P::F64)), Coercible);
    assert_eq!(q(&ScriptValue::F64(1.5), &T::Primitive(P::F64)), Exact);
    assert_eq!(q(&ScriptValue::F64(1.5), &T::Primitive(P::I64)), No);

    // Strings and chars.
    assert_eq!(q(&ScriptValue::String("x".into()), &T::String), Exact);
    assert_eq!(
        q(&ScriptValue::String("x".into()), &T::Primitive(P::Char)),
        Coercible
    );
    assert_eq!(
        q(&ScriptValue::String("xy".into()), &T::Primitive(P::Char)),
        No
    );
    assert_eq!(q(&ScriptValue::Char('x'), &T::String), Coercible);

    // Containers: first-element heuristic; empty is Coercible (unknown).
    static I64_D: TypeDescriptor<'static> = T::Primitive(P::I64);
    let list = ScriptValue::List(vec![ScriptValue::I64(1)]);
    assert_eq!(q(&list, &T::List(&I64_D)), Exact);
    assert_eq!(q(&ScriptValue::List(vec![]), &T::List(&I64_D)), Coercible);
    assert_eq!(
        q(
            &ScriptValue::List(vec![ScriptValue::String("s".into())]),
            &T::List(&I64_D)
        ),
        No
    );

    // Optionals.
    assert_eq!(q(&ScriptValue::Optional(None), &T::Option(&I64_D)), Exact);
    assert_eq!(q(&ScriptValue::Optional(None), &I64_D), No);
    assert_eq!(
        q(
            &ScriptValue::Optional(Some(Box::new(ScriptValue::I64(1)))),
            &T::Option(&I64_D)
        ),
        Exact
    );

    // Enum vs Ref: coercible (registry-free heuristic; try-call decides).
    assert_eq!(
        q(
            &ScriptValue::Enum {
                case: "A".into(),
                discriminant: None,
                payload: vec![],
            },
            &T::Ref(TypeId::new("m::E"))
        ),
        Coercible
    );

    // UserData: tag-exact, tag-mismatch No, untagged Coercible.
    #[derive(Clone)]
    struct Thing;
    impl haphe_core::ScriptType for Thing {
        const ID: TypeId<'static> = TypeId::new("m::Thing");
    }
    let tagged = ScriptValue::UserData(OpaqueUserData::new_typed(Thing));
    assert_eq!(q(&tagged, &T::Ref(TypeId::new("m::Thing"))), Exact);
    assert_eq!(q(&tagged, &T::Ref(TypeId::new("m::Other"))), No);
    let untagged = ScriptValue::UserData(OpaqueUserData::new(Thing));
    assert_eq!(q(&untagged, &T::Ref(TypeId::new("m::Thing"))), Coercible);
}

#[test]
fn generic_params_resolve_through_subst() {
    static I64_D: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I64);
    let subst = GenericSubst {
        params: &[GenericParam {
            name: "T",
            bounds: &[],
            default: None,
        }],
        args: &[I64_D],
    };
    assert_eq!(
        value_matches_descriptor(
            &ScriptValue::I64(1),
            &TypeDescriptor::GenericParam("T"),
            &subst
        ),
        MatchQuality::Exact
    );
    assert_eq!(
        value_matches_descriptor(
            &ScriptValue::String("x".into()),
            &TypeDescriptor::GenericParam("T"),
            &subst
        ),
        MatchQuality::No
    );
}

const fn dyn_fn(params: &'static [ParamDescriptor<'static>]) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name: "echo",
        doc: None,
        receiver: None,
        generic_params: &[GenericParam {
            name: "T",
            bounds: &[],
            default: None,
        }],
        instantiations: &[],
        dispatch: Dispatch::Dyn,
        params,
        return_type: &TypeDescriptor::Unit,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }
}

static T_PARAM: [ParamDescriptor<'static>; 1] = [ParamDescriptor {
    name: "value",
    ty: &TypeDescriptor::GenericParam("T"),
    ownership: Ownership::Owned,
}];
static DESC: FunctionDescriptor<'static> = dyn_fn(&T_PARAM);
static I64_ARGS: [TypeDescriptor<'static>; 1] = [TypeDescriptor::Primitive(PrimitiveType::I64)];
static F64_ARGS: [TypeDescriptor<'static>; 1] = [TypeDescriptor::Primitive(PrimitiveType::F64)];
static STR_ARGS: [TypeDescriptor<'static>; 1] = [TypeDescriptor::String];

#[test]
fn resolver_two_pass_and_tiebreak() {
    let candidates = [
        DynCandidate {
            type_args: &F64_ARGS,
            descriptor: &DESC,
        },
        DynCandidate {
            type_args: &I64_ARGS,
            descriptor: &DESC,
        },
        DynCandidate {
            type_args: &STR_ARGS,
            descriptor: &DESC,
        },
    ];
    // Exact pass: I64 value picks the i64 candidate over the earlier
    // coercible f64 one.
    assert_eq!(
        resolve_dyn_candidate(&[ScriptValue::I64(5)], &candidates),
        Resolution::Ranked(1)
    );
    // String exact.
    assert_eq!(
        resolve_dyn_candidate(&[ScriptValue::String("x".into())], &candidates),
        Resolution::Ranked(2)
    );
    // F64 value: exact on candidate 0.
    assert_eq!(
        resolve_dyn_candidate(&[ScriptValue::F64(2.0)], &candidates),
        Resolution::Ranked(0)
    );
    // Nothing fits (arity mismatch): try-call order.
    assert_eq!(
        resolve_dyn_candidate(&[], &candidates),
        Resolution::TryCallOrder
    );
    // Bool fits no candidate shape: try-call order.
    assert_eq!(
        resolve_dyn_candidate(&[ScriptValue::Bool(true)], &candidates),
        Resolution::TryCallOrder
    );
}

#[test]
fn coercible_pass_declaration_order_tiebreak() {
    // Two float candidates: an I64 value is Coercible to both; the first
    // declared wins.
    let candidates = [
        DynCandidate {
            type_args: &F64_ARGS,
            descriptor: &DESC,
        },
        DynCandidate {
            type_args: &F64_ARGS,
            descriptor: &DESC,
        },
    ];
    assert_eq!(
        resolve_dyn_candidate(&[ScriptValue::I64(5)], &candidates),
        Resolution::Ranked(0)
    );
}
