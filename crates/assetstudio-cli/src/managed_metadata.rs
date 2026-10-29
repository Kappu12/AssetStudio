//! Bounded, metadata-only ECMA-335 assembly inspection.
//!
//! The raw table reader is clrmeta 0.1.0, MIT (c) 2026 coconutbird. PE address
//! mapping, resource preflight, signature projection and name resolution here
//! follow ECMA-335 II.22–II.24. No method body is read or executed. No GPL
//! implementation is used by this module.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use clrmeta::Metadata;
use clrmeta::tables::{CodedIndex, TableId};

const MAX_ASSEMBLY_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: usize = 1_000_000;
const MAX_TYPES: usize = 100_000;
const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 4096;
const MAX_SIGNATURE_BYTES: usize = 64 * 1024;
const MAX_SIGNATURE_NODES: usize = 500_000;
const MAX_RESOLUTION_STEPS: usize = 4_000_000;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Debug)]
pub struct ManagedAssembly {
    pub name: String,
    pub references: Vec<String>,
    pub types: Vec<ManagedType>,
    /// Full local name to `TargetAssembly::FullName`.
    pub type_forwarders: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ManagedType {
    pub namespace: String,
    pub name: String,
    /// Nested types use `Namespace.Outer+Inner`.
    pub full_name: String,
    pub declaring_type: Option<String>,
    pub base_type: Option<ManagedFieldType>,
    pub flags: u32,
    pub generic_parameters: Vec<String>,
    pub custom_attributes: Vec<String>,
    pub fields: Vec<ManagedField>,
}

#[derive(Clone, Debug)]
pub struct ManagedField {
    pub name: String,
    pub flags: u16,
    pub signature: ManagedFieldType,
    pub custom_attributes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedFieldType {
    Void,
    Boolean,
    Char,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
    Object,
    Class(String),
    ValueType(String),
    SzArray(Box<Self>),
    Array {
        element: Box<Self>,
        rank: u32,
    },
    GenericInst {
        value_type: bool,
        name: String,
        args: Vec<Self>,
    },
    Var(u32),
    MVar(u32),
    ByRef(Box<Self>),
    Pointer(Box<Self>),
    /// Parsed and bounded, but never a Unity-serializable field kind.
    FunctionPointer,
}

pub fn parse_assembly(bytes: &[u8]) -> Result<ManagedAssembly> {
    ensure!(
        bytes.len() <= MAX_ASSEMBLY_BYTES,
        "Managed assembly exceeds 64 MiB"
    );
    let raw = metadata_region(bytes)?;
    preflight_metadata(raw)?;
    let metadata = Metadata::parse(raw).context("Invalid ECMA-335 metadata")?;
    ensure!(
        metadata.assemblies.len() == 1,
        "Managed input must have one assembly manifest"
    );
    ensure!(
        metadata.modules.len() == 1,
        "Managed input must contain exactly one Module row"
    );
    let name = metadata.strings.get(metadata.assemblies[0].name)?;
    valid_component(name)?;
    let assembly = name.to_owned();
    let mut resolver = Resolver::new(&metadata, &assembly)?;
    let references = metadata
        .assembly_refs
        .iter()
        .map(|row| resolver.text(metadata.strings.get(row.name)?))
        .collect::<Result<Vec<_>>>()?;
    for name in &references {
        valid_component(name)?;
    }

    let mut method_owners = vec![None; metadata.method_defs.len()];
    let method_list_count = if metadata.method_ptrs.is_empty() {
        metadata.method_defs.len()
    } else {
        metadata.method_ptrs.len()
    };
    let field_list_count = if metadata.field_ptrs.is_empty() {
        metadata.fields.len()
    } else {
        metadata.field_ptrs.len()
    };
    let mut owned_fields = vec![false; metadata.fields.len()];
    for (index, row) in metadata.type_defs.iter().enumerate() {
        let end = metadata
            .type_defs
            .get(index + 1)
            .map_or(method_list_count as u32 + 1, |next| next.method_list);
        for item in list_range(row.method_list, end, method_list_count, "method")? {
            let actual = if metadata.method_ptrs.is_empty() {
                item
            } else {
                row_index(
                    metadata.method_ptrs[item].method,
                    metadata.method_defs.len(),
                    "MethodPtr target",
                )?
            };
            ensure!(
                method_owners[actual].replace(index).is_none(),
                "A managed method has multiple owning types"
            );
        }
    }

    let mut type_attributes = vec![Vec::new(); metadata.type_defs.len()];
    let mut field_attributes = vec![Vec::new(); metadata.fields.len()];
    for attribute in &metadata.custom_attributes {
        if !matches!(
            attribute.parent.table,
            Some(TableId::TypeDef | TableId::Field)
        ) {
            continue;
        }
        let owner = resolver.attribute_owner(attribute.attr_type, &method_owners)?;
        let owner = owner
            .split_once("::")
            .map_or(owner.as_str(), |(_, name)| name);
        let name = resolver.text(owner)?;
        // Attribute data are not evaluated, but its blob must be in range.
        metadata.blobs.get(attribute.value)?;
        match attribute.parent.table {
            Some(TableId::TypeDef) => {
                let index = row_index(
                    attribute.parent.row,
                    type_attributes.len(),
                    "attribute type owner",
                )?;
                type_attributes[index].try_reserve(1)?;
                type_attributes[index].push(name);
            }
            Some(TableId::Field) => {
                let index = row_index(
                    attribute.parent.row,
                    field_attributes.len(),
                    "attribute field owner",
                )?;
                field_attributes[index].try_reserve(1)?;
                field_attributes[index].push(name);
            }
            _ => {}
        }
    }

    let mut generics: Vec<Vec<(u16, String)>> = vec![Vec::new(); metadata.type_defs.len()];
    for generic in &metadata.generic_params {
        if generic.owner.table != Some(TableId::TypeDef) {
            continue;
        }
        let index = row_index(generic.owner.row, generics.len(), "generic type owner")?;
        ensure!(
            generic.number < 256,
            "Managed generic parameter index exceeds 255"
        );
        let name = resolver.text(metadata.strings.get(generic.name)?)?;
        generics[index].try_reserve(1)?;
        generics[index].push((generic.number, name));
    }
    let mut types = Vec::new();
    types.try_reserve_exact(metadata.type_defs.len())?;
    let mut names = HashSet::new();
    names.try_reserve(metadata.type_defs.len())?;
    for (index, row) in metadata.type_defs.iter().enumerate() {
        let full_name = resolver.definition_name(index, 0)?;
        ensure!(
            names.insert(full_name.clone()),
            "Duplicate managed type {full_name:?}"
        );
        let base_type = if row.extends.row == 0 {
            ensure!(
                matches!(row.extends.table, None | Some(TableId::TypeDef)),
                "Managed base type has a nonzero tag on its null coded index"
            );
            None
        } else {
            let base = resolver.coded_type(row.extends, 0)?;
            ensure!(
                matches!(
                    base,
                    ManagedFieldType::Class(_)
                        | ManagedFieldType::ValueType(_)
                        | ManagedFieldType::GenericInst { .. }
                ),
                "Managed base type is not a named class or generic instance"
            );
            Some(base)
        };
        let declaring_type = resolver.parents[index]
            .map(|parent| resolver.definition_name(parent, 0))
            .transpose()?;
        let mut generic_parameters = std::mem::take(&mut generics[index]);
        generic_parameters.sort_by_key(|(number, _)| *number);
        for (expected, (number, _)) in generic_parameters.iter().enumerate() {
            ensure!(
                usize::from(*number) == expected,
                "Managed generic parameters contain a gap or duplicate"
            );
        }
        let end = metadata
            .type_defs
            .get(index + 1)
            .map_or(field_list_count as u32 + 1, |next| next.field_list);
        let mut fields = Vec::new();
        for item in list_range(row.field_list, end, field_list_count, "field")? {
            let field_index = if metadata.field_ptrs.is_empty() {
                item
            } else {
                row_index(
                    metadata.field_ptrs[item].field,
                    metadata.fields.len(),
                    "FieldPtr target",
                )?
            };
            ensure!(
                !std::mem::replace(&mut owned_fields[field_index], true),
                "A managed field has multiple owning types"
            );
            let field = &metadata.fields[field_index];
            let signature = metadata.blobs.get(field.signature)?;
            ensure!(
                signature.len() <= MAX_SIGNATURE_BYTES,
                "Managed field signature exceeds 64 KiB"
            );
            let mut reader = Reader::new(signature);
            ensure!(
                reader.byte()? == 0x06,
                "Managed field has an invalid field-signature prefix"
            );
            let signature = resolver.signature(&mut reader, 0)?;
            ensure!(
                signature != ManagedFieldType::Void,
                "Managed field cannot have the void type"
            );
            ensure!(
                reader.remaining() == 0,
                "Managed field signature has unexpected trailing bytes"
            );
            fields.try_reserve(1)?;
            fields.push(ManagedField {
                name: resolver.text(metadata.strings.get(field.name)?)?,
                flags: field.flags,
                signature,
                custom_attributes: std::mem::take(&mut field_attributes[field_index]),
            });
        }
        types.push(ManagedType {
            namespace: resolver.text(metadata.strings.get(row.type_namespace)?)?,
            name: resolver.text(metadata.strings.get(row.type_name)?)?,
            full_name,
            declaring_type,
            base_type,
            flags: row.flags,
            generic_parameters: generic_parameters
                .into_iter()
                .map(|(_, name)| name)
                .collect(),
            custom_attributes: std::mem::take(&mut type_attributes[index]),
            fields,
        });
    }
    ensure!(
        owned_fields.iter().all(|owned| *owned),
        "Managed field table contains an unowned field"
    );
    let mut type_forwarders = HashMap::new();
    type_forwarders.try_reserve(metadata.exported_types.len())?;
    for index in 0..metadata.exported_types.len() {
        if let Some((name, target)) = resolver.forwarder(index, 0)? {
            ensure!(
                type_forwarders.insert(name, target).is_none(),
                "Duplicate managed type forwarder"
            );
        }
    }
    Ok(ManagedAssembly {
        name: assembly,
        references,
        types,
        type_forwarders,
    })
}

fn list_range(start: u32, end: u32, count: usize, kind: &str) -> Result<std::ops::Range<usize>> {
    ensure!(
        start >= 1 && end >= start && end as usize <= count + 1,
        "Managed {kind} ownership range is invalid"
    );
    Ok(start as usize - 1..end as usize - 1)
}

fn row_index(index: u32, length: usize, kind: &str) -> Result<usize> {
    ensure!(
        index > 0 && index as usize <= length,
        "Invalid managed {kind} row {index}"
    );
    Ok(index as usize - 1)
}

fn valid_component(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= MAX_NAME_BYTES && !value.contains("::"),
        "Invalid or excessively long managed name component"
    );
    Ok(())
}

struct Resolver<'a> {
    metadata: &'a Metadata,
    assembly: &'a str,
    parents: Vec<Option<usize>>,
    definition_names: Vec<Option<String>>,
    reference_names: Vec<Option<String>>,
    text_bytes: Cell<usize>,
    signature_nodes: Cell<usize>,
    resolution_steps: Cell<usize>,
}

