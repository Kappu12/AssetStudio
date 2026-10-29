//! Unity field selection over owned, permissively parsed CLR metadata.
//!
//! Serialization policy and templates follow the MIT AssetStudio/AssetsTools
//! research credited in managed_templates.rs. No method bodies are executed.

use anyhow::{Context, Result, bail, ensure};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use unity_rs_core::unity_version::UnityVersion;

use super::managed_metadata::{ManagedAssembly, ManagedField, ManagedFieldType as Ty, ManagedType};
use super::managed_templates::*;

#[derive(Clone)]
struct Resolved {
    assembly: Arc<ManagedAssembly>,
    index: usize,
    args: Vec<Ty>,
}
impl Resolved {
    fn ty(&self) -> &ManagedType {
        &self.assembly.types[self.index]
    }
}

pub(super) struct Serializer {
    index: HashMap<String, PathBuf>,
    loaded: HashMap<String, Arc<ManagedAssembly>>,
    bytes: usize,
    visits: usize,
    tree_work: usize,
    tree_string_work: usize,
    retained_model_bytes: usize,
    using_managed_reference: bool,
}

impl Serializer {
    pub(super) fn new(index: HashMap<String, PathBuf>) -> Self {
        Self {
            index,
            loaded: HashMap::new(),
            bytes: 0,
            visits: 0,
            tree_work: 0,
            tree_string_work: 0,
            retained_model_bytes: 0,
            using_managed_reference: false,
        }
    }

    pub(super) fn generate(
        &mut self,
        assembly: &str,
        namespace: &str,
        class: &str,
        version: &UnityVersion,
    ) -> Result<Vec<TemplateField>> {
        self.using_managed_reference = false;
        let name = if namespace.is_empty() {
            class.to_owned()
        } else {
            format!("{namespace}.{class}")
        };
        let resolved = self.resolve(&format!("{assembly}::{name}"), assembly, Vec::new(), 0)?;
        let limit = if version.major > 2020
            || (version.major == 2020
                && (version.minor > 1 || (version.minor == 1 && version.patch >= 4)))
            || (version.major == 2019 && version.minor == 4 && version.patch >= 9)
        {
            10
        } else {
            7
        };
        let mut fields = self.walk(&resolved, version, limit, 0)?;
        if self.using_managed_reference {
            let registry = managed_references_registry("references", version);
            self.charge_tree(&registry)?;
            fields.push(registry);
        }
        Ok(fields)
    }

    fn load(&mut self, name: &str) -> Result<Arc<ManagedAssembly>> {
        let normalized = super::assembly_name(name)?.to_ascii_lowercase();
        if let Some(assembly) = self.loaded.get(&normalized) {
            return Ok(Arc::clone(assembly));
        }
        let path = self.index.get(&normalized).with_context(|| {
            format!("Missing managed assembly {name:?}; provide all matching dependency DLLs")
        })?;
        let remaining = super::MAX_TOTAL_DLL_BYTES
            .checked_sub(self.bytes)
            .context("Managed assembly input budget exhausted")?;
        let bytes = super::read_file(path, remaining.min(super::MAX_DLL_BYTES))?;
        let assembly = super::managed_metadata::parse_assembly(&bytes)
            .with_context(|| format!("Invalid managed metadata in {path:?}"))?;
        ensure!(
            super::assembly_name(&assembly.name)?.to_ascii_lowercase() == normalized,
            "Managed DLL manifest identity does not match the requested assembly {name:?}"
        );
        ensure!(
            assembly.references.len() <= super::MAX_DLL_FILES,
            "Managed assembly has too many dependency identities"
        );
        for dependency in &assembly.references {
            super::assembly_name(dependency)?;
        }
        let retained = self
            .retained_model_bytes
            .checked_add(model_storage(&assembly)?)
            .context("Managed model storage count overflow")?;
        ensure!(
            retained <= super::MAX_TOTAL_DLL_BYTES,
            "Managed metadata cache exceeds its retained model byte limit"
        );
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .context("Managed assembly input size overflow")?;
        let assembly = Arc::new(assembly);
        self.loaded.try_reserve(1)?;
        self.loaded.insert(normalized, Arc::clone(&assembly));
        self.retained_model_bytes = retained;
        Ok(assembly)
    }

