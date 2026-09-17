use haphe::{
    ConstantDescriptor, ModuleDescriptor, PrimitiveType, TypeDescriptor, ValidatedRegistry,
};
use mlua::{Lua, Table, Value};

use crate::LuaBindError;

/// Recursively binds a module descriptor into a Lua table.
pub(crate) fn bind_module(
    lua: &Lua,
    registry: &ValidatedRegistry<'_>,
    module: &ModuleDescriptor<'_>,
) -> Result<Table, LuaBindError> {
    let table = lua.create_table()?;

    // Constants
    for constant in module.constants {
        let value = constant_to_lua(lua, module.name, constant)?;
        table.set(constant.name, value)?;
    }

    // Type tables, keyed by the type's short name. Structs and aliases get
    // an empty stub; enums get a populated CASE TABLE: `Table.Case = value`
    // for every unit case, where the value is the case-name string for
    // string-represented enums and the integer discriminant for numeric
    // ones (a Rust `#[repr]` integer type; flags carry their real bit
    // values).
    for type_id in module.type_ids {
        let type_kind = registry.get_type(type_id);
        let name = type_kind
            .as_ref()
            .map(|tk| match tk {
                haphe::TypeKind::Struct(s) => s.name,
                haphe::TypeKind::Enum(e) => e.name,
                haphe::TypeKind::TypeAlias(a) => a.name,
            })
            .unwrap_or(type_id.as_str());

        // Only insert if the name isn't already taken by a constant or
        // function with the same name.
        if table.get::<Value>(name)?.is_nil() {
            let type_table = lua.create_table()?;
            if let Some(haphe::TypeKind::Enum(e)) = type_kind {
                for variant in e.variants {
                    if !matches!(variant.kind, haphe::VariantKind::Unit) {
                        continue;
                    }
                    if !type_table.get::<Value>(variant.name)?.is_nil() {
                        return Err(LuaBindError::DuplicateEnumCase {
                            enum_name: name.to_owned(),
                            case: variant.name.to_owned(),
                        });
                    }
                    match variant.discriminant {
                        Some(value) => type_table.set(variant.name, value)?,
                        None => type_table.set(variant.name, variant.name)?,
                    }
                }
            }
            table.set(name, type_table)?;
        }
    }

    // Submodules (recursive)
    for submodule in module.submodules {
        let sub_table = bind_module(lua, registry, submodule)?;
        table.set(submodule.name, sub_table)?;
    }

    // Free functions: stubs for now. Module-level free functions need
    // a FnBinder (analogous to TypeBinder for types) which is not yet
    // implemented. Each function gets a placeholder that errors with a
    // helpful message.
    for function in module.functions {
        let fn_name = function.name.to_owned();
        let mod_name = module.name.to_owned();
        let stub = lua.create_function(move |_, _args: mlua::MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::runtime(format!(
                "{mod_name}.{fn_name}: free function binding not yet implemented"
            )))
        })?;
        table.set(function.name, stub)?;
    }

    Ok(table)
}

/// Converts a constant descriptor's string value into a Lua value.
///
/// The IR stores constant values as string representations. This function
/// parses them according to the constant's declared type.
fn constant_to_lua(
    lua: &Lua,
    module_name: &str,
    constant: &ConstantDescriptor<'_>,
) -> Result<Value, LuaBindError> {
    let value = constant.value;
    let ty = crate::peel_borrowed(constant.ty);

    match ty {
        TypeDescriptor::Primitive(p) => {
            primitive_to_lua(lua, module_name, constant.name, *p, value)
        }
        TypeDescriptor::String => Ok(Value::String(lua.create_string(value)?)),
        TypeDescriptor::Unit => Ok(Value::Nil),
        // For types we can't yet convert, store as a string representation.
        _ => Ok(Value::String(lua.create_string(value)?)),
    }
}

fn primitive_to_lua(
    lua: &Lua,
    module_name: &str,
    name: &str,
    prim: PrimitiveType,
    value: &str,
) -> Result<Value, LuaBindError> {
    match prim {
        PrimitiveType::Bool => match value {
            "true" => Ok(Value::Boolean(true)),
            "false" => Ok(Value::Boolean(false)),
            _ => Err(LuaBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            }),
        },
        PrimitiveType::I8
        | PrimitiveType::I16
        | PrimitiveType::I32
        | PrimitiveType::I64
        | PrimitiveType::I128 => {
            let n: i64 = value.parse().map_err(|_| LuaBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            Ok(Value::Integer(n))
        }
        PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128 => {
            // Lua integers are signed 64-bit; large unsigned values may need
            // the number (float) representation.
            if let Ok(n) = value.parse::<i64>() {
                Ok(Value::Integer(n))
            } else if let Ok(n) = value.parse::<u64>() {
                // Fits in u64 but not i64 — store as float.
                Ok(Value::Number(n as f64))
            } else {
                Err(LuaBindError::InvalidConstant {
                    module: module_name.to_owned(),
                    name: name.to_owned(),
                    value: value.to_owned(),
                })
            }
        }
        PrimitiveType::F32 | PrimitiveType::F64 => {
            let n: f64 = value.parse().map_err(|_| LuaBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            Ok(Value::Number(n))
        }
        PrimitiveType::Char => {
            // A Rust char is a single Unicode scalar value; store as a
            // one-character Lua string.
            let c: char = value.parse().map_err(|_| LuaBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            Ok(Value::String(lua.create_string(s)?))
        }
        // PrimitiveType is non-exhaustive; future variants fall back to
        // string representation.
        _ => Ok(Value::String(lua.create_string(value)?)),
    }
}