impl<'a> Resolver<'a> {
    fn new(metadata: &'a Metadata, assembly: &'a str) -> Result<Self> {
        let mut parents = vec![None; metadata.type_defs.len()];
        for pair in &metadata.nested_classes {
            let nested = row_index(pair.nested_class, parents.len(), "nested type")?;
            let parent = row_index(pair.enclosing_class, parents.len(), "enclosing type")?;
            ensure!(
                nested != parent && parents[nested].replace(parent).is_none(),
                "Managed nested type has a cycle or duplicate owner"
            );
        }
        Ok(Self {
            metadata,
            assembly,
            parents,
            definition_names: vec![None; metadata.type_defs.len()],
            reference_names: vec![None; metadata.type_refs.len()],
            text_bytes: Cell::new(assembly.len()),
            signature_nodes: Cell::new(0),
            resolution_steps: Cell::new(0),
        })
    }

    fn text(&self, value: &str) -> Result<String> {
        ensure!(
            value.len() <= MAX_NAME_BYTES,
            "Managed metadata name exceeds 4096 bytes"
        );
        let next = self
            .text_bytes
            .get()
            .checked_add(value.len())
            .context("Managed text budget overflow")?;
        ensure!(
            next <= MAX_TEXT_BYTES,
            "Managed metadata names exceed 64 MiB"
        );
        self.text_bytes.set(next);
        let mut result = String::new();
        result.try_reserve_exact(value.len())?;
        result.push_str(value);
        Ok(result)
    }

