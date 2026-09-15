use std::fmt;
use std::marker::PhantomData;

use haphe::{
    BackendCapabilities, BindingGenerator, ConstantDescriptor, PrimitiveType, Receiver,
    RuntimeBinder, TypeDescriptor, TypeKind, ValidatedRegistry,
};
use wasmtime::component::{Linker, LinkerInstance, ResourceType, Val};

use crate::model::Plan;
use crate::names::NameMap;
use crate::{ConstantMode, WitGenError, WitGenerator};

/// Binds a haphe registry into a wasmtime component [`Linker`] as host
/// (import-direction) definitions, mirroring the WIT document the same
/// [`WitGenerator`] configuration produces.
pub struct WasmBinder<T = ()> {
    config: WitGenerator,
    _marker: PhantomData<fn(T)>,
}

impl<T> WasmBinder<T> {
    /// Creates a binder from the same configuration used to generate the
    /// `.wit` document, so linker names always match the generated file.
    pub fn new(config: WitGenerator) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

/// Placeholder host representation for resources until real dispatch lands.
struct StubResource;

/// Errors produced while binding into a wasmtime linker.
#[derive(Debug)]
pub enum WasmBindError {
    /// The shared planning/naming pass failed (same errors as generation).
    Gen(WitGenError),
    /// A constant's IR value string could not be parsed as its declared type.
    UnsupportedConstant {
        /// The constant's name.
        name: String,
        /// Why the value could not be converted.
        detail: String,
    },
    /// wasmtime rejected a linker definition.
    Wasm(wasmtime::Error),
}

impl fmt::Display for WasmBindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gen(e) => write!(f, "{e}"),
            Self::UnsupportedConstant { name, detail } => {
                write!(f, "constant `{name}` cannot be bound: {detail}")
            }
            Self::Wasm(e) => write!(f, "wasmtime linker error: {e}"),
        }
    }
}

impl std::error::Error for WasmBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gen(e) => Some(e),
            _ => None,
        }
    }
}

impl From<WitGenError> for WasmBindError {
    fn from(e: WitGenError) -> Self {
        Self::Gen(e)
    }
}

impl From<wasmtime::Error> for WasmBindError {
    fn from(e: wasmtime::Error) -> Self {
        Self::Wasm(e)
    }
}

