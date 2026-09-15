use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use haphe::{
    ConstantDescriptor, FunctionDescriptor, ModuleDescriptor, PrimitiveType, TypeDescriptor,
    TypeKind, ValidatedRegistry, VariantKind,
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
    /// TypeIds (as strings) of concrete types defined in this interface.
    pub type_ids: Vec<&'a str>,
    /// Indices into [`Plan::instances`] of generic instantiations defined in
    /// this interface.
    pub instance_indices: Vec<usize>,
}

/// A planned monomorphization of a generic type: the erased descriptor plus
/// concrete arguments, emitted under a deterministic mangled name.
pub(crate) struct PlannedInstance<'a> {
    pub erased_id: &'a str,
    pub args: &'a [TypeDescriptor<'a>],
    pub wit_name: String,
}

/// Substitution environment used when emitting one [`PlannedInstance`]:
/// generic parameter name -> concrete descriptor, plus the identity of the
/// instance being emitted (for self-references).
pub(crate) struct Env<'p, 'a> {
    pub bindings: Vec<(&'a str, &'p TypeDescriptor<'a>)>,
    pub self_id: &'a str,
    pub self_name: &'p str,
}

impl<'p, 'a> Env<'p, 'a> {
    pub fn lookup(&self, name: &str) -> Option<&'p TypeDescriptor<'a>> {
        self.bindings
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| *d)
    }
}

/// Pre-computed structure of the generated WIT document.
pub(crate) struct Plan<'a> {
    /// Index 0 is the default interface (may own no types, then skipped).
    pub interfaces: Vec<Iface<'a>>,
    /// TypeId string -> kebab-case WIT type name (concrete types only).
    pub type_names: HashMap<&'a str, String>,
    /// Erased TypeId strings of structs that map to WIT resources.
    pub resources: HashSet<&'a str>,
    /// TypeId string -> index into `interfaces` of the defining interface.
    pub owner_of: HashMap<&'a str, usize>,
    /// Deduplicated generic instantiations, in registry order.
    pub instances: Vec<PlannedInstance<'a>>,
    /// Erased TypeId strings of generic types (emitted only as instances).
    generics: HashSet<&'a str>,
    /// Erased TypeId string -> kebab of the generic type's name.
    erased_names: HashMap<&'a str, String>,
    /// Mangled names of all planned instances, for reference validation.
    instance_names: HashSet<String>,
}

impl<'a> Plan<'a> {
    pub fn build(
        registry: &'a ValidatedRegistry<'a>,
        default_interface: &str,
    ) -> Result<Self, WitGenError> {
        let mut generics = HashSet::new();
        let mut erased_names = HashMap::new();
        for s in registry.structs() {
            if !s.generic_params.is_empty() {
                generics.insert(s.id.as_str());
                erased_names.insert(s.id.as_str(), to_kebab(s.name));
            }
        }
        for e in registry.enums() {
            if !e.generic_params.is_empty() {
                generics.insert(e.id.as_str());
                erased_names.insert(e.id.as_str(), to_kebab(e.name));
            }
        }

        let mut type_names = HashMap::new();
        let mut names = NameMap::new();
        for s in registry.structs() {
            if !generics.contains(s.id.as_str()) {
                type_names.insert(s.id.as_str(), names.insert(s.name)?);
            }
        }
        for e in registry.enums() {
            if !generics.contains(e.id.as_str()) {
                type_names.insert(e.id.as_str(), names.insert(e.name)?);
            }
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
            instance_indices: Vec::new(),
        }];
        let mut owner_of = HashMap::new();
        let mut iface_names = NameMap::new();
        iface_names.insert(default_interface)?;
        for module in registry.modules() {
            flatten_module(
                module,
                "",
                &mut interfaces,
                &mut owner_of,
                &mut iface_names,
                &generics,
            )?;
        }