    fn resolve(
        &mut self,
        name: &str,
        current: &str,
        args: Vec<Ty>,
        hops: usize,
    ) -> Result<Resolved> {
        ensure!(
            hops < 64,
            "Managed type forwarding chain is cyclic or too deep"
        );
        let (assembly_name, full) = name.split_once("::").unwrap_or((current, name));
        let assembly = self.load(assembly_name)?;
        let normalized = full.replace('/', "+");
        if let Some(index) = assembly
            .types
            .iter()
            .position(|ty| ty.full_name == normalized)
        {
            let ty = &assembly.types[index];
            ensure!(
                ty.custom_attributes.len() <= 65_536,
                "Managed type has too many custom attributes"
            );
            if ty.declaring_type.is_none() {
                let full = if ty.namespace.is_empty() {
                    ty.name.clone()
                } else {
                    format!("{}.{}", ty.namespace, ty.name)
                };
                ensure!(
                    full == ty.full_name,
                    "Managed type namespace/name identity is inconsistent"
                );
            }
            return Ok(Resolved {
                assembly,
                index,
                args,
            });
        }
        if let Some(target) = assembly.type_forwarders.get(&normalized) {
            return self.resolve(target, assembly_name, args, hops + 1);
        }
        bail!("Managed type {full:?} is missing from assembly {assembly_name:?}")
    }

    fn resolved_type(&mut self, ty: &Ty, current: &str) -> Result<Option<Resolved>> {
        Ok(match ty {
            Ty::Class(name) | Ty::ValueType(name) => {
                Some(self.resolve(name, current, Vec::new(), 0)?)
            }
            Ty::GenericInst { name, args, .. } => {
                Some(self.resolve(name, current, args.clone(), 0)?)
            }
            _ => None,
        })
    }

    fn walk(
        &mut self,
        resolved: &Resolved,
        version: &UnityVersion,
        depth: i32,
        expansion: usize,
    ) -> Result<Vec<TemplateField>> {
        ensure!(
            expansion < 128,
            "Managed type expansion is cyclic or too deep"
        );
        let ty = resolved.ty();
        let mut fields = Vec::new();
        if let Some(base) = &ty.base_type {
            let base = substitute(base, &resolved.args, 0)?;
            if !is_base_stop(&base) {
                let parent = self
                    .resolved_type(&base, &resolved.assembly.name)?
                    .context("Managed base type is not a named type")?;
                fields = self.walk(&parent, version, depth, expansion + 1)?;
            }
        }
        for field in &ty.fields {
            self.visits = self
                .visits
                .checked_add(1)
                .context("Managed field traversal overflow")?;
            ensure!(
                self.visits <= super::MAX_TOTAL_NODES,
                "Managed field traversal exceeds its limit"
            );
            if !serializable_field(field) {
                continue;
            }
            let sig = substitute(&field.signature, &resolved.args, 0)?;
            let (element, collection) = match &sig {
                Ty::SzArray(inner) => (inner.as_ref(), true),
                Ty::GenericInst { name, args, .. }
                    if simple_name(name) == "System.Collections.Generic.List`1" =>
                {
                    ensure!(
                        args.len() == 1,
                        "Managed List signature has the wrong generic arity"
                    );
                    (&args[0], true)
                }
                Ty::Array { .. } => continue,
                _ => (&sig, false),
            };
            if collection && (depth < 0 || is_collection(element)) {
                continue;
            }
            if !collection
                && named(element).is_some_and(|name| simple_name(name) == ty.full_name)
                && !self.derives_unity_object(resolved, 0)?
            {
                continue;
            }
            let Some((mut node, plain)) = self
                .field(
                    element,
                    field,
                    &resolved.assembly.name,
                    version,
                    depth,
                    expansion,
                )
                .with_context(|| format!("Read managed field {}.{:?}", ty.full_name, field.name))?
            else {
                continue;
            };
            node.name = field.name.clone();
            if collection {
                node = if plain {
                    vector(node)
                } else {
                    vector_with_type(node)
                };
            }
            self.charge_tree(&node)?;
            fields.try_reserve(1)?;
            fields.push(node);
        }
        Ok(fields)
    }