    fn resolution_step(&self) -> Result<()> {
        let next = self
            .resolution_steps
            .get()
            .checked_add(1)
            .context("Managed name-resolution work counter overflow")?;
        ensure!(
            next <= MAX_RESOLUTION_STEPS,
            "Managed name resolution exceeds four million steps"
        );
        self.resolution_steps.set(next);
        Ok(())
    }

    fn join(&self, left: &str, delimiter: &str, right: &str) -> Result<String> {
        let length = left
            .len()
            .checked_add(delimiter.len())
            .and_then(|value| value.checked_add(right.len()))
            .context("Managed name length overflow")?;
        ensure!(
            length <= MAX_NAME_BYTES,
            "Resolved managed name exceeds 4096 bytes"
        );
        let mut result = String::new();
        result.try_reserve_exact(length)?;
        result.push_str(left);
        result.push_str(delimiter);
        result.push_str(right);
        self.text(&result)
    }

    fn definition_name(&mut self, index: usize, depth: usize) -> Result<String> {
        self.resolution_step()?;
        ensure!(
            depth <= MAX_DEPTH,
            "Managed nesting cycle or depth exceeds 128"
        );
        if let Some(name) = &self.definition_names[index] {
            return self.text(name);
        }
        let row = &self.metadata.type_defs[index];
        let name = self.metadata.strings.get(row.type_name)?;
        valid_component(name)?;
        let namespace = self.metadata.strings.get(row.type_namespace)?;
        if !namespace.is_empty() {
            valid_component(namespace)?;
        }
        let full = if let Some(parent) = self.parents[index] {
            let parent = self.definition_name(parent, depth + 1)?;
            self.join(&parent, "+", name)?
        } else if namespace.is_empty() {
            self.text(name)?
        } else {
            self.join(namespace, ".", name)?
        };
        self.definition_names[index] = Some(self.text(&full)?);
        Ok(full)
    }

