//! Internal, short-lived Rust metadata helper. Never executes an assembly.
//!
//! Metadata uses the MIT clrmeta parser and owned structures. The native child
//! additionally bounds parser work, failures, output, and process lifetime.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use assetstudio_core::schema::{
    BoundedWriter, MAX_GENERATOR_OUTPUT_BYTES, MAX_GENERATOR_REQUEST_BYTES, ManagedSchemaRequest,
};
use serde::Serialize;
#[path = "managed_metadata.rs"]
mod managed_metadata;
#[path = "managed_serializer.rs"]
mod managed_serializer;
#[path = "managed_templates.rs"]
mod managed_templates;
use managed_templates::TemplateField;

const MAX_DIRECTORY_ENTRIES: usize = 16_384;
const MAX_DLL_FILES: usize = 4096;
const MAX_DLL_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_DLL_BYTES: usize = 256 * 1024 * 1024;
const MAX_TOTAL_NODES: usize = 500_000;
const MAX_ENTRY_NODES: usize = 100_000;
const MAX_DEPTH: u32 = 128;
const MAX_SCHEMA_STRING_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize)]
struct Document {
    version: u32,
    entries: Vec<Entry>,
}
#[derive(Serialize)]
struct Entry {
    assembly: String,
    namespace: String,
    #[serde(rename = "class")]
    class_name: String,
    unity_version: String,
    nodes: Vec<Node>,
}
#[derive(Serialize)]
struct Node {
    level: u32,
    #[serde(rename = "type")]
    type_name: String,
    name: String,
    byte_size: i32,
    meta_flags: i32,
    type_flags: i32,
}

/// Reads a bounded request and writes only a version-1 schema JSON document.
pub fn run_child(path: &Path) -> Result<()> {
    let request = read_request(path)?;
    let document = generate(&request)?;
    let stdout = io::stdout();
    let mut output = BufWriter::new(BoundedWriter::new(
        stdout.lock(),
        MAX_GENERATOR_OUTPUT_BYTES,
    ));
    serde_json::to_writer(&mut output, &document)?;
    output.flush()?;
    Ok(())
}

fn read_request(path: &Path) -> Result<ManagedSchemaRequest> {
    let bytes = read_file(path, MAX_GENERATOR_REQUEST_BYTES)?;
    let request: ManagedSchemaRequest =
        serde_json::from_slice(&bytes).context("Invalid native schema request")?;
    request.validate()?;
    Ok(request)
}

fn generate(request: &ManagedSchemaRequest) -> Result<Document> {
    request.validate()?;
    let directory = fs::canonicalize(&request.dll_directory)?;
    let index = assembly_index(&directory)?;
    let mut serializer = managed_serializer::Serializer::new(index);
    let mut entries = Vec::new();
    entries.try_reserve(request.targets.len())?;
    let mut total_nodes = 0_usize;
    let mut total_strings = 0_usize;
    for target in &request.targets {
        charge_strings(
            &mut total_strings,
            target.assembly.len()
                + target.namespace.len()
                + target.class_name.len()
                + target.unity_version.len(),
        )?;
        let version = target
            .unity_version
            .parse()
            .context("Invalid generator Unity version")?;
        let fields = serializer
            .generate(
                &target.assembly,
                &target.namespace,
                &target.class_name,
                &version,
            )
            .with_context(|| {
                format!(
                    "Reconstruct managed type {:?}::{:?} for {}",
                    target.namespace, target.class_name, target.unity_version
                )
            })?;
        let nodes = full_object_nodes(&fields, &mut total_nodes, &mut total_strings)?;
        entries.push(Entry {
            assembly: target.assembly.clone(),
            namespace: target.namespace.clone(),
            class_name: target.class_name.clone(),
            unity_version: target.unity_version.clone(),
            nodes,
        });
    }
    Ok(Document {
        version: 1,
        entries,
    })
}

fn assembly_name(name: &str) -> Result<String> {
    ensure!(
        !name.is_empty()
            && name.len() <= 240
            && !name.contains(['/', '\\', ':', '\0'])
            && name != "."
            && name != "..",
        "Assembly name must be one portable DLL file name"
    );
    Ok(if name.to_ascii_lowercase().ends_with(".dll") {
        name.to_owned()
    } else {
        format!("{name}.dll")
    })
}