    // This is a generated-work budget, not only a final output count. Nested
    // child trees may be charged again when a parent retains them, deliberately
    // bounding template expansion and string copies before flattening.
    fn charge_tree(&mut self, root: &TemplateField) -> Result<()> {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            self.tree_work = self
                .tree_work
                .checked_add(1)
                .context("Generated tree work count overflow")?;
            ensure!(
                self.tree_work <= super::MAX_TOTAL_NODES,
                "Generated tree work exceeds its node limit"
            );
            self.tree_string_work = self
                .tree_string_work
                .checked_add(node.name.len())
                .and_then(|value| value.checked_add(node.ty.len()))
                .context("Generated tree work string count overflow")?;
            ensure!(
                self.tree_string_work <= super::MAX_SCHEMA_STRING_BYTES,
                "Generated tree work exceeds its string byte limit"
            );
            let pending = self
                .tree_work
                .checked_add(stack.len())
                .and_then(|count| count.checked_add(node.children.len()))
                .context("Generated tree pending work count overflow")?;
            ensure!(
                pending <= super::MAX_TOTAL_NODES,
                "Generated tree work exceeds its pending node limit"
            );
            stack.try_reserve(node.children.len())?;
            stack.extend(node.children.iter());
        }
        Ok(())
    }

    fn field(
        &mut self,
        sig: &Ty,
        field: &ManagedField,
        current: &str,
        version: &UnityVersion,
        depth: i32,
        expansion: usize,
    ) -> Result<Option<(TemplateField, bool)>> {
        if let Some(primitive) = primitive(sig) {
            return Ok(Some((
                leaf(primitive, type_aligns_by_name(primitive), Vec::new()),
                true,
            )));
        }
        if matches!(sig, Ty::String) {
            return Ok(Some((leaf("string", false, string_children()), true)));
        }
        if matches!(
            sig,
            Ty::Array { .. } | Ty::Pointer(_) | Ty::ByRef(_) | Ty::Void | Ty::MVar(_)
        ) {
            return Ok(None);
        }
        if matches!(sig, Ty::Var(_)) {
            bail!(
                "Managed field {:?} has an unresolved generic parameter",
                field.name
            );
        }
        let managed_ref = has_attr(field, "UnityEngine.SerializeReference");
        if matches!(sig, Ty::Object) {
            return if managed_ref {
                self.using_managed_reference = true;
                Ok(Some((
                    leaf(
                        "managedReference",
                        false,
                        managed_reference_children(version),
                    ),
                    false,
                )))
            } else {
                Ok(None)
            };
        }
        let Some(name) = named(sig) else {
            return Ok(None);
        };
        if let Some(primitive) = convert_base_to_primitive(simple_name(name)) {
            return if primitive == "string" {
                Ok(Some((leaf("string", false, string_children()), true)))
            } else {
                Ok(Some((
                    leaf(primitive, type_aligns_by_name(primitive), Vec::new()),
                    true,
                )))
            };
        }
        // Intrinsic system types are not Unity serializable; List was unwrapped
        // above. This also avoids requiring runtime method-only BCL metadata.
        if simple_name(name).starts_with("System.") {
            return Ok(None);
        }
        let rt = self
            .resolved_type(sig, current)?
            .context("Managed serialized field type is not named")?;
        if !rt.ty().generic_parameters.is_empty() && version.major < 2020 {
            return Ok(None);
        }
        if rt
            .ty()
            .base_type
            .as_ref()
            .and_then(named)
            .is_some_and(|name| simple_name(name) == "System.Enum")
        {
            let underlying = rt
                .ty()
                .fields
                .iter()
                .find(|field| field.name == "value__")
                .context("Managed enum has no underlying field")?;
            if matches!(underlying.signature, Ty::I64 | Ty::U64) {
                return Ok(None);
            }
            let primitive = primitive(&underlying.signature)
                .context("Managed enum has an invalid underlying type")?;
            return Ok(Some((
                leaf(primitive, type_aligns_by_name(primitive), Vec::new()),
                true,
            )));
        }
        let unity_object = self.derives_unity_object(&rt, 0)?;
        if unity_object {
            return Ok(Some((
                leaf(
                    &format!("PPtr<${}>", rt.ty().name),
                    false,
                    pptr_children(version),
                ),
                true,
            )));
        }
        let value_type = matches!(
            sig,
            Ty::ValueType(_)
                | Ty::GenericInst {
                    value_type: true,
                    ..
                }
        ) || rt
            .ty()
            .base_type
            .as_ref()
            .and_then(named)
            .is_some_and(|name| matches!(simple_name(name), "System.ValueType" | "System.Enum"));
        if managed_ref {
            if value_type || !rt.ty().generic_parameters.is_empty() {
                return Ok(None);
            }
            self.using_managed_reference = true;
            return Ok(Some((
                leaf(
                    "managedReference",
                    false,
                    managed_reference_children(version),
                ),
                false,
            )));
        }
        let full = rt.ty().full_name.as_str();
        let special = is_special_unity_type(full);
        let serializable = rt.ty().flags & 0x2000 != 0;
        if depth < 0 && !(value_type && (serializable || special)) {
            return Ok(None);
        }
        if !special
            && (rt.ty().flags & 0x80 != 0
                || rt.ty().flags & 0x20 != 0
                || !serializable
                || is_assembly_blacklisted(&rt.assembly.name))
        {
            return Ok(None);
        }
        let children = match special_unity_children(&rt.ty().name, version).filter(|_| special) {
            Some(children) => children,
            None => self.walk(&rt, version, depth - 1, expansion + 1)?,
        };
        Ok(Some((leaf(&rt.ty().name, false, children), false)))
    }

    fn derives_unity_object(&mut self, resolved: &Resolved, hops: usize) -> Result<bool> {
        ensure!(
            hops < 128,
            "Managed inheritance chain is cyclic or too deep"
        );
        if resolved.ty().full_name == "UnityEngine.Object" {
            return Ok(true);
        }
        let Some(base) = &resolved.ty().base_type else {
            return Ok(false);
        };
        let base = substitute(base, &resolved.args, 0)?;
        if named(&base).is_some_and(|name| simple_name(name) == "UnityEngine.Object") {
            return Ok(true);
        }
        if is_intrinsic_base(&base) {
            return Ok(false);
        }
        let parent = self
            .resolved_type(&base, &resolved.assembly.name)?
            .context("Managed base type is not named")?;
        self.derives_unity_object(&parent, hops + 1)
    }
}