    fn assembly_reference(&self, index: u32) -> Result<String> {
        let index = row_index(index, self.metadata.assembly_refs.len(), "AssemblyRef")?;
        let name = self
            .metadata
            .strings
            .get(self.metadata.assembly_refs[index].name)?;
        valid_component(name)?;
        self.text(name)
    }

    fn reference_name(&mut self, index: usize, depth: usize) -> Result<String> {
        self.resolution_step()?;
        ensure!(
            depth <= MAX_DEPTH,
            "Managed TypeRef cycle or depth exceeds 128"
        );
        if let Some(name) = &self.reference_names[index] {
            return self.text(name);
        }
        let row = &self.metadata.type_refs[index];
        let name = self.metadata.strings.get(row.type_name)?;
        valid_component(name)?;
        let namespace = self.metadata.strings.get(row.type_namespace)?;
        if !namespace.is_empty() {
            valid_component(namespace)?;
        }
        let scope = row.resolution_scope;
        let full = match scope.table {
            Some(TableId::TypeRef) if scope.row > 0 => {
                let parent = row_index(
                    scope.row,
                    self.metadata.type_refs.len(),
                    "enclosing TypeRef",
                )?;
                let parent = self.reference_name(parent, depth + 1)?;
                self.join(&parent, "+", name)?
            }
            Some(TableId::Module | TableId::AssemblyRef) | None => {
                if scope.table == Some(TableId::Module) {
                    ensure!(
                        scope.row as usize <= self.metadata.modules.len(),
                        "Managed TypeRef has an invalid Module scope"
                    );
                }
                let full = if namespace.is_empty() {
                    self.text(name)?
                } else {
                    self.join(namespace, ".", name)?
                };
                let assembly = if scope.table == Some(TableId::AssemblyRef) {
                    self.assembly_reference(scope.row)?
                } else {
                    self.text(self.assembly)?
                };
                self.join(&assembly, "::", &full)?
            }
            _ => bail!("Multi-module or invalid managed TypeRef scope is not implemented"),
        };
        self.reference_names[index] = Some(self.text(&full)?);
        Ok(full)
    }

    fn coded_type(&mut self, value: CodedIndex, depth: usize) -> Result<ManagedFieldType> {
        self.resolution_step()?;
        ensure!(
            depth <= MAX_DEPTH,
            "Managed signature cycle or depth exceeds 128"
        );
        Ok(match value.table {
            Some(TableId::TypeDef) => {
                let index = row_index(value.row, self.metadata.type_defs.len(), "TypeDef")?;
                let name = self.definition_name(index, depth + 1)?;
                ManagedFieldType::Class(self.join(self.assembly, "::", &name)?)
            }
            Some(TableId::TypeRef) => {
                let index = row_index(value.row, self.metadata.type_refs.len(), "TypeRef")?;
                ManagedFieldType::Class(self.reference_name(index, depth + 1)?)
            }
            Some(TableId::TypeSpec) => {
                let index = row_index(value.row, self.metadata.type_specs.len(), "TypeSpec")?;
                let bytes = self
                    .metadata
                    .blobs
                    .get(self.metadata.type_specs[index].signature)?;
                ensure!(
                    bytes.len() <= MAX_SIGNATURE_BYTES,
                    "Managed TypeSpec signature exceeds 64 KiB"
                );
                let mut reader = Reader::new(bytes);
                let signature = self.signature(&mut reader, depth + 1)?;
                ensure!(
                    reader.remaining() == 0,
                    "Managed TypeSpec has trailing signature bytes"
                );
                signature
            }
            _ => bail!("Managed type coded index is invalid"),
        })
    }

    fn token_type(&mut self, token: u32, depth: usize) -> Result<ManagedFieldType> {
        let table = match token & 3 {
            0 => TableId::TypeDef,
            1 => TableId::TypeRef,
            2 => TableId::TypeSpec,
            _ => bail!("Invalid TypeDefOrRef signature tag"),
        };
        self.coded_type(
            CodedIndex {
                table: Some(table),
                row: token >> 2,
            },
            depth,
        )
    }

    fn token_name(&mut self, token: u32, depth: usize) -> Result<String> {
        match self.token_type(token, depth)? {
            ManagedFieldType::Class(name) | ManagedFieldType::ValueType(name) => Ok(name),
            _ => bail!("Managed named type resolves to a compound signature"),
        }
    }

