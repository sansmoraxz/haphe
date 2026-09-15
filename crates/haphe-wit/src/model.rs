use std::collections::{BTreeMap, HashMap, HashSet};

use haphe::{
    ConstantDescriptor, FunctionDescriptor, ModuleDescriptor, TypeDescriptor, TypeKind,
    ValidatedRegistry, VariantKind,
};

use crate::WitGenError;
use crate::names::{NameMap, to_kebab};

/// One WIT interface: a flattened module (or the default interface holding
/// types unclaimed by any module).
pub(crate) struct Iface<'a> {
    pub name: String,
    pub doc: Option<&'a str>,
    pub functions: &'a [FunctionDescriptor<'a>],
    pub constants: &'a [ConstantDescriptor<'a>],
    /// TypeIds (as strings) of types defined in this interface, in order.
    pub type_ids: Vec<&'a str>,
}

/// Pre-computed structure of the generated WIT document.
pub(crate) struct Plan<'a> {
    /// Index 0 is the default interface (may own no types, then skipped).
    pub interfaces: Vec<Iface<'a>>,
    /// TypeId string -> kebab-case WIT type name.
    pub type_names: HashMap<&'a str, String>,
    /// TypeId strings of structs that map to WIT resources.
    pub resources: HashSet<&'a str>,
    /// TypeId string -> index into `interfaces` of the defining interface.
    pub owner_of: HashMap<&'a str, usize>,
}

impl<'a> Plan<'a> {
    pub fn build(
        registry: &'a ValidatedRegistry<'a>,
        default_interface: &str,
    ) -> Result<Self, WitGenError> {
        let mut type_names = HashMap::new();
        let mut names = NameMap::new();
        for s in registry.structs() {
            type_names.insert(s.id.as_str(), names.insert(s.name)?);
        }
        for e in registry.enums() {
            type_names.insert(e.id.as_str(), names.insert(e.name)?);
        }
        for a in registry.type_aliases() {
            type_names.insert(a.id.as_str(), names.insert(a.name)?);
        }

        let resources = registry
            .structs()
            .iter()
            .filter(|s| {
                !(s.methods.is_empty() && s.constructors.is_empty() && s.properties.is_empty())
            })
            .map(|s| s.id.as_str())
            .collect();

        let mut interfaces = vec![Iface {
            name: to_kebab(default_interface),
            doc: None,
            functions: &[],
            constants: &[],
            type_ids: Vec::new(),
        }];
        let mut owner_of = HashMap::new();
        let mut iface_names = NameMap::new();
        iface_names.insert(default_interface)?;
        for module in registry.modules() {
            flatten_module(module, "", &mut interfaces, &mut owner_of, &mut iface_names)?;
        }

        // Types unclaimed by any module land in the default interface.
        let unclaimed: Vec<&'a str> = registry
            .structs()
            .iter()
            .map(|s| s.id.as_str())
            .chain(registry.enums().iter().map(|e| e.id.as_str()))
            .chain(registry.type_aliases().iter().map(|a| a.id.as_str()))
            .filter(|id| !owner_of.contains_key(id))
            .collect();
        for id in unclaimed {
            owner_of.insert(id, 0);
            interfaces[0].type_ids.push(id);
        }

        Ok(Self {
            interfaces,
            type_names,
            resources,
            owner_of,
        })
    }

    pub fn is_resource(&self, id: &str) -> bool {
        self.resources.contains(id)
    }

    pub fn type_name(&self, id: &str) -> &str {
        &self.type_names[id]
    }

    /// Foreign types referenced by interface `index`, grouped as
    /// owner-interface-name -> sorted type names, for `use` statements.
    pub fn uses_for(
        &self,
        registry: &ValidatedRegistry<'a>,
        index: usize,
    ) -> BTreeMap<String, Vec<String>> {
        let iface = &self.interfaces[index];
        let mut refs = HashSet::new();
        for f in iface.functions {
            collect_fn_refs(f, &mut refs);
        }
        for c in iface.constants {
            collect_refs(c.ty, &mut refs);
        }
        for id in &iface.type_ids {
            match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
                TypeKind::Struct(s) => {
                    for field in s.fields {
                        collect_refs(field.ty, &mut refs);
                    }
                    for prop in s.properties {
                        collect_refs(prop.ty, &mut refs);
                    }
                    for f in s.methods.iter().chain(s.constructors) {
                        collect_fn_refs(f, &mut refs);
                    }
                }
                TypeKind::Enum(e) => {
                    for v in e.variants {
                        match v.kind {
                            VariantKind::Unit => {}
                            VariantKind::Tuple(types) => {
                                for ty in types {
                                    collect_refs(ty, &mut refs);
                                }
                            }
                            VariantKind::Struct(fields) => {
                                for field in fields {
                                    collect_refs(field.ty, &mut refs);
                                }
                            }
                        }
                    }
                    for f in e.methods {
                        collect_fn_refs(f, &mut refs);
                    }
                }
                TypeKind::TypeAlias(a) => collect_refs(a.inner, &mut refs),
            }
        }

        let mut uses: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for id in refs {
            let owner = self.owner_of[id];
            if owner != index {
                uses.entry(self.interfaces[owner].name.clone())
                    .or_default()
                    .push(self.type_names[id].clone());
            }
        }
        for names in uses.values_mut() {
            names.sort();
        }
        uses
    }
}

fn flatten_module<'a>(
    module: &'a ModuleDescriptor<'a>,
    prefix: &str,
    interfaces: &mut Vec<Iface<'a>>,
    owner_of: &mut HashMap<&'a str, usize>,
    iface_names: &mut NameMap,
) -> Result<(), WitGenError> {
    let name = if prefix.is_empty() {
        iface_names.insert(module.name)?
    } else {
        // Flattened path: collision-check the joined name against all others.
        iface_names.insert(&format!("{prefix}-{}", module.name))?
    };
    let index = interfaces.len();
    let mut type_ids = Vec::new();
    for id in module.type_ids {
        // First module to claim a type defines it; later mentions become uses.
        if !owner_of.contains_key(id.as_str()) {
            owner_of.insert(id.as_str(), index);
            type_ids.push(id.as_str());
        }
    }
    interfaces.push(Iface {
        name: name.clone(),
        doc: module.doc,
        functions: module.functions,
        constants: module.constants,
        type_ids,
    });
    for sub in module.submodules {
        flatten_module(sub, &name, interfaces, owner_of, iface_names)?;
    }
    Ok(())
}

fn collect_fn_refs<'a>(f: &FunctionDescriptor<'a>, refs: &mut HashSet<&'a str>) {
    for param in f.params {
        collect_refs(param.ty, refs);
    }
    collect_refs(f.return_type, refs);
}

fn collect_refs<'a>(ty: &TypeDescriptor<'a>, refs: &mut HashSet<&'a str>) {
    match ty {
        TypeDescriptor::Ref(id) => {
            refs.insert(id.as_str());
        }
        TypeDescriptor::Option(inner)
        | TypeDescriptor::List(inner)
        | TypeDescriptor::Array(inner, _) => collect_refs(inner, refs),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            collect_refs(k, refs);
            collect_refs(v, refs);
        }
        TypeDescriptor::Tuple(elems) => {
            for elem in *elems {
                collect_refs(elem, refs);
            }
        }
        TypeDescriptor::Callback {
            params,
            return_type,
        } => {
            for param in *params {
                collect_refs(param, refs);
            }
            collect_refs(return_type, refs);
        }
        _ => {}
    }
}