// Conservative retained-storage accounting covers owned metadata buffers,
// string capacity, nested signatures and per-allocation overhead. It is shared
// across cached assemblies, independently of their encoded file-byte budget.
fn model_storage(model: &ManagedAssembly) -> Result<usize> {
    use std::mem::size_of;
    fn add(total: &mut usize, bytes: usize) -> Result<()> {
        *total = total
            .checked_add(bytes)
            .context("Managed model storage overflow")?;
        ensure!(
            *total <= super::MAX_TOTAL_DLL_BYTES,
            "Managed model exceeds its retained model byte limit"
        );
        Ok(())
    }
    fn buffer<T>(total: &mut usize, values: &Vec<T>) -> Result<()> {
        add(
            total,
            values
                .capacity()
                .checked_mul(size_of::<T>())
                .context("Managed model vector storage overflow")?,
        )?;
        if values.capacity() != 0 {
            add(total, 32)?;
        }
        Ok(())
    }
    fn string(total: &mut usize, value: &String) -> Result<()> {
        add(total, value.capacity())?;
        if value.capacity() != 0 {
            add(total, 32)?;
        }
        Ok(())
    }
    fn strings(total: &mut usize, values: &Vec<String>) -> Result<()> {
        buffer(total, values)?;
        for value in values {
            string(total, value)?;
        }
        Ok(())
    }
    fn signature(total: &mut usize, root: &Ty) -> Result<()> {
        let mut stack = vec![root];
        let mut nodes = 0_usize;
        while let Some(ty) = stack.pop() {
            nodes = nodes
                .checked_add(1)
                .context("Managed signature storage-node overflow")?;
            ensure!(
                nodes <= 500_000,
                "Managed signature storage traversal exceeds its limit"
            );
            match ty {
                Ty::Class(name) | Ty::ValueType(name) => string(total, name)?,
                Ty::SzArray(inner)
                | Ty::ByRef(inner)
                | Ty::Pointer(inner)
                | Ty::Array { element: inner, .. } => {
                    add(total, size_of::<Ty>() + 32)?;
                    stack.try_reserve(1)?;
                    stack.push(inner);
                }
                Ty::GenericInst { name, args, .. } => {
                    string(total, name)?;
                    buffer(total, args)?;
                    stack.try_reserve(args.len())?;
                    stack.extend(args.iter());
                }
                _ => {}
            }
        }
        Ok(())
    }
    let mut total = size_of::<ManagedAssembly>();
    string(&mut total, &model.name)?;
    strings(&mut total, &model.references)?;
    buffer(&mut total, &model.types)?;
    for ty in &model.types {
        for value in [&ty.namespace, &ty.name, &ty.full_name] {
            string(&mut total, value)?;
        }
        if let Some(declaring) = &ty.declaring_type {
            string(&mut total, declaring)?;
        }
        if let Some(base) = &ty.base_type {
            signature(&mut total, base)?;
        }
        strings(&mut total, &ty.generic_parameters)?;
        strings(&mut total, &ty.custom_attributes)?;
        buffer(&mut total, &ty.fields)?;
        for field in &ty.fields {
            string(&mut total, &field.name)?;
            strings(&mut total, &field.custom_attributes)?;
            signature(&mut total, &field.signature)?;
        }
    }
    let buckets = model
        .type_forwarders
        .capacity()
        .checked_mul(2)
        .context("Managed forwarder storage overflow")?;
    add(
        &mut total,
        buckets
            .checked_mul(size_of::<(String, String)>() + 1)
            .context("Managed forwarder storage overflow")?,
    )?;
    for (name, target) in &model.type_forwarders {
        string(&mut total, name)?;
        string(&mut total, target)?;
    }
    Ok(total)
}

