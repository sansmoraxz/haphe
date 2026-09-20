use haphe::{Ownership, PrimitiveType, TypeDescriptor};

use crate::WitGenError;
use crate::model::{Env, Plan};

/// Values cross the component boundary by copy: a lifetime-carrying
/// descriptor (`Cow<'a, T>`) lowers as its carried type, and the lifetime
/// does not exist in WIT. Applied at the entry of every descriptor match so
/// nested occurrences normalize uniformly.
pub(crate) fn peel_borrowed<'b, 'a>(mut ty: &'b TypeDescriptor<'a>) -> &'b TypeDescriptor<'a> {
    while let TypeDescriptor::Borrowed { inner, .. } = ty {
        ty = inner;
    }
    ty
}

/// Whether a signature type mentions one of the function's own generic
/// parameters. Such positions carry the concrete instantiation in dynamic
/// dispatch: they become case variants in synthesized dispatchers and in
/// erased (`dyn`) foreign imports.
#[cfg_attr(
    not(any(feature = "runtime", feature = "dyn-generics")),
    allow(dead_code, reason = "only dynamic-dispatch machinery consumes it")
)]
pub(crate) fn mentions_generic(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::GenericParam(_) => true,
        TypeDescriptor::Option(inner)
        | TypeDescriptor::List(inner)
        | TypeDescriptor::Stream(inner)
        | TypeDescriptor::Future(inner)
        | TypeDescriptor::Borrowed { inner, .. }
        | TypeDescriptor::Array(inner, _) => mentions_generic(inner),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            mentions_generic(k) || mentions_generic(v)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(mentions_generic),
        TypeDescriptor::Callback {
            params,
            return_type,
        } => params.iter().any(mentions_generic) || mentions_generic(return_type),
        TypeDescriptor::Instance { args, .. } => args.iter().any(mentions_generic),
        _ => false,
    }
}