    fn signature(&mut self, reader: &mut Reader<'_>, depth: usize) -> Result<ManagedFieldType> {
        ensure!(
            depth <= MAX_DEPTH,
            "Managed field signature depth exceeds 128"
        );
        let count = self
            .signature_nodes
            .get()
            .checked_add(1)
            .context("Managed signature node overflow")?;
        ensure!(
            count <= MAX_SIGNATURE_NODES,
            "Managed signatures exceed 500000 nodes"
        );
        self.signature_nodes.set(count);
        use ManagedFieldType as T;
        Ok(match reader.byte()? {
            0x01 => T::Void,
            0x02 => T::Boolean,
            0x03 => T::Char,
            0x04 => T::I8,
            0x05 => T::U8,
            0x06 => T::I16,
            0x07 => T::U16,
            0x08 => T::I32,
            0x09 => T::U32,
            0x0a => T::I64,
            0x0b => T::U64,
            0x0c => T::F32,
            0x0d => T::F64,
            0x0e => T::String,
            0x1c => T::Object,
            0x11 => T::ValueType(self.token_name(reader.compressed()?, depth + 1)?),
            0x12 => T::Class(self.token_name(reader.compressed()?, depth + 1)?),
            0x0f => T::Pointer(Box::new(self.signature(reader, depth + 1)?)),
            0x10 => T::ByRef(Box::new(self.signature(reader, depth + 1)?)),
            0x1d => T::SzArray(Box::new(self.signature(reader, depth + 1)?)),
            0x13 => T::Var(reader.compressed()?),
            0x1e => T::MVar(reader.compressed()?),
            0x14 => {
                let element = Box::new(self.signature(reader, depth + 1)?);
                let rank = reader.compressed()?;
                ensure!(rank > 0 && rank <= 32, "Managed array rank exceeds 32");
                let sizes = reader.compressed()?;
                ensure!(sizes <= rank, "Managed array has too many sizes");
                for _ in 0..sizes {
                    reader.compressed()?;
                }
                let bounds = reader.compressed()?;
                ensure!(bounds <= rank, "Managed array has too many lower bounds");
                for _ in 0..bounds {
                    reader.compressed()?;
                }
                T::Array { element, rank }
            }
            0x15 => {
                let kind = reader.byte()?;
                ensure!(
                    kind == 0x11 || kind == 0x12,
                    "Managed generic instance has invalid kind"
                );
                let name = self.token_name(reader.compressed()?, depth + 1)?;
                let count = reader.compressed()? as usize;
                ensure!(
                    count <= 256 && count <= reader.remaining(),
                    "Managed generic argument count exceeds bytes or limit"
                );
                let mut args = Vec::new();
                args.try_reserve_exact(count)?;
                for _ in 0..count {
                    args.push(self.signature(reader, depth + 1)?);
                }
                T::GenericInst {
                    value_type: kind == 0x11,
                    name,
                    args,
                }
            }
            0x1f | 0x20 => {
                self.token_type(reader.compressed()?, depth + 1)?;
                self.signature(reader, depth + 1)?
            }
            0x1b => {
                let convention = reader.byte()?;
                ensure!(
                    matches!(convention & 15, 0..=5 | 9),
                    "Managed function pointer has an invalid calling convention"
                );
                if convention & 0x10 != 0 {
                    ensure!(
                        reader.compressed()? <= 256,
                        "Managed function pointer has too many generic parameters"
                    );
                }
                let count = reader.compressed()? as usize;
                ensure!(
                    count <= 1024 && count <= reader.remaining(),
                    "Managed function pointer parameter count exceeds bytes or limit"
                );
                self.signature(reader, depth + 1)?;
                let mut sentinel = false;
                for _ in 0..count {
                    if reader.peek() == Some(0x41) {
                        ensure!(
                            convention & 15 == 5 && !sentinel,
                            "Invalid managed function pointer vararg sentinel"
                        );
                        reader.byte()?;
                        sentinel = true;
                    }
                    let parameter = self.signature(reader, depth + 1)?;
                    ensure!(
                        parameter != T::Void,
                        "Managed function pointer parameter cannot be void"
                    );
                }
                T::FunctionPointer
            }
            // These framework value types are never normal Unity fields. Keep
            // them named so the serialization policy can reject/skip them.
            0x16 => T::ValueType(self.text("System.TypedReference")?),
            0x18 => T::ValueType(self.text("System.IntPtr")?),
            0x19 => T::ValueType(self.text("System.UIntPtr")?),
            value => bail!("Managed field signature element {value:#x} is not implemented"),
        })
    }

    fn attribute_owner(
        &mut self,
        constructor: CodedIndex,
        method_owners: &[Option<usize>],
    ) -> Result<String> {
        self.resolution_step()?;
        match constructor.table {
            Some(TableId::MethodDef) => {
                let index = row_index(
                    constructor.row,
                    self.metadata.method_defs.len(),
                    "attribute constructor",
                )?;
                ensure!(
                    self.metadata
                        .strings
                        .get(self.metadata.method_defs[index].name)?
                        == ".ctor",
                    "Custom attribute method is not a constructor"
                );
                let owner = method_owners[index]
                    .context("Custom attribute constructor has no type owner")?;
                self.definition_name(owner, 0)
            }
            Some(TableId::MemberRef) => {
                let index = row_index(
                    constructor.row,
                    self.metadata.member_refs.len(),
                    "attribute MemberRef",
                )?;
                let row = &self.metadata.member_refs[index];
                ensure!(
                    self.metadata.strings.get(row.name)? == ".ctor",
                    "Custom attribute MemberRef is not a constructor"
                );
                match self.coded_type(row.class, 0)? {
                    ManagedFieldType::Class(name) | ManagedFieldType::ValueType(name) => Ok(name),
                    ManagedFieldType::GenericInst { name, .. } => Ok(name),
                    _ => bail!("Custom attribute constructor owner is not a named type"),
                }
            }
            _ => bail!("Custom attribute constructor token is invalid"),
        }
    }