fn leaf(ty: &str, aligned: bool, children: Vec<TemplateField>) -> TemplateField {
    TemplateField {
        name: String::new(),
        ty: ty.into(),
        aligned,
        children,
    }
}
fn serializable_field(field: &ManagedField) -> bool {
    (field.flags & 7 == 6
        || has_attr(field, "UnityEngine.SerializeField")
        || has_attr(field, "UnityEngine.SerializeReference"))
        && field.flags & (0x10 | 0x20 | 0x40 | 0x80) == 0
}
fn has_attr(field: &ManagedField, name: &str) -> bool {
    field
        .custom_attributes
        .iter()
        .any(|attr| simple_name(attr) == name)
}
fn simple_name(name: &str) -> &str {
    name.split_once("::").map_or(name, |(_, name)| name)
}
fn named(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Class(name) | Ty::ValueType(name) | Ty::GenericInst { name, .. } => Some(name),
        _ => None,
    }
}
fn is_collection(ty: &Ty) -> bool {
    matches!(ty, Ty::SzArray(_) | Ty::Array { .. })
        || matches!(ty,Ty::GenericInst{name,..} if simple_name(name)=="System.Collections.Generic.List`1")
}
fn is_intrinsic_base(ty: &Ty) -> bool {
    matches!(ty, Ty::Object)
        || named(ty).is_some_and(|name| {
            matches!(
                simple_name(name),
                "System.Object" | "System.ValueType" | "System.Enum"
            ) || is_framework_type(name)
        })
}