/// Whether `ty` mentions one of `params` by name — the check dispatcher
/// machinery uses so that only the METHOD's own generic parameters become
/// variant cases; a generic SELF type's parameters render concretely per
/// monomorph and pass through.
#[cfg_attr(
    not(feature = "dyn-generics"),
    allow(dead_code, reason = "only synthesized dispatcher emission consumes it")
)]
pub(crate) fn mentions_named_generic(
    ty: &TypeDescriptor<'_>,
    params: &[haphe::GenericParam<'_>],
) -> bool {
    match ty {
        TypeDescriptor::GenericParam(name) => params.iter().any(|p| p.name == *name),
        TypeDescriptor::Option(inner)
        | TypeDescriptor::List(inner)
        | TypeDescriptor::Stream(inner)
        | TypeDescriptor::Future(inner)
        | TypeDescriptor::Borrowed { inner, .. }
        | TypeDescriptor::Array(inner, _) => mentions_named_generic(inner, params),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            mentions_named_generic(k, params) || mentions_named_generic(v, params)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(|e| mentions_named_generic(e, params)),
        TypeDescriptor::Callback {
            params: cb_params,
            return_type,
        } => {
            cb_params.iter().any(|p| mentions_named_generic(p, params))
                || mentions_named_generic(return_type, params)
        }
        TypeDescriptor::Instance { args, .. } => {
            args.iter().any(|a| mentions_named_generic(a, params))
        }
        _ => false,
    }
}

/// Where a type appears; determines resource handle rendering and whether
/// Unit is allowed.
#[derive(Clone, Copy)]
pub(crate) enum Pos {
    Param(Ownership),
    Return(Ownership),
    Field,
}

/// Renders a type in return position: `Ok(None)` means no `-> T` arrow.
pub(crate) fn render_return(
    ty: &TypeDescriptor<'_>,
    ownership: Ownership,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<Option<String>, WitGenError> {
    if matches!(peel_borrowed(ty), TypeDescriptor::Unit) {
        return Ok(None);
    }
    render_type(ty, Pos::Return(ownership), plan, env, context).map(Some)
}
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive pass per descriptor/channel shape; splitting the walk would scatter the per-shape rules"
)]
pub(crate) fn render_type(
    ty: &TypeDescriptor<'_>,
    pos: Pos,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<String, WitGenError> {
    match peel_borrowed(ty) {
        TypeDescriptor::Primitive(p) => render_primitive(*p, context),
        TypeDescriptor::String => Ok("string".to_string()),
        TypeDescriptor::Bytes => Ok("list<u8>".to_string()),
        TypeDescriptor::Option(inner) => Ok(format!(
            "option<{}>",
            render_type(inner, nested(pos), plan, env, context)?
        )),
        TypeDescriptor::List(inner) => Ok(format!(
            "list<{}>",
            render_type(inner, nested(pos), plan, env, context)?
        )),
        TypeDescriptor::Array(inner, len) => Ok(format!(
            "list<{}, {len}>",
            render_type(inner, nested(pos), plan, env, context)?
        )),
        TypeDescriptor::Map(k, v) => Ok(format!(
            "list<tuple<{}, {}>>",
            render_type(k, nested(pos), plan, env, context)?,
            render_type(v, nested(pos), plan, env, context)?
        )),
        TypeDescriptor::Tuple(elems) => {
            if elems.is_empty() {
                return Err(WitGenError::UnrepresentableType {
                    context: context.to_string(),
                    detail: "empty tuple has no WIT equivalent".to_string(),
                });
            }
            let rendered: Vec<String> = elems
                .iter()
                .map(|e| render_type(e, nested(pos), plan, env, context))
                .collect::<Result<_, _>>()?;
            Ok(format!("tuple<{}>", rendered.join(", ")))
        }
        TypeDescriptor::Result(ok, err) => {
            let ok_unit = matches!(ok, TypeDescriptor::Unit);
            let err_unit = matches!(err, TypeDescriptor::Unit);
            Ok(match (ok_unit, err_unit) {
                (true, true) => "result".to_string(),
                (true, false) => format!(
                    "result<_, {}>",
                    render_type(err, nested(pos), plan, env, context)?
                ),
                (false, true) => {
                    format!(
                        "result<{}>",
                        render_type(ok, nested(pos), plan, env, context)?
                    )
                }
                (false, false) => format!(
                    "result<{}, {}>",
                    render_type(ok, nested(pos), plan, env, context)?,
                    render_type(err, nested(pos), plan, env, context)?
                ),
            })
        }
        TypeDescriptor::Ref(id) => {
            if plan.is_generic(id.as_str()) {
                // Only self-references inside the instance being emitted are
                // meaningful for a bare (argument-less) generic ref.
                return match env {
                    Some(e) if e.self_id == id.as_str() => {
                        render_named(e.self_name.to_string(), true, pos, id.as_str(), context)
                    }
                    _ => Err(WitGenError::UnrepresentableType {
                        context: context.to_string(),
                        detail: format!(
                            "bare reference to generic type `{}` (missing type arguments)",
                            id.as_str()
                        ),
                    }),
                };
            }
            let name = plan.type_name(id.as_str()).to_string();
            render_named(
                name,
                plan.is_resource(id.as_str()),
                pos,
                id.as_str(),
                context,
            )
        }
        TypeDescriptor::Instance { id, args } => {
            let name = plan.instance_ref_name(id.as_str(), args, env)?;
            render_named(
                name,
                plan.is_resource(id.as_str()),
                pos,
                id.as_str(),
                context,
            )
        }
        TypeDescriptor::GenericParam(name) => {
            let bound = env.and_then(|e| e.lookup(name)).ok_or_else(|| {
                WitGenError::UnrepresentableType {
                    context: context.to_string(),
                    detail: format!("unbound generic parameter `{name}`"),
                }
            })?;
            // Instantiation args are concrete, so no further env applies.
            render_type(bound, pos, plan, None, context)
        }
        TypeDescriptor::Stream(inner) => Ok(if matches!(inner, TypeDescriptor::Unit) {
            "stream".to_string()
        } else {
            format!(
                "stream<{}>",
                render_type(inner, nested(pos), plan, env, context)?
            )
        }),
        TypeDescriptor::Future(inner) => Ok(if matches!(inner, TypeDescriptor::Unit) {
            "future".to_string()
        } else {
            format!(
                "future<{}>",
                render_type(inner, nested(pos), plan, env, context)?
            )
        }),
        TypeDescriptor::Unit => Err(WitGenError::UnrepresentableType {
            context: context.to_string(),
            detail: "unit type is only valid as a bare return type".to_string(),
        }),
        // Callback and future variants are rejected by the capability check
        // before generation; this arm is defensive.
        _ => Err(WitGenError::UnrepresentableType {
            context: context.to_string(),
            detail: format!("unsupported type descriptor: {ty:?}"),
        }),
    }
}

/// Position-dependent rendering of a named type: resources become handles
/// (`borrow<t>` in by-ref params, owned in returns — borrowed returns are
/// invalid WIT).
fn render_named(
    name: String,
    is_resource: bool,
    pos: Pos,
    type_id: &str,
    context: &str,
) -> Result<String, WitGenError> {
    if !is_resource {
        return Ok(name);
    }
    match pos {
        Pos::Param(Ownership::Ref | Ownership::RefMut) => Ok(format!("borrow<{name}>")),
        Pos::Param(_) | Pos::Field | Pos::Return(Ownership::Owned | Ownership::Clone) => Ok(name),
        Pos::Return(Ownership::Ref | Ownership::RefMut) => {
            Err(WitGenError::BorrowedResourceReturn {
                type_id: type_id.to_string(),
                function: context.to_string(),
            })
        }
    }
}

fn render_primitive(p: PrimitiveType, context: &str) -> Result<String, WitGenError> {
    let s = match p {
        PrimitiveType::Bool => "bool",
        PrimitiveType::I8 => "s8",
        PrimitiveType::I16 => "s16",
        PrimitiveType::I32 => "s32",
        PrimitiveType::I64 => "s64",
        PrimitiveType::U8 => "u8",
        PrimitiveType::U16 => "u16",
        PrimitiveType::U32 => "u32",
        PrimitiveType::U64 => "u64",
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::Char => "char",
        PrimitiveType::I128 | PrimitiveType::U128 => {
            return Err(WitGenError::UnrepresentableType {
                context: context.to_string(),
                detail: "WIT has no 128-bit integer type".to_string(),
            });
        }
        other => {
            return Err(WitGenError::UnrepresentableType {
                context: context.to_string(),
                detail: format!("unsupported primitive: {other:?}"),
            });
        }
    };
    Ok(s.to_string())
}

/// Inside a compound type, resource refs are always owned handles.
fn nested(pos: Pos) -> Pos {
    match pos {
        Pos::Return(_) => Pos::Return(Ownership::Owned),
        Pos::Param(_) | Pos::Field => Pos::Field,
    }
}
