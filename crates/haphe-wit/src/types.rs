use haphe::{Ownership, PrimitiveType, TypeDescriptor};

use crate::WitGenError;
use crate::model::{Env, Plan};

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
    if matches!(ty, TypeDescriptor::Unit) {
        return Ok(None);
    }
    render_type(ty, Pos::Return(ownership), plan, env, context).map(Some)
}

pub(crate) fn render_type(
    ty: &TypeDescriptor<'_>,
    pos: Pos,
    plan: &Plan<'_>,
    env: Option<&Env<'_, '_>>,
    context: &str,
) -> Result<String, WitGenError> {
    match ty {
        TypeDescriptor::Primitive(p) => render_primitive(*p, context),
        TypeDescriptor::String => Ok("string".to_string()),
        TypeDescriptor::Bytes => Ok("list<u8>".to_string()),
        TypeDescriptor::Option(inner) => Ok(format!(
            "option<{}>",
            render_type(inner, nested(pos), plan, env, context)?
        )),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => Ok(format!(
            "list<{}>",
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
        Pos::Param(_) | Pos::Field => Ok(name),
        Pos::Return(Ownership::Owned | Ownership::Clone) => Ok(name),
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