    fn forwarder(&mut self, index: usize, depth: usize) -> Result<Option<(String, String)>> {
        self.resolution_step()?;
        ensure!(
            depth <= MAX_DEPTH,
            "Managed exported type cycle or depth exceeds 128"
        );
        let row = &self.metadata.exported_types[index];
        let name = self.metadata.strings.get(row.type_name)?;
        valid_component(name)?;
        let namespace = self.metadata.strings.get(row.type_namespace)?;
        if !namespace.is_empty() {
            valid_component(namespace)?;
        }
        match row.implementation.table {
            Some(TableId::AssemblyRef) => {
                let full = if namespace.is_empty() {
                    self.text(name)?
                } else {
                    self.join(namespace, ".", name)?
                };
                let assembly = self.assembly_reference(row.implementation.row)?;
                let target = self.join(&assembly, "::", &full)?;
                Ok(Some((full, target)))
            }
            Some(TableId::ExportedType) => {
                let parent = row_index(
                    row.implementation.row,
                    self.metadata.exported_types.len(),
                    "enclosing exported type",
                )?;
                let Some((full, target)) = self.forwarder(parent, depth + 1)? else {
                    return Ok(None);
                };
                Ok(Some((
                    self.join(&full, "+", name)?,
                    self.join(&target, "+", name)?,
                )))
            }
            Some(TableId::File) => Ok(None),
            _ => bail!("Managed type forwarder implementation token is invalid"),
        }
    }
}

/// Only maps file-backed RVA ranges. Virtual zero-fill cannot contain metadata.
fn metadata_region(bytes: &[u8]) -> Result<&[u8]> {
    ensure!(
        bytes.get(..2) == Some(b"MZ"),
        "Managed input is not a PE image"
    );
    let pe = read_u32_at(bytes, 0x3c)? as usize;
    ensure!(
        slice(bytes, pe, 4)? == b"PE\0\0",
        "Managed PE signature is invalid"
    );
    let sections = read_u16_at(bytes, pe + 6)? as usize;
    ensure!(
        sections > 0 && sections <= 96,
        "Managed PE section count is invalid"
    );
    let optional_size = read_u16_at(bytes, pe + 20)? as usize;
    let optional = pe
        .checked_add(24)
        .context("PE optional header offset overflow")?;
    let magic = read_u16_at(bytes, optional)?;
    let directory_start = match magic {
        0x10b => 96,
        0x20b => 112,
        _ => bail!("Managed PE optional header kind is invalid"),
    };
    ensure!(
        optional_size >= directory_start + 15 * 8,
        "Managed PE optional header has no CLI data directory"
    );
    let directory_count = read_u32_at(bytes, optional + directory_start - 4)?;
    ensure!(
        directory_count >= 15,
        "Managed PE has no CLI data directory"
    );
    let cli_rva = read_u32_at(bytes, optional + directory_start + 14 * 8)?;
    let cli_size = read_u32_at(bytes, optional + directory_start + 14 * 8 + 4)?;
    ensure!(
        cli_rva > 0 && cli_size >= 72,
        "Managed PE CLI header is missing or truncated"
    );
    let section_table = optional
        .checked_add(optional_size)
        .context("PE section table overflow")?;
    let headers_size = read_u32_at(bytes, optional + 60)? as usize;
    ensure!(
        headers_size <= bytes.len(),
        "Managed PE SizeOfHeaders exceeds file length"
    );
    let map = |rva: u32, length: usize| -> Result<&[u8]> {
        if (rva as usize) < headers_size {
            return slice(bytes, rva as usize, length);
        }
        let mut found = None;
        for index in 0..sections {
            let entry = section_table
                .checked_add(index * 40)
                .context("PE section offset overflow")?;
            let address = read_u32_at(bytes, entry + 12)?;
            let raw_size = read_u32_at(bytes, entry + 16)?;
            let raw_offset = read_u32_at(bytes, entry + 20)?;
            if rva >= address {
                let delta = rva - address;
                if u64::from(delta) + length as u64 <= u64::from(raw_size) {
                    ensure!(
                        found.is_none(),
                        "Managed PE has ambiguous overlapping RVA mappings"
                    );
                    let offset = (raw_offset as usize)
                        .checked_add(delta as usize)
                        .context("PE raw offset overflow")?;
                    found = Some(slice(bytes, offset, length)?);
                }
            }
        }
        found.context("Managed PE RVA range is not backed by file bytes")
    };
    let cli = map(cli_rva, 72)?;
    ensure!(
        read_u32_at(cli, 0)? >= 72,
        "Managed CLI header length is invalid"
    );
    let metadata_rva = read_u32_at(cli, 8)?;
    let metadata_size = read_u32_at(cli, 12)? as usize;
    ensure!(
        metadata_size > 0 && metadata_size <= MAX_ASSEMBLY_BYTES,
        "Managed metadata byte count is invalid"
    );
    map(metadata_rva, metadata_size)
}