fn is_framework_type(name: &str) -> bool {
    let Some((assembly, full)) = name.split_once("::") else {
        return false;
    };
    full.starts_with("System.")
        && (is_assembly_blacklisted(assembly) || assembly.eq_ignore_ascii_case("System.Runtime"))
}
fn is_base_stop(ty: &Ty) -> bool {
    is_intrinsic_base(ty)
        || named(ty).is_some_and(|name| {
            matches!(
                simple_name(name),
                "UnityEngine.Object" | "UnityEngine.MonoBehaviour" | "UnityEngine.ScriptableObject"
            )
        })
}
fn primitive(ty: &Ty) -> Option<&'static str> {
    Some(match ty {
        Ty::Boolean | Ty::U8 => "UInt8",
        Ty::Char | Ty::U16 => "UInt16",
        Ty::I8 => "SInt8",
        Ty::I16 => "SInt16",
        Ty::I32 => "int",
        Ty::U32 => "unsigned int",
        Ty::I64 => "SInt64",
        Ty::U64 => "UInt64",
        Ty::F32 => "float",
        Ty::F64 => "double",
        _ => return None,
    })
}

fn substitute(ty: &Ty, args: &[Ty], depth: usize) -> Result<Ty> {
    ensure!(
        depth < 64,
        "Managed generic substitution is cyclic or too deep"
    );
    Ok(match ty {
        Ty::Var(index) => args
            .get(*index as usize)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Ty::SzArray(inner) => Ty::SzArray(Box::new(substitute(inner, args, depth + 1)?)),
        Ty::Array { element, rank } => Ty::Array {
            element: Box::new(substitute(element, args, depth + 1)?),
            rank: *rank,
        },
        Ty::GenericInst {
            value_type,
            name,
            args: params,
        } => Ty::GenericInst {
            value_type: *value_type,
            name: name.clone(),
            args: params
                .iter()
                .map(|param| substitute(param, args, depth + 1))
                .collect::<Result<_>>()?,
        },
        Ty::Pointer(inner) => Ty::Pointer(Box::new(substitute(inner, args, depth + 1)?)),
        Ty::ByRef(inner) => Ty::ByRef(Box::new(substitute(inner, args, depth + 1)?)),
        _ => ty.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutually_recursive_value_types_fail_at_shared_expansion_limit() {
        let value_type = |name: &str, other: &str| ManagedType {
            namespace: "Example".into(),
            name: name.into(),
            full_name: format!("Example.{name}"),
            declaring_type: None,
            base_type: Some(Ty::ValueType("mscorlib::System.ValueType".into())),
            flags: 0x2000,
            generic_parameters: Vec::new(),
            custom_attributes: Vec::new(),
            fields: vec![ManagedField {
                name: "nested".into(),
                flags: 6,
                signature: Ty::ValueType(format!("Cycle::Example.{other}")),
                custom_attributes: Vec::new(),
            }],
        };
        let assembly = Arc::new(ManagedAssembly {
            name: "Cycle".into(),
            references: Vec::new(),
            types: vec![value_type("A", "B"), value_type("B", "A")],
            type_forwarders: HashMap::new(),
        });
        let mut serializer = Serializer::new(HashMap::new());
        serializer.loaded.insert("cycle.dll".into(), assembly);
        let error = serializer
            .generate("Cycle", "Example", "A", &UnityVersion::new(6000, 0, 0))
            .unwrap_err();
        assert!(format!("{error:#}").contains("type expansion"));
        assert!(serializer.visits < 256);
    }

    #[test]
    fn generated_tree_and_shared_model_budgets_apply_before_retention() {
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/managed");
        for (node_budget, string_budget, model_budget, expected) in [
            (super::super::MAX_TOTAL_NODES, 0, 0, "tree work"),
            (0, super::super::MAX_SCHEMA_STRING_BYTES, 0, "string byte"),
            (0, 0, super::super::MAX_TOTAL_DLL_BYTES, "retained model"),
        ] {
            let index = super::super::assembly_index(&directory).unwrap();
            let mut serializer = Serializer::new(index);
            serializer.tree_work = node_budget;
            serializer.tree_string_work = string_budget;
            serializer.retained_model_bytes = model_budget;
            let error = serializer
                .generate(
                    "Fixtures",
                    "Fixtures",
                    "Primitives",
                    &UnityVersion::new(6000, 0, 0),
                )
                .unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            if model_budget != 0 {
                assert!(serializer.loaded.is_empty());
            }
        }
    }
}
