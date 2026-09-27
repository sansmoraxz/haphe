use haphe::{ConstantDescriptor, ModuleDescriptor, PrimitiveType, TypeDescriptor, ValidatedRegistry};
use rhai::{Dynamic, Module};

use crate::RhaiBindError;

pub(crate) fn bind_module(
    registry: &ValidatedRegistry<'_>,
    module: &ModuleDescriptor<'_>,
) -> Result<Module, RhaiBindError> {
    let mut rhai_module = Module::new();

    for constant in module.constants {
        let value = constant_to_dynamic(module.name, constant)?;
        rhai_module.set_var(constant.name, value);
    }

    for type_id in module.type_ids {
        let type_kind = registry.get_type(type_id);
        let name = type_kind.as_ref().map_or(type_id.as_str(), |tk| match tk {
            haphe::TypeKind::Struct(s) => s.name,
            haphe::TypeKind::Enum(e) => e.name,
            haphe::TypeKind::TypeAlias(a) => a.name,
        });

        if let Some(haphe::TypeKind::Enum(e)) = type_kind {
            let mut case_module = Module::new();
            for variant in e.variants {
                match &variant.kind {
                    haphe::VariantKind::Unit => {
                        match variant.discriminant {
                            Some(value) => {
                                case_module.set_var(variant.name, Dynamic::from(value));
                            }
                            None => {
                                case_module
                                    .set_var(variant.name, Dynamic::from(variant.name.to_owned()));
                            }
                        }
                    }
                    haphe::VariantKind::Tuple(_) | haphe::VariantKind::Struct(_) => {
                        // Payload enum constructors are registered via
                        // bind_enum_type; the module holds only unit cases.
                    }
                }
            }
            rhai_module.set_sub_module(name, case_module);
        } else {
            rhai_module.set_sub_module(name, Module::new());
        }
    }

    for submodule in module.submodules {
        let sub = bind_module(registry, submodule)?;
        rhai_module.set_sub_module(submodule.name, sub);
    }

    Ok(rhai_module)
}

fn constant_to_dynamic(
    module_name: &str,
    constant: &ConstantDescriptor<'_>,
) -> Result<Dynamic, RhaiBindError> {
    let value = constant.value;
    let ty = peel_borrowed(constant.ty);

    match ty {
        TypeDescriptor::Primitive(p) => primitive_to_dynamic(module_name, constant.name, *p, value),
        TypeDescriptor::Unit => Ok(Dynamic::UNIT),
        TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Ref(_) => Ok(Dynamic::from(value.to_owned())),
        _ => Err(RhaiBindError::InvalidConstant {
            module: module_name.to_owned(),
            name: constant.name.to_owned(),
            value: value.to_owned(),
        }),
    }
}

fn peel_borrowed<'a>(ty: &'a TypeDescriptor<'a>) -> &'a TypeDescriptor<'a> {
    match ty {
        TypeDescriptor::Borrowed { inner, .. } => peel_borrowed(inner),
        other => other,
    }
}

fn primitive_to_dynamic(
    module_name: &str,
    name: &str,
    prim: PrimitiveType,
    value: &str,
) -> Result<Dynamic, RhaiBindError> {
    match prim {
        PrimitiveType::Bool => match value {
            "true" => Ok(Dynamic::from(true)),
            "false" => Ok(Dynamic::from(false)),
            _ => Err(RhaiBindError::InvalidConstant {
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
            let n: i64 = value.parse().map_err(|_| RhaiBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            Ok(Dynamic::from(n))
        }
        PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128 => {
            if let Ok(n) = value.parse::<i64>() {
                Ok(Dynamic::from(n))
            } else if let Ok(n) = value.parse::<u64>() {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "large u64 values outside i64 range cross as float, matching the Lua backend"
                )]
                Ok(Dynamic::from(n as f64))
            } else {
                Err(RhaiBindError::InvalidConstant {
                    module: module_name.to_owned(),
                    name: name.to_owned(),
                    value: value.to_owned(),
                })
            }
        }
        PrimitiveType::F32 | PrimitiveType::F64 => {
            let n: f64 = value.parse().map_err(|_| RhaiBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            Ok(Dynamic::from(n))
        }
        PrimitiveType::Char => {
            let c: char = value.parse().map_err(|_| RhaiBindError::InvalidConstant {
                module: module_name.to_owned(),
                name: name.to_owned(),
                value: value.to_owned(),
            })?;
            Ok(Dynamic::from(c))
        }
        _ => unreachable!("unhandled PrimitiveType variant"),
    }
}