fn preflight_metadata(bytes: &[u8]) -> Result<()> {
    let mut reader = Reader::new(bytes);
    ensure!(
        reader.u32()? == 0x424a5342,
        "Managed metadata BSJB signature is invalid"
    );
    reader.take(8)?;
    let version = reader.u32()? as usize;
    ensure!(
        version <= 1024 && version.is_multiple_of(4),
        "Managed runtime version string length is invalid"
    );
    reader.take(version)?;
    reader.u16()?;
    let count = reader.u16()? as usize;
    ensure!(
        count > 0 && count <= 32,
        "Managed metadata stream count exceeds 32"
    );
    let mut ranges = Vec::new();
    let mut names = HashSet::new();
    let mut table = None;
    for _ in 0..count {
        let offset = reader.u32()? as usize;
        let size = reader.u32()? as usize;
        let name = reader.string(32)?;
        ensure!(names.insert(name), "Duplicate managed metadata stream");
        let padding = (4 - (name.len() + 1) % 4) % 4;
        reader.take(padding)?;
        let stream = slice(bytes, offset, size)?;
        if matches!(name, b"#Strings" | b"#Blob" | b"#US") {
            ensure!(
                stream.first() == Some(&0),
                "Managed metadata heap must begin with its reserved zero byte"
            );
        }
        if name == b"#GUID" {
            ensure!(
                size.is_multiple_of(16),
                "Managed GUID heap size is not a multiple of 16"
            );
        }
        if name == b"#~" || name == b"#-" {
            ensure!(table.is_none(), "Multiple managed tables streams");
            table = Some(stream);
        }
        if size > 0 {
            ranges.push((offset, offset + size));
        }
    }
    ensure!(
        names.contains(b"#Strings".as_slice()) && names.contains(b"#Blob".as_slice()),
        "Managed string/blob heaps are missing"
    );
    ranges.sort_unstable();
    let mut end = reader.position;
    for (start, next) in ranges {
        ensure!(
            start >= end,
            "Managed metadata streams overlap each other or the header"
        );
        end = next;
    }
    let table = table.context("Managed metadata has no tables stream")?;
    ensure!(
        table.get(6).is_some_and(|flags| flags & !7 == 0),
        "Managed tables stream uses unimplemented delta/extra-data flags"
    );
    let mut reader = Reader::new(table);
    reader.take(8)?;
    let valid = reader.u64()?;
    reader.u64()?;
    ensure!(
        valid >> 45 == 0,
        "Managed metadata contains unimplemented table kinds"
    );
    let mut total = 0_usize;
    for index in 0..45 {
        if valid & (1_u64 << index) != 0 {
            let rows = reader.u32()? as usize;
            ensure!(
                rows <= MAX_ROWS && rows <= table.len() / 2,
                "Managed table row count exceeds bytes or limit"
            );
            if index == 2 {
                ensure!(rows <= MAX_TYPES, "Managed type count exceeds 100000");
            }
            total = total
                .checked_add(rows)
                .context("Managed table row count overflow")?;
            ensure!(
                total <= MAX_ROWS,
                "Managed metadata exceeds one million table rows"
            );
        }
    }
    Ok(())
}