        // Concrete types unclaimed by any module land in the default
        // interface; generic types just get an owner for their instances.
        let unclaimed: Vec<(&'a str, bool)> = registry
            .structs()
            .iter()
            .map(|s| s.id.as_str())
            .chain(registry.enums().iter().map(|e| e.id.as_str()))
            .chain(registry.type_aliases().iter().map(|a| a.id.as_str()))
            .filter(|id| !owner_of.contains_key(id))
            .map(|id| (id, generics.contains(id)))
            .collect();
        for (id, is_generic) in unclaimed {
            owner_of.insert(id, 0);
            if !is_generic {
                interfaces[0].type_ids.push(id);
            }
        }

        let mut plan = Self {
            interfaces,
            type_names,
            resources,
            owner_of,
            instances: Vec::new(),
            generics,
            erased_names,
            instance_names: HashSet::new(),
        };

        // Plan one emission per distinct instantiation (identical duplicates
        // dedupe silently); mangled names share the type namespace.
        for inst in registry.instantiations() {
            let wit_name = plan.mangle_instance(inst.id.as_str(), inst.args, None)?;
            if plan.instance_names.contains(&wit_name) {
                continue;
            }
            names.insert(&wit_name)?;
            plan.instance_names.insert(wit_name.clone());
            let owner = plan.owner_of[inst.id.as_str()];
            plan.interfaces[owner]
                .instance_indices
                .push(plan.instances.len());
            plan.instances.push(PlannedInstance {
                erased_id: inst.id.as_str(),
                args: inst.args,
                wit_name,
            });
        }

        Ok(plan)
    }

    pub fn is_resource(&self, id: &str) -> bool {
        self.resources.contains(id)
    }

    pub fn is_generic(&self, id: &str) -> bool {
        self.generics.contains(id)
    }

    pub fn type_name(&self, id: &str) -> &str {
        &self.type_names[id]
    }