fn assembly_index(directory: &Path) -> Result<HashMap<String, PathBuf>> {
    let mut index = HashMap::new();
    for (count, entry) in fs::read_dir(directory)?.enumerate() {
        ensure!(
            count < MAX_DIRECTORY_ENTRIES,
            "Managed folder has too many directory entries"
        );
        let entry = entry?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            continue;
        }
        ensure!(
            entry.file_type()?.is_file(),
            "Managed DLL must be a regular file: {path:?}"
        );
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Managed DLL file name is not UTF-8"))?;
        let name = assembly_name(&name)?.to_ascii_lowercase();
        ensure!(
            index.len() < MAX_DLL_FILES,
            "Managed folder has too many DLL files"
        );
        index.try_reserve(1)?;
        ensure!(
            index.insert(name, path).is_none(),
            "Managed folder contains ambiguous case-insensitive DLL names"
        );
    }
    Ok(index)
}

fn read_file(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file(),
        "Metadata input must be a regular file: {path:?}"
    );
    ensure!(
        metadata.len() <= maximum as u64,
        "Metadata input exceeds its {maximum} byte limit: {path:?}"
    );
    let mut file = File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Metadata input must be a regular file: {path:?}"
    );
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(bytes);
        }
        let next = bytes
            .len()
            .checked_add(count)
            .context("Managed metadata length overflow")?;
        ensure!(
            next <= maximum,
            "Metadata input exceeds its {maximum} byte limit: {path:?}"
        );
        bytes.try_reserve(count)?;
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn full_object_nodes(
    tree: &[TemplateField],
    total: &mut usize,
    strings: &mut usize,
) -> Result<Vec<Node>> {
    // The serializer returns script-defined fields. The provider must see
    // the complete player-object body, including Unity's engine-owned header.
    let mut nodes = vec![
        header_node(0, "MonoBehaviour", "Base", -1, 0),
        header_node(1, "PPtr<GameObject>", "m_GameObject", 12, 0),
        header_node(2, "int", "m_FileID", 4, 0),
        header_node(2, "SInt64", "m_PathID", 8, 0),
        header_node(1, "UInt8", "m_Enabled", 1, 0x4000),
        header_node(1, "PPtr<MonoScript>", "m_Script", 12, 0),
        header_node(2, "int", "m_FileID", 4, 0),
        header_node(2, "SInt64", "m_PathID", 8, 0),
        header_node(1, "string", "m_Name", -1, 0x4000),
        header_node(2, "Array", "Array", -1, 0x4000),
        header_node(3, "int", "size", 4, 0),
        header_node(3, "char", "data", 1, 0),
    ];
    for node in &nodes {
        charge_strings(strings, node.type_name.len() + node.name.len())?;
    }
    let mut stack = Vec::new();
    ensure!(
        tree.len() <= MAX_ENTRY_NODES - nodes.len(),
        "Generated schema has too many root fields"
    );
    stack.try_reserve(tree.len())?;
    for node in tree.iter().rev() {
        stack.push((node, 1_u32));
    }
    while let Some((node, level)) = stack.pop() {
        ensure!(
            level <= MAX_DEPTH,
            "Generated schema exceeds the depth limit"
        );
        ensure!(
            !node.ty.is_empty() && !node.name.is_empty(),
            "Managed field type cannot be resolved"
        );
        ensure!(
            node.ty.len() <= 64 * 1024 && node.name.len() <= 64 * 1024,
            "Generated schema field name exceeds its string limit"
        );
        ensure!(
            nodes.len() < MAX_ENTRY_NODES,
            "Generated schema has too many nodes"
        );
        charge_strings(strings, node.ty.len() + node.name.len())?;
        nodes.try_reserve(1)?;
        nodes.push(Node {
            level,
            type_name: node.ty.clone(),
            name: node.name.clone(),
            byte_size: node_size(node)?,
            meta_flags: if node.ty == "Array" {
                if array_element_aligned(node)? {
                    0x4000
                } else {
                    0
                }
            } else if node.aligned {
                0x4000
            } else {
                0
            },
            type_flags: i32::from(node.ty == "Array"),
        });
        let pending = nodes
            .len()
            .checked_add(stack.len())
            .and_then(|count| count.checked_add(node.children.len()))
            .context("Generated schema pending-node count overflow")?;
        ensure!(
            pending <= MAX_ENTRY_NODES,
            "Generated schema has too many pending nodes"
        );
        stack.try_reserve(node.children.len())?;
        for child in node.children.iter().rev() {
            stack.push((child, level + 1));
        }
    }
    *total = total
        .checked_add(nodes.len())
        .context("Generated schema node count overflow")?;
    ensure!(
        *total <= MAX_TOTAL_NODES,
        "Generated schema set has too many nodes"
    );
    Ok(nodes)
}