impl<T: 'static> RuntimeBinder for WasmBinder<T> {
    type Runtime = Linker<T>;
    type Error = WasmBindError;

    fn language_name(&self) -> &str {
        "wasm"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BindingGenerator::capabilities(&self.config)
    }

    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        linker: &mut Linker<T>,
    ) -> Result<(), WasmBindError> {
        crate::validate_package_name(&self.config.package)?;
        let plan = Plan::build(registry, &self.config.default_interface)?;

        for iface in &plan.interfaces {
            let empty = iface.type_ids.is_empty()
                && iface.instance_indices.is_empty()
                && iface.functions.is_empty()
                && (iface.constants.is_empty() || self.config.constants == ConstantMode::Skip);
            if empty {
                continue;
            }
            let instance_name = match &self.config.version {
                Some(v) => format!("{}/{}@{v}", self.config.package, iface.name),
                None => format!("{}/{}", self.config.package, iface.name),
            };
            let mut inst = linker.instance(&instance_name)?;
            let mut member_names = NameMap::new();

            for id in &iface.type_ids {
                match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
                    TypeKind::Struct(s) if plan.is_resource(id) => {
                        bind_resource(&mut inst, s, plan.type_name(id))?;
                    }
                    // Records, enums, variants, and aliases are type-only in
                    // WIT: nothing to define in a linker.
                    _ => {}
                }
            }

            // Monomorphized generic instantiations, same mangled names the
            // generator emits.
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Struct(s)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                    && plan.is_resource(planned.erased_id)
                {
                    bind_resource(&mut inst, s, &planned.wit_name)?;
                }
            }

            // Companion functions for enum methods, same names the generator
            // emits.
            for id in &iface.type_ids {
                if let Some(TypeKind::Enum(e)) = registry.get_type(&haphe::TypeId::new(id)) {
                    for m in e.methods {
                        let name = member_names.insert(&format!("{}_{}", e.name, m.name))?;
                        stub_func(&mut inst, &name)?;
                    }
                }
            }
            for &i in &iface.instance_indices {
                let planned = &plan.instances[i];
                if let Some(TypeKind::Enum(e)) =
                    registry.get_type(&haphe::TypeId::new(planned.erased_id))
                {
                    for m in e.methods {
                        let name =
                            member_names.insert(&format!("{}-{}", planned.wit_name, m.name))?;
                        stub_func(&mut inst, &name)?;
                    }
                }
            }

            for func in iface.functions {
                let name = member_names.insert(func.name)?;
                stub_func(&mut inst, &name)?;
            }

            if self.config.constants == ConstantMode::Getter {
                for c in iface.constants {
                    let name = member_names.insert(c.name)?;
                    let val = constant_to_val(c)?;
                    inst.func_new(&name, move |_store, _ty, _params, results| {
                        results[0] = val.clone();
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
}

/// Registers a resource placeholder plus trap-stubs for its constructor,
/// field/property accessors, and methods, using the same member names the
/// generator emits (canonical-ABI prefixed).
fn bind_resource<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    s: &haphe::StructDescriptor<'_>,
    res: &str,
) -> Result<(), WasmBindError> {
    inst.resource(res, ResourceType::host::<StubResource>(), |_, _| Ok(()))?;
    let mut members = NameMap::new();

    for field in s.fields {
        let getter = members.insert(field.name)?;
        stub_func(inst, &format!("[method]{res}.{getter}"))?;
        if !field.readonly {
            let setter = members.insert(&format!("set_{}", field.name))?;
            stub_func(inst, &format!("[method]{res}.{setter}"))?;
        }
    }
    for prop in s.properties {
        let getter = members.insert(prop.name)?;
        stub_func(inst, &format!("[method]{res}.{getter}"))?;
        if !prop.readonly {
            let setter = members.insert(&format!("set_{}", prop.name))?;
            stub_func(inst, &format!("[method]{res}.{setter}"))?;
        }
    }

    let mut ctor_slot_free = true;
    for ctor in s.constructors {
        if ctor_slot_free && !ctor.is_async {
            ctor_slot_free = false;
            stub_func(inst, &format!("[constructor]{res}"))?;
        } else {
            let name = members.insert(ctor.name)?;
            stub_func(inst, &format!("[static]{res}.{name}"))?;
        }
    }

    for m in s.methods {
        let name = members.insert(m.name)?;
        let prefixed = match m.receiver {
            Some(Receiver::Ref | Receiver::RefMut) => format!("[method]{res}.{name}"),
            Some(Receiver::Owned) | None => format!("[static]{res}.{name}"),
        };
        stub_func(inst, &prefixed)?;
    }
    Ok(())
}

fn stub_func<T: 'static>(
    inst: &mut LinkerInstance<'_, T>,
    name: &str,
) -> Result<(), WasmBindError> {
    let msg = format!("haphe-wit: `{name}` not yet implemented");
    inst.func_new(name, move |_store, _ty, _params, _results| {
        Err(wasmtime::Error::msg(msg.clone()))
    })?;
    Ok(())
}

/// Parses a constant's IR string value into a component [`Val`]
/// or returns an error if the value cannot be represented in the declared type.
fn constant_to_val(c: &ConstantDescriptor<'_>) -> Result<Val, WasmBindError> {
    let err = |detail: &str| WasmBindError::UnsupportedConstant {
        name: c.name.to_string(),
        detail: detail.to_string(),
    };
    let v = c.value;
    match c.ty {
        TypeDescriptor::String => Ok(Val::String(v.to_string())),
        TypeDescriptor::Primitive(p) => match p {
            PrimitiveType::Bool => match v {
                "true" => Ok(Val::Bool(true)),
                "false" => Ok(Val::Bool(false)),
                _ => Err(err("not a bool")),
            },
            PrimitiveType::I8 => v.parse().map(Val::S8).map_err(|_| err("not an s8")),
            PrimitiveType::I16 => v.parse().map(Val::S16).map_err(|_| err("not an s16")),
            PrimitiveType::I32 => v.parse().map(Val::S32).map_err(|_| err("not an s32")),
            PrimitiveType::I64 => v.parse().map(Val::S64).map_err(|_| err("not an s64")),
            PrimitiveType::U8 => v.parse().map(Val::U8).map_err(|_| err("not a u8")),
            PrimitiveType::U16 => v.parse().map(Val::U16).map_err(|_| err("not a u16")),
            PrimitiveType::U32 => v.parse().map(Val::U32).map_err(|_| err("not a u32")),
            PrimitiveType::U64 => v.parse().map(Val::U64).map_err(|_| err("not a u64")),
            PrimitiveType::F32 => v.parse().map(Val::Float32).map_err(|_| err("not an f32")),
            PrimitiveType::F64 => v.parse().map(Val::Float64).map_err(|_| err("not an f64")),
            PrimitiveType::Char => v.parse().map(Val::Char).map_err(|_| err("not a char")),
            _ => Err(err("unsupported primitive constant type")),
        },
        _ => Err(err("only primitive and string constants can be bound")),
    }
}