fn slice(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    let end = start
        .checked_add(length)
        .context("Managed byte range overflow")?;
    bytes.get(start..end).context("Managed input is truncated")
}
fn read_u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(bytes, offset, 2)?.try_into()?))
}
fn read_u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(bytes, offset, 4)?.try_into()?))
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let bytes = slice(self.bytes, self.position, length)?;
        self.position += length;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn string(&mut self, maximum: usize) -> Result<&'a [u8]> {
        let length = self.bytes[self.position..]
            .iter()
            .take(maximum)
            .position(|byte| *byte == 0)
            .context("Managed stream name is too long or unterminated")?;
        let bytes = self.take(length + 1)?;
        Ok(&bytes[..length])
    }
    fn compressed(&mut self) -> Result<u32> {
        let first = self.byte()?;
        Ok(if first & 0x80 == 0 {
            u32::from(first)
        } else if first & 0xc0 == 0x80 {
            u32::from(first & 0x3f) << 8 | u32::from(self.byte()?)
        } else {
            ensure!(
                first & 0xe0 == 0xc0,
                "Managed compressed integer prefix is invalid"
            );
            let rest = self.take(3)?;
            u32::from(first & 0x1f) << 24
                | u32::from(rest[0]) << 16
                | u32::from(rest[1]) << 8
                | u32::from(rest[2])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &[u8] = include_bytes!("../tests/fixtures/managed/Fixtures.dll");
    const ENGINE: &[u8] = include_bytes!("../tests/fixtures/managed/UnityEngine.dll");
    const CORE: &[u8] = include_bytes!("../tests/fixtures/managed/UnityEngine.CoreModule.dll");

    #[test]
    fn projects_primitives_attributes_generic_inheritance_and_arrays() {
        let assembly = parse_assembly(FIXTURES).unwrap();
        assert_eq!(assembly.name, "Fixtures");
        let primitives = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.Primitives")
            .unwrap();
        assert_eq!(primitives.fields.len(), 12);
        assert_eq!(primitives.fields[0].signature, ManagedFieldType::Boolean);
        let visibility = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.FieldVisibility")
            .unwrap();
        let private = visibility
            .fields
            .iter()
            .find(|field| field.name == "serializedPrivate")
            .unwrap();
        assert!(
            private
                .custom_attributes
                .iter()
                .any(|name| name == "UnityEngine.SerializeField")
        );
        let arrays = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.Arrays")
            .unwrap();
        assert!(matches!(
            arrays.fields[0].signature,
            ManagedFieldType::SzArray(_)
        ));
        let multidim = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.MultidimArrays")
            .unwrap();
        assert!(matches!(
            multidim.fields[1].signature,
            ManagedFieldType::Array { rank: 2, .. }
        ));
        let holder = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.Holder`1")
            .unwrap();
        assert_eq!(holder.generic_parameters, ["T"]);
        assert_eq!(holder.fields[0].signature, ManagedFieldType::Var(0));
    }

    #[test]
    fn nested_references_and_type_forwarders_retain_assembly_identity() {
        let assembly = parse_assembly(FIXTURES).unwrap();
        let nested = assembly
            .types
            .iter()
            .find(|ty| ty.full_name == "Fixtures.NestedEnumField")
            .unwrap();
        assert_eq!(
            nested.fields[0].signature,
            ManagedFieldType::ValueType(
                "UnityEngine.CoreModule::UnityEngine.Navigation+Mode".to_owned()
            )
        );
        let facade = parse_assembly(ENGINE).unwrap();
        assert_eq!(
            facade
                .type_forwarders
                .get("UnityEngine.ForwardedAsset")
                .unwrap(),
            "UnityEngine.CoreModule::UnityEngine.ForwardedAsset"
        );
        let core = parse_assembly(CORE).unwrap();
        let nested = core
            .types
            .iter()
            .find(|ty| ty.full_name == "UnityEngine.Navigation+Mode")
            .unwrap();
        assert_eq!(
            nested.declaring_type.as_deref(),
            Some("UnityEngine.Navigation")
        );
    }

    #[test]
    fn malformed_pe_and_metadata_are_errors_not_panics() {
        for length in 0..256 {
            assert!(parse_assembly(&FIXTURES[..length]).is_err());
        }
        for index in (0..FIXTURES.len()).step_by(23) {
            let mut bytes = FIXTURES.to_vec();
            bytes[index] ^= 0xff;
            assert!(
                std::panic::catch_unwind(|| parse_assembly(&bytes)).is_ok(),
                "metadata mutation at {index} panicked"
            );
        }
    }

    #[test]
    fn function_pointer_signatures_are_bounded_and_not_executed() {
        let metadata = Metadata::parse(metadata_region(FIXTURES).unwrap()).unwrap();
        let mut resolver = Resolver::new(&metadata, "Fixtures").unwrap();
        // fnptr default, two parameters, void return, int32 and void* params.
        let mut reader = Reader::new(&[0x1b, 0, 2, 1, 8, 0x0f, 1]);
        assert_eq!(
            resolver.signature(&mut reader, 0).unwrap(),
            ManagedFieldType::FunctionPointer
        );
        assert_eq!(reader.remaining(), 0);
        let mut recursive = Vec::new();
        for _ in 0..256 {
            recursive.extend_from_slice(&[0x1b, 0, 0]);
        }
        recursive.push(1);
        assert!(resolver.signature(&mut Reader::new(&recursive), 0).is_err());
    }

    #[test]
    fn metadata_preflight_rejects_overflowing_streams_and_row_counts() {
        let raw = metadata_region(FIXTURES).unwrap();
        let version_length = read_u32_at(raw, 12).unwrap() as usize;
        let first_stream = 20 + version_length;
        let mut broken = raw.to_vec();
        broken[first_stream..first_stream + 8].fill(0xff);
        assert!(preflight_metadata(&broken).is_err());
        let mut directory = Reader::new(&raw[first_stream..]);
        let table_offset = loop {
            let offset = directory.u32().unwrap() as usize;
            directory.u32().unwrap();
            let name = directory.string(32).unwrap();
            directory.take((4 - (name.len() + 1) % 4) % 4).unwrap();
            if matches!(name, b"#~" | b"#-") {
                break offset;
            }
        };
        let mut broken = raw.to_vec();
        broken[table_offset + 24..table_offset + 28].fill(0xff);
        assert!(preflight_metadata(&broken).is_err());
    }
}