fn node_size(field: &TemplateField) -> Result<i32> {
    Ok(match field.ty.as_str() {
        "bool" | "UInt8" | "SInt8" | "char" => 1,
        "UInt16" | "SInt16" => 2,
        "int" | "SInt32" | "UInt32" | "unsigned int" | "float" => 4,
        "SInt64" | "UInt64" | "double" => 8,
        "string" | "Array" | "vector" | "map" => -1,
        _ => {
            let mut size = 0_i32;
            for child in &field.children {
                let next = node_size(child)?;
                if next < 0 {
                    return Ok(-1);
                }
                size = size
                    .checked_add(next)
                    .context("Generated fixed node size overflow")?;
            }
            size
        }
    })
}
fn is_pod(field: &TemplateField) -> Result<bool> {
    if matches!(
        field.ty.as_str(),
        "bool"
            | "UInt8"
            | "SInt8"
            | "char"
            | "UInt16"
            | "SInt16"
            | "int"
            | "SInt32"
            | "UInt32"
            | "unsigned int"
            | "float"
            | "SInt64"
            | "UInt64"
            | "double"
    ) {
        return Ok(true);
    }
    let size = node_size(field)?;
    if size <= 0 || size % 4 != 0 {
        return Ok(false);
    }
    for child in &field.children {
        if !is_pod(child)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn array_element_aligned(array: &TemplateField) -> Result<bool> {
    let Some(data) = array.children.iter().find(|node| node.name == "data") else {
        return Ok(false);
    };
    Ok(data.ty == "string" || is_pod(data)? || ends_aligned(data)?)
}

// A string, explicitly aligned field, or a structure ending in one already
// finishes at a four-byte boundary. An Array align flag is then redundant,
// and can safely preserve the reference generator's exact metadata flags.
fn ends_aligned(field: &TemplateField) -> Result<bool> {
    if field.aligned || field.ty == "string" || field.ty == "ReferencedObjectData" {
        return Ok(true);
    }
    if field.ty == "Array" {
        return array_element_aligned(field);
    }
    field.children.last().map_or(Ok(false), ends_aligned)
}

fn charge_strings(strings: &mut usize, amount: usize) -> Result<()> {
    let next = strings
        .checked_add(amount)
        .context("Generated schema string byte count overflow")?;
    ensure!(
        next <= MAX_SCHEMA_STRING_BYTES,
        "Generated schema exceeds its cumulative string byte limit"
    );
    *strings = next;
    Ok(())
}

fn header_node(level: u32, ty: &str, name: &str, size: i32, flags: i32) -> Node {
    Node {
        level,
        type_name: ty.into(),
        name: name.into(),
        byte_size: size,
        meta_flags: flags,
        type_flags: i32::from(ty == "Array"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assetstudio_core::schema::ManagedSchemaTarget;

    fn fixture_request(version: &str) -> ManagedSchemaRequest {
        ManagedSchemaRequest {
            version: 1,
            dll_directory: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/managed"),
            targets: vec![ManagedSchemaTarget {
                unity_version: version.into(),
                assembly: "Fixtures.dll".into(),
                namespace: "Fixtures".into(),
                class_name: "Primitives".into(),
            }],
        }
    }

    #[test]
    fn actual_dll_fields_match_independent_snapshots_for_three_unity_versions() {
        let classes = [
            "Primitives",
            "Enums",
            "Strings",
            "NestedEnumField",
            "Pointers",
            "SpecialTypes",
            "Arrays",
            "MultidimArrays",
            "Lists",
            "NestedSerializable",
            "FieldVisibility",
            "Inheritance",
            "ManagedRefs",
            "ScriptableRefs",
            "Generics",
            "GenericCollectionArg",
            "GenericInheritance",
            "TwoParamGeneric",
            "NestedTypes",
        ];
        for version in ["5.0.0f1", "2019.4.0f1", "6000.0.0f1"] {
            let mut request = fixture_request(version);
            request.targets = classes
                .iter()
                .map(|class| ManagedSchemaTarget {
                    unity_version: version.into(),
                    assembly: "Fixtures.dll".into(),
                    namespace: "Fixtures".into(),
                    class_name: (*class).into(),
                })
                .collect();
            request.targets.push(ManagedSchemaTarget {
                unity_version: version.into(),
                assembly: "UnityEngine.dll".into(),
                namespace: "UnityEngine".into(),
                class_name: "ForwardedAsset".into(),
            });
            let document = generate(&request).unwrap();
            assert_eq!(document.entries.len(), 20);
            for entry in &document.entries {
                let path = request.dll_directory.join(format!(
                    "snapshots/{version}/{}.{}.json",
                    entry.namespace, entry.class_name
                ));
                let reference: Vec<serde_json::Value> =
                    serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                let generated = &entry.nodes;
                assert_eq!(generated[0].type_name, "MonoBehaviour");
                assert_eq!(generated[1].name, "m_GameObject");
                assert_eq!(generated[8].name, "m_Name");
                assert_eq!(
                    generated.len() - 12,
                    reference.len() - 1,
                    "{} {version}",
                    entry.class_name
                );
                for (actual, expected) in generated[12..].iter().zip(&reference[1..]) {
                    assert_eq!(
                        actual.type_name, expected["m_Type"],
                        "{} {version}",
                        entry.class_name
                    );
                    assert_eq!(
                        actual.name, expected["m_Name"],
                        "{} {version}",
                        entry.class_name
                    );
                    assert_eq!(
                        actual.level,
                        expected["m_Level"].as_u64().unwrap() as u32,
                        "{} {version}",
                        entry.class_name
                    );
                    assert_eq!(
                        actual.meta_flags,
                        expected["m_MetaFlag"].as_i64().unwrap() as i32,
                        "{} {version} {:?}",
                        entry.class_name,
                        actual.name
                    );
                }
            }
            let bytes = serde_json::to_vec(&document).unwrap();
            assert_eq!(unity_rs_core_registry(&bytes), 20);
        }
    }

    fn unity_rs_core_registry(bytes: &[u8]) -> usize {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schema.json");
        fs::write(&path, bytes).unwrap();
        assetstudio_core::schema::load_registry(&path)
            .unwrap()
            .entries()
            .len()
    }

    #[test]
    fn missing_types_dependencies_and_invalid_pe_are_errors() {
        let mut request = fixture_request("6000.0.0f1");
        request.targets[0].class_name = "DefinitelyMissing".into();
        assert!(generate(&request).is_err());
        let directory = tempfile::tempdir().unwrap();
        fs::copy(
            fixture_request("6000.0.0f1")
                .dll_directory
                .join("Fixtures.dll"),
            directory.path().join("Fixtures.dll"),
        )
        .unwrap();
        request = fixture_request("6000.0.0f1");
        request.targets[0].class_name = "Pointers".into();
        request.dll_directory = directory.path().to_owned();
        assert!(
            generate(&request).is_err(),
            "missing engine dependency must not erase pointer fields"
        );
        fs::write(
            directory.path().join("Fixtures.dll"),
            b"MZ invalid PE metadata",
        )
        .unwrap();
        assert!(generate(&request).is_err());
    }

    #[test]
    fn paths_and_output_are_bounded() {
        for name in ["../other.dll", "C:\\other.dll", "/tmp/other.dll", "", ".."] {
            assert!(assembly_name(name).is_err());
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.dll");
        fs::write(&path, [0u8; 8]).unwrap();
        assert!(read_file(&path, 4).is_err());
        let mut bytes = Vec::new();
        let mut bounded = BoundedWriter::new(&mut bytes, 4);
        bounded.write_all(b"1234").unwrap();
        assert!(bounded.write_all(b"5").is_err());
        assert_eq!(bytes, b"1234");
    }

    #[test]
    fn renamed_dll_cannot_supply_a_different_assembly_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut request = fixture_request("6000.0.0f1");
        fs::copy(
            request.dll_directory.join("Fixtures.dll"),
            directory.path().join("Other.dll"),
        )
        .unwrap();
        request.dll_directory = directory.path().to_owned();
        request.targets[0].assembly = "Other.dll".into();
        let error = generate(&request).err().unwrap();
        assert!(format!("{error:#}").contains("manifest identity"));
    }
}