    /// Builds the substitution environment for a planned instance.
    pub fn env_for<'p>(
        &'p self,
        registry: &'a ValidatedRegistry<'a>,
        inst: &'p PlannedInstance<'a>,
    ) -> Env<'p, 'a> {
        let params = match registry
            .get_type(&haphe::TypeId::new(inst.erased_id))
            .unwrap()
        {
            TypeKind::Struct(s) => s.generic_params,
            TypeKind::Enum(e) => e.generic_params,
            TypeKind::TypeAlias(_) => &[],
        };
        Env {
            bindings: params
                .iter()
                .map(|p| p.name)
                .zip(inst.args.iter())
                .collect(),
            self_id: inst.erased_id,
            self_name: &inst.wit_name,
        }
    }

    /// Deterministic mangled WIT name for an instantiation of the generic
    /// type `erased_id` with `args` (documented in the crate README).
    pub fn mangle_instance(
        &self,
        erased_id: &str,
        args: &[TypeDescriptor<'a>],
        env: Option<&Env<'_, 'a>>,
    ) -> Result<String, WitGenError> {
        let base = self.erased_names.get(erased_id).cloned().ok_or_else(|| {
            WitGenError::UnrepresentableType {
                context: erased_id.to_string(),
                detail: "instantiation of a non-generic type".to_string(),
            }
        })?;
        let mut name = base;
        for arg in args {
            name.push('-');
            name.push_str(&self.mangle_type(arg, env)?);
        }
        Ok(name)
    }

    /// Mangled name fragment for one type argument.
    fn mangle_type(
        &self,
        ty: &TypeDescriptor<'a>,
        env: Option<&Env<'_, 'a>>,
    ) -> Result<String, WitGenError> {
        let err = |detail: &str| WitGenError::UnrepresentableType {
            context: "generic instantiation argument".to_string(),
            detail: detail.to_string(),
        };
        Ok(match ty {
            TypeDescriptor::Primitive(p) => match p {
                PrimitiveType::Bool => "bool".into(),
                PrimitiveType::I8 => "s8".into(),
                PrimitiveType::I16 => "s16".into(),
                PrimitiveType::I32 => "s32".into(),
                PrimitiveType::I64 => "s64".into(),
                PrimitiveType::U8 => "u8".into(),
                PrimitiveType::U16 => "u16".into(),
                PrimitiveType::U32 => "u32".into(),
                PrimitiveType::U64 => "u64".into(),
                PrimitiveType::F32 => "f32".into(),
                PrimitiveType::F64 => "f64".into(),
                PrimitiveType::Char => "char".into(),
                _ => return Err(err("unsupported primitive in instantiation")),
            },
            TypeDescriptor::String => "string".into(),
            TypeDescriptor::Bytes => "bytes".into(),
            TypeDescriptor::Unit => "unit".into(),
            TypeDescriptor::Option(t) => format!("option-{}", self.mangle_type(t, env)?),
            TypeDescriptor::List(t) | TypeDescriptor::Array(t, _) => {
                format!("list-{}", self.mangle_type(t, env)?)
            }
            TypeDescriptor::Map(k, v) => format!(
                "map-{}-{}",
                self.mangle_type(k, env)?,
                self.mangle_type(v, env)?
            ),
            TypeDescriptor::Tuple(es) => {
                let mut s = format!("tuple{}", es.len());
                for e in *es {
                    s.push('-');
                    s.push_str(&self.mangle_type(e, env)?);
                }
                s
            }
            TypeDescriptor::Result(o, e) => format!(
                "result-{}-{}",
                self.mangle_type(o, env)?,
                self.mangle_type(e, env)?
            ),
            TypeDescriptor::Ref(id) => self
                .type_names
                .get(id.as_str())
                .cloned()
                .ok_or_else(|| err("bare reference to a generic type"))?,
            TypeDescriptor::Instance { id, args } => {
                self.mangle_instance(id.as_str(), args, env)?
            }
            TypeDescriptor::GenericParam(name) => {
                let bound = env
                    .and_then(|e| e.lookup(name))
                    .ok_or_else(|| err("unbound generic parameter"))?;
                self.mangle_type(bound, None)?
            }
            _ => return Err(err("unsupported type in instantiation")),
        })
    }

    /// Deterministic marker text identifying an instance's origin, e.g.
    /// `labeled<string, s32>` — derived purely from descriptors so output is
    /// reproducible across builds.
    pub fn instance_marker(&self, inst: &PlannedInstance<'a>) -> Result<String, WitGenError> {
        let base = &self.erased_names[inst.erased_id];
        let args: Vec<String> = inst
            .args
            .iter()
            .map(|a| self.mangle_type(a, None))
            .collect::<Result<_, _>>()?;
        Ok(format!("{base}<{}>", args.join(", ")))
    }

    /// Resolves an `Instance` reference to its planned mangled name, erroring
    /// if the instantiation was never recorded in the registry.
    pub fn instance_ref_name(
        &self,
        id: &str,
        args: &[TypeDescriptor<'a>],
        env: Option<&Env<'_, 'a>>,
    ) -> Result<String, WitGenError> {
        let name = self.mangle_instance(id, args, env)?;
        if self.instance_names.contains(&name) {
            Ok(name)
        } else {
            Err(WitGenError::UnregisteredInstantiation { name })
        }
    }

    /// Foreign type names referenced by interface `index`, grouped as
    /// owner-interface-name -> sorted names, for `use` statements.
    pub fn uses_for(
        &self,
        registry: &'a ValidatedRegistry<'a>,
        index: usize,
    ) -> Result<BTreeMap<String, BTreeSet<String>>, WitGenError> {
        let iface = &self.interfaces[index];
        // (owner interface index, resolved WIT name)
        let mut refs: BTreeSet<(usize, String)> = BTreeSet::new();

        for f in iface.functions {
            self.collect_fn_uses(f, None, &mut refs)?;
        }
        for c in iface.constants {
            self.collect_uses(c.ty, None, &mut refs)?;
        }
        for id in &iface.type_ids {
            self.collect_type_uses(registry, id, None, &mut refs)?;
        }
        for &i in &iface.instance_indices {
            let inst = &self.instances[i];
            let env = self.env_for(registry, inst);
            self.collect_type_uses(registry, inst.erased_id, Some(&env), &mut refs)?;
        }

        let mut uses: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (owner, name) in refs {
            if owner != index {
                uses.entry(self.interfaces[owner].name.clone())
                    .or_default()
                    .insert(name);
            }
        }
        Ok(uses)
    }

    fn collect_type_uses(
        &self,
        registry: &'a ValidatedRegistry<'a>,
        id: &str,
        env: Option<&Env<'_, 'a>>,
        refs: &mut BTreeSet<(usize, String)>,
    ) -> Result<(), WitGenError> {
        match registry.get_type(&haphe::TypeId::new(id)).unwrap() {
            TypeKind::Struct(s) => {
                for field in s.fields {
                    self.collect_uses(field.ty, env, refs)?;
                }
                for prop in s.properties {
                    self.collect_uses(prop.ty, env, refs)?;
                }
                for f in s.methods.iter().chain(s.constructors) {
                    self.collect_fn_uses(f, env, refs)?;
                }
            }
            TypeKind::Enum(e) => {
                for v in e.variants {
                    match v.kind {
                        VariantKind::Unit => {}
                        VariantKind::Tuple(types) => {
                            for ty in types {
                                self.collect_uses(ty, env, refs)?;
                            }
                        }
                        VariantKind::Struct(fields) => {
                            for field in fields {
                                self.collect_uses(field.ty, env, refs)?;
                            }
                        }
                    }
                }
                for f in e.methods {
                    self.collect_fn_uses(f, env, refs)?;
                }
            }
            TypeKind::TypeAlias(a) => self.collect_uses(a.inner, env, refs)?,
        }
        Ok(())
    }

    fn collect_fn_uses(
        &self,
        f: &FunctionDescriptor<'a>,
        env: Option<&Env<'_, 'a>>,
        refs: &mut BTreeSet<(usize, String)>,
    ) -> Result<(), WitGenError> {
        for param in f.params {
            self.collect_uses(param.ty, env, refs)?;
        }
        self.collect_uses(f.return_type, env, refs)
    }

    fn collect_uses(
        &self,
        ty: &TypeDescriptor<'a>,
        env: Option<&Env<'_, 'a>>,
        refs: &mut BTreeSet<(usize, String)>,
    ) -> Result<(), WitGenError> {
        match ty {
            TypeDescriptor::Ref(id) => {
                // Bare refs to generic types are either self-references
                // (same interface, no `use` needed) or render-time errors.
                if !self.is_generic(id.as_str()) {
                    refs.insert((
                        self.owner_of[id.as_str()],
                        self.type_names[id.as_str()].clone(),
                    ));
                }
            }
            TypeDescriptor::Instance { id, args } => {
                let name = self.instance_ref_name(id.as_str(), args, env)?;
                refs.insert((self.owner_of[id.as_str()], name));
            }
            TypeDescriptor::GenericParam(name) => {
                if let Some(bound) = env.and_then(|e| e.lookup(name)) {
                    self.collect_uses(bound, None, refs)?;
                }
            }
            TypeDescriptor::Option(inner)
            | TypeDescriptor::List(inner)
            | TypeDescriptor::Array(inner, _) => self.collect_uses(inner, env, refs)?,
            TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
                self.collect_uses(k, env, refs)?;
                self.collect_uses(v, env, refs)?;
            }
            TypeDescriptor::Tuple(elems) => {
                for elem in *elems {
                    self.collect_uses(elem, env, refs)?;
                }
            }
            TypeDescriptor::Callback {
                params,
                return_type,
            } => {
                for param in *params {
                    self.collect_uses(param, env, refs)?;
                }
                self.collect_uses(return_type, env, refs)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten_module<'a>(
    module: &'a ModuleDescriptor<'a>,
    prefix: &str,
    interfaces: &mut Vec<Iface<'a>>,
    owner_of: &mut HashMap<&'a str, usize>,
    iface_names: &mut NameMap,
    generics: &HashSet<&'a str>,
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
            if !generics.contains(id.as_str()) {
                type_ids.push(id.as_str());
            }
        }
    }
    interfaces.push(Iface {
        name: name.clone(),
        doc: module.doc,
        functions: module.functions,
        constants: module.constants,
        type_ids,
        instance_indices: Vec::new(),
    });
    for sub in module.submodules {
        flatten_module(sub, &name, interfaces, owner_of, iface_names, generics)?;
    }
    Ok(())
}
