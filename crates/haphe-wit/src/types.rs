use haphe::{Ownership, PrimitiveType, TypeDescriptor};

use crate::WitGenError;
use crate::model::Plan;

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
    context: &str,
) -> Result<Option<String>, WitGenError> {
    if matches!(ty, TypeDescriptor::Unit) {
        return Ok(None);
    }
    render_type(ty, Pos::Return(ownership), plan, context).map(Some)
}

pub(crate) fn render_type(
    ty: &TypeDescriptor<'_>,
    pos: Pos,
    plan: &Plan<'_>,
    context: &str,
) -> Result<String, WitGenError> {
    match ty {
        TypeDescriptor::Primitive(p) => render_primitive(*p, context),
        TypeDescriptor::String => Ok("string".to_string()),
        TypeDescriptor::Bytes => Ok("list<u8>".to_string()),
        TypeDescriptor::Option(inner) => Ok(format!(
            "option<{}>",
            render_type(inner, nested(pos), plan, context)?
        )),
        TypeDescriptor::List(inner) | TypeDescriptor::Array(inner, _) => Ok(format!(
            "list<{}>",
            render_type(inner, nested(pos), plan, context)?
        )),
        TypeDescriptor::Map(k, v) => Ok(format!(
            "list<tuple<{}, {}>>",
            render_type(k, nested(pos), plan, context)?,
            render_type(v, nested(pos), plan, context)?
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
                .map(|e| render_type(e, nested(pos), plan, context))
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
                    render_type(err, nested(pos), plan, context)?
                ),
                (false, true) => {
                    format!("result<{}>", render_type(ok, nested(pos), plan, context)?)
                }
                (false, false) => format!(
                    "result<{}, {}>",
                    render_type(ok, nested(pos), plan, context)?,
                    render_type(err, nested(pos), plan, context)?
                ),
            })
        }
        TypeDescriptor::Ref(id) => {
            let name = plan.type_name(id.as_str()).to_string();
            if !plan.is_resource(id.as_str()) {
                return Ok(name);
            }
            match pos {
                Pos::Param(Ownership::Ref | Ownership::RefMut) => Ok(format!("borrow<{name}>")),
                Pos::Param(_) | Pos::Field => Ok(name),
                Pos::Return(Ownership::Owned | Ownership::Clone) => Ok(name),
                Pos::Return(Ownership::Ref | Ownership::RefMut) => {
                    Err(WitGenError::BorrowedResourceReturn {
                        type_id: id.as_str().to_string(),
                        function: context.to_string(),
                    })
                }
            }
        }
        TypeDescriptor::Unit => Err(WitGenError::UnrepresentableType {
            context: context.to_string(),
            detail: "unit type is only valid as a bare return type".to_string(),
        }),
        // Callback, GenericParam, and future variants are rejected by the
        // capability check before generation; this arm is defensive.
        _ => Err(WitGenError::UnrepresentableType {
            context: context.to_string(),
            detail: format!("unsupported type descriptor: {ty:?}"),
        }),
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
