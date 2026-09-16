use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use haphe::{
    ConstantDescriptor, FnInstantiation, FunctionDescriptor, ModuleDescriptor, PrimitiveType,
    StructDescriptor, TraitImpl, TypeDescriptor, TypeKind, ValidatedRegistry, VariantKind,
};

use crate::WitGenError;
use crate::names::{NameMap, to_kebab};

/// Who owns an interface's function bodies.
///
/// How this maps to world `import`/`export` lines depends on the
/// [`WorldPerspective`](crate::WorldPerspective) the generator is configured
/// with — the haphe program may sit on either side of the component boundary.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// The haphe program provides the implementation (registered modules and
    /// types).
    Provided,
    /// The embedding counterpart supplies the implementation (foreign
    /// interfaces).
    Foreign,
}

/// One WIT interface: a flattened module, the default interface holding
/// types unclaimed by any module, or a foreign interface.
pub(crate) struct Iface<'a> {
    pub name: String,
    pub doc: Option<&'a str>,
    pub functions: &'a [FunctionDescriptor<'a>],
    /// Registry-level instantiations of this interface's generic functions
    /// (a flattened module's `function_instantiations`; empty elsewhere).
    /// Always consumed unioned with the fn-site declarations.
    pub fn_instantiations: &'a [FnInstantiation<'a>],
    pub constants: &'a [ConstantDescriptor<'a>],
    /// TypeIds (as strings) of concrete types defined in this interface.
    pub type_ids: Vec<&'a str>,
    /// Indices into [`Plan::instances`] of generic instantiations defined in
    /// this interface.
    pub instance_indices: Vec<usize>,
    /// How the world lists this interface.
    pub direction: Direction,
    /// For a monomorphized generic foreign interface (`generics` feature):
    /// index into [`Plan::instances`] of the instantiation it represents.
    pub foreign_instance: Option<usize>,
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
            fn_instantiations: &[],
            constants: &[],
            type_ids: Vec::new(),
            instance_indices: Vec::new(),
            direction: Direction::Provided,
            foreign_instance: None,
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

        // Foreign interfaces: the counterpart supplies the implementation.
        // Generic foreign interfaces and generic functions need the haphe
        // naming extension (`generics` feature); without it they are
        // rejected. With it, every emitted name derives purely from
        // descriptors, so output stays deterministic across builds.
        let mut foreign_generics: HashSet<&'a str> = HashSet::new();
        for fi in registry.foreign_interfaces() {
            if !fi.generic_params.is_empty() {
                if cfg!(feature = "generics") {
                    foreign_generics.insert(fi.id.as_str());
                    erased_names.insert(fi.id.as_str(), to_kebab(fi.name));
                    continue;
                }
                return Err(WitGenError::GenericForeignInterface {
                    name: fi.name.to_string(),
                });
            }
            interfaces.push(Iface {
                name: iface_names.insert(fi.name)?,
                doc: fi.doc,
                functions: fi.functions,
                fn_instantiations: &[],
                constants: &[],
                type_ids: Vec::new(),
                instance_indices: Vec::new(),
                direction: Direction::Foreign,
                foreign_instance: None,
            });
        }
        if !cfg!(feature = "generics") {
            for iface in &interfaces {
                for f in iface.functions {
                    if !f.generic_params.is_empty() {
                        return Err(WitGenError::GenericFunction {
                            interface: iface.name.clone(),
                            function: f.name.to_string(),
                        });
                    }
                }
            }
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
        // dedupe silently); mangled names share the type namespace. An
        // instantiation of a generic foreign interface (`generics` feature)
        // becomes its own interface under the mangled name.
        for inst in registry.instantiations() {
            let wit_name = plan.mangle_instance(inst.id.as_str(), inst.args, None)?;
            if plan.instance_names.contains(&wit_name) {
                continue;
            }
            if foreign_generics.contains(inst.id.as_str()) {
                let fi = registry
                    .get_foreign_interface(&haphe::TypeId::new(inst.id.as_str()))
                    .expect("id came from foreign_interfaces");
                iface_names.insert(&wit_name)?;
                plan.instance_names.insert(wit_name.clone());
                let instance_index = plan.instances.len();
                plan.instances.push(PlannedInstance {
                    erased_id: inst.id.as_str(),
                    args: inst.args,
                    wit_name: wit_name.clone(),
                });
                plan.interfaces.push(Iface {
                    name: wit_name,
                    doc: fi.doc,
                    functions: fi.functions,
                    fn_instantiations: &[],
                    constants: &[],
                    type_ids: Vec::new(),
                    instance_indices: Vec::new(),
                    direction: Direction::Foreign,
                    foreign_instance: Some(instance_index),
                });
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
        let id = haphe::TypeId::new(inst.erased_id);
        let params = if let Some(fi) = registry.get_foreign_interface(&id) {
            fi.generic_params
        } else {
            match registry.get_type(&id).unwrap() {
                TypeKind::Struct(s) => s.generic_params,
                TypeKind::Enum(e) => e.generic_params,
                TypeKind::TypeAlias(_) => &[],
            }
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

    /// Extends `base` with a generic function's own parameter bindings for
    /// one instantiation.
    pub fn fn_env<'p>(
        &'p self,
        f: &'a FunctionDescriptor<'a>,
        args: &'a [TypeDescriptor<'a>],
        base: Option<&Env<'p, 'a>>,
    ) -> Env<'p, 'a> {
        let mut bindings: Vec<(&'a str, &'p TypeDescriptor<'a>)> =
            base.map(|e| e.bindings.clone()).unwrap_or_default();
        bindings.extend(f.generic_params.iter().map(|p| p.name).zip(args.iter()));
        Env {
            bindings,
            self_id: base.map(|e| e.self_id).unwrap_or(""),
            self_name: base.map(|e| e.self_name).unwrap_or(""),
        }
    }

    /// Deterministic mangled WIT name for one instantiation of a generic
    /// function: `{kebab(fn)}-{mangled args}` (same argument mangling as
    /// generic type instances).
    pub fn mangle_fn_instance(
        &self,
        fn_name: &str,
        args: &[TypeDescriptor<'a>],
        env: Option<&Env<'_, 'a>>,
    ) -> Result<String, WitGenError> {
        let mut name = to_kebab(fn_name);
        for arg in args {
            name.push('-');
            name.push_str(&self.mangle_type(arg, env)?);
        }
        Ok(name)
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
    pub(crate) fn mangle_type(
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
            TypeDescriptor::List(t) => format!("list-{}", self.mangle_type(t, env)?),
            TypeDescriptor::Array(t, n) => format!("list{n}-{}", self.mangle_type(t, env)?),
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
            TypeDescriptor::Stream(inner) => match inner {
                TypeDescriptor::Unit => "stream".to_string(),
                _ => format!("stream-{}", self.mangle_type(inner, env)?),
            },
            TypeDescriptor::Future(inner) => match inner {
                TypeDescriptor::Unit => "future".to_string(),
                _ => format!("future-{}", self.mangle_type(inner, env)?),
            },
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

        let base_env = iface
            .foreign_instance
            .map(|i| self.env_for(registry, &self.instances[i]));
        for f in iface.functions {
            if f.generic_params.is_empty() {
                self.collect_fn_uses(f, base_env.as_ref(), &mut refs)?;
            } else {
                for args in haphe::union_instantiations(f, iface.fn_instantiations) {
                    let env = self.fn_env(f, args, base_env.as_ref());
                    self.collect_fn_uses(f, Some(&env), &mut refs)?;
                }
            }
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
            | TypeDescriptor::Array(inner, _)
            | TypeDescriptor::Stream(inner)
            | TypeDescriptor::Future(inner) => self.collect_uses(inner, env, refs)?,
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

/// Plan-independent mangled fragment for a type argument: the subset of
/// [`Plan::mangle_type`] that never touches registered type names. Used by
/// the runtime foreign caller, which has no plan; `Ref`, `Instance`, and
/// `GenericParam` arguments error. KEEP IN SYNC with [`Plan::mangle_type`].
#[cfg_attr(not(feature = "runtime"), allow(dead_code))]
pub(crate) fn mangle_plain_type(ty: &TypeDescriptor<'_>) -> Result<String, WitGenError> {
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
        TypeDescriptor::Option(t) => format!("option-{}", mangle_plain_type(t)?),
        TypeDescriptor::List(t) => format!("list-{}", mangle_plain_type(t)?),
        TypeDescriptor::Array(t, n) => format!("list{n}-{}", mangle_plain_type(t)?),
        TypeDescriptor::Map(k, v) => {
            format!("map-{}-{}", mangle_plain_type(k)?, mangle_plain_type(v)?)
        }
        TypeDescriptor::Tuple(es) => {
            let mut s = format!("tuple{}", es.len());
            for e in *es {
                s.push('-');
                s.push_str(&mangle_plain_type(e)?);
            }
            s
        }
        TypeDescriptor::Result(o, e) => {
            format!("result-{}-{}", mangle_plain_type(o)?, mangle_plain_type(e)?)
        }
        TypeDescriptor::Stream(inner) => match inner {
            TypeDescriptor::Unit => "stream".to_string(),
            _ => format!("stream-{}", mangle_plain_type(inner)?),
        },
        TypeDescriptor::Future(inner) => match inner {
            TypeDescriptor::Unit => "future".to_string(),
            _ => format!("future-{}", mangle_plain_type(inner)?),
        },
        _ => {
            return Err(err(
                "type arguments referencing registered types are not supported at runtime",
            ));
        }
    })
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
        fn_instantiations: module.function_instantiations,
        constants: module.constants,
        type_ids,
        instance_indices: Vec::new(),
        direction: Direction::Provided,
        foreign_instance: None,
    });
    for sub in module.submodules {
        flatten_module(sub, &name, interfaces, owner_of, iface_names, generics)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Trait projection (interusability): every declared TraitImpl surfaces as a
// WIT-native named function. Single source of truth consumed by both the
// emitter and the live runtime dispatch.
// ---------------------------------------------------------------------------

/// One projected trait member's dispatch shape.
pub(crate) enum ProjKind<'a> {
    /// Binary operator whose rhs is the type itself (`meta_arith_self`).
    /// `op` keys the runtime dispatch table (`runtime` feature).
    ArithSelf {
        #[cfg_attr(not(feature = "runtime"), allow(dead_code))]
        op: &'static str,
        output: &'a TypeDescriptor<'a>,
    },
    /// Binary operator with a non-self rhs (`meta_arith_scalar`).
    ArithScalar {
        #[cfg_attr(not(feature = "runtime"), allow(dead_code))]
        op: &'static str,
        rhs: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    /// Unary negation (`meta_unm`).
    Neg {
        output: &'a TypeDescriptor<'a>,
    },
    /// Bitwise not (`meta_bnot`).
    BNot {
        output: &'a TypeDescriptor<'a>,
    },
    Eq,
    Lt,
    Le,
    /// `to-string` (Display/ToString, deduplicated).
    ToString,
    /// `to-debug-string` (Debug).
    DebugString,
    /// `hash: func() -> u64`.
    Hash,
    /// `call` (`meta_call` / `meta_call_async`).
    Call {
        args: &'a [TypeDescriptor<'a>],
        output: &'a TypeDescriptor<'a>,
        is_async: bool,
    },
    /// `at` (`meta_index`).
    IndexGet {
        index: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    /// `set-at` (`meta_newindex`). For records (value semantics) the
    /// projection returns the updated record.
    IndexSet {
        index: &'a TypeDescriptor<'a>,
        output: &'a TypeDescriptor<'a>,
    },
    /// `items: func() -> list<ITEM>` — an EAGER SNAPSHOT of the iteration
    /// (`meta_iter`); lazy iterators are not WIT-native.
    Items {
        item: &'a TypeDescriptor<'a>,
    },
    /// `length: func() -> u64` (`meta_len`).
    Length,
    /// `default: static func() -> T` (`Default`, through the constructor
    /// channel; never the `constructor(...)` slot).
    Default,
}

/// One projected trait member: `source_name` runs through the same
/// kebab-cased [`NameMap`] as user members, so collisions with user-declared
/// names are descriptive generation errors.
pub(crate) struct Projected<'a> {
    pub source_name: &'static str,
    pub kind: ProjKind<'a>,
}

/// Whether a trait operand descriptor denotes the type itself.
fn is_self_ty(ty: &TypeDescriptor<'_>, self_id: &str) -> bool {
    match ty {
        TypeDescriptor::Ref(id) => id.as_str() == self_id,
        TypeDescriptor::Instance { id, .. } => id.as_str() == self_id,
        _ => false,
    }
}

/// The deterministic projection of a struct's declared trait impls into
/// WIT members (documented in the crate README). Errors on shapes that
/// cannot project uniquely: multiple overloads of one operator, or both
/// `Call` and `AsyncCall` (one `call` member can exist).
pub(crate) fn projected_trait_members<'a>(
    s: &StructDescriptor<'a>,
) -> Result<Vec<Projected<'a>>, WitGenError> {
    let mut out: Vec<Projected<'a>> = Vec::new();
    let mut seen: HashSet<&'static str> = HashSet::new();
    let mut push = |name: &'static str, kind: ProjKind<'a>| -> Result<(), WitGenError> {
        if !seen.insert(name) {
            return Err(WitGenError::UnrepresentableType {
                context: s.name.to_string(),
                detail: format!(
                    "declares multiple trait impls projecting to WIT member `{name}`; \
                     expose a named method instead"
                ),
            });
        }
        out.push(Projected {
            source_name: name,
            kind,
        });
        Ok(())
    };

    let self_id = s.id.as_str();
    let mut has_eq = false;
    let mut has_ord = false;
    let mut has_to_string = false;
    let mut has_iter = false;
    let mut has_call = false;
    for ti in s.trait_impls {
        match ti {
            TraitImpl::Add { rhs, output }
            | TraitImpl::Sub { rhs, output }
            | TraitImpl::Mul { rhs, output }
            | TraitImpl::Div { rhs, output }
            | TraitImpl::Rem { rhs, output }
            | TraitImpl::IDiv { rhs, output }
            | TraitImpl::Mod { rhs, output }
            | TraitImpl::Pow { rhs, output }
            | TraitImpl::BitAnd { rhs, output }
            | TraitImpl::BitOr { rhs, output }
            | TraitImpl::BitXor { rhs, output }
            | TraitImpl::Shl { rhs, output }
            | TraitImpl::Shr { rhs, output } => {
                let op: &'static str = match ti {
                    TraitImpl::Add { .. } => "add",
                    TraitImpl::Sub { .. } => "sub",
                    TraitImpl::Mul { .. } => "mul",
                    TraitImpl::Div { .. } => "div",
                    TraitImpl::Rem { .. } => "rem",
                    TraitImpl::IDiv { .. } => "idiv",
                    TraitImpl::Mod { .. } => "mod",
                    TraitImpl::Pow { .. } => "pow",
                    TraitImpl::BitAnd { .. } => "bitand",
                    TraitImpl::BitOr { .. } => "bitor",
                    TraitImpl::BitXor { .. } => "bitxor",
                    TraitImpl::Shl { .. } => "shl",
                    TraitImpl::Shr { .. } => "shr",
                    _ => unreachable!(),
                };
                if is_self_ty(rhs, self_id) {
                    push(op, ProjKind::ArithSelf { op, output })?;
                } else {
                    push(op, ProjKind::ArithScalar { op, rhs, output })?;
                }
            }
            TraitImpl::Neg { output } => push("neg", ProjKind::Neg { output })?,
            TraitImpl::Not { output } => push("not", ProjKind::BNot { output })?,
            TraitImpl::PartialEq | TraitImpl::Eq => {
                if !has_eq {
                    has_eq = true;
                    push("eq", ProjKind::Eq)?;
                }
            }
            TraitImpl::PartialOrd | TraitImpl::Ord => {
                if !has_ord {
                    has_ord = true;
                    push("lt", ProjKind::Lt)?;
                    push("le", ProjKind::Le)?;
                }
            }
            TraitImpl::Display | TraitImpl::ToString => {
                if !has_to_string {
                    has_to_string = true;
                    push("to_string", ProjKind::ToString)?;
                }
            }
            TraitImpl::Debug => push("to_debug_string", ProjKind::DebugString)?,
            TraitImpl::Hash => push("hash", ProjKind::Hash)?,
            TraitImpl::Call { args, output } | TraitImpl::AsyncCall { args, output } => {
                if has_call {
                    return Err(WitGenError::UnrepresentableType {
                        context: s.name.to_string(),
                        detail: "declares both `Call` and `AsyncCall`; only one `call` \
                                 projection can exist"
                            .to_string(),
                    });
                }
                has_call = true;
                push(
                    "call",
                    ProjKind::Call {
                        args,
                        output,
                        is_async: matches!(ti, TraitImpl::AsyncCall { .. }),
                    },
                )?;
            }
            TraitImpl::Index { index, output } => push("at", ProjKind::IndexGet { index, output })?,
            TraitImpl::IndexMut { index, output } => {
                push("set_at", ProjKind::IndexSet { index, output })?
            }
            TraitImpl::Iterator { item } | TraitImpl::IntoIterator { item } => {
                if !has_iter {
                    has_iter = true;
                    push("items", ProjKind::Items { item })?;
                    push("length", ProjKind::Length)?;
                }
            }
            TraitImpl::Default => push("default", ProjKind::Default)?,
            // `Clone` is deliberately unprojected: handles give guests
            // sharing, and value types copy structurally.
            TraitImpl::Clone => {}
            _ => {
                return Err(WitGenError::UnrepresentableType {
                    context: s.name.to_string(),
                    detail: format!("trait impl {ti:?} has no WIT projection"),
                });
            }
        }
    }
    Ok(out)
}
