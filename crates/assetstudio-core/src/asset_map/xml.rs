use std::collections::HashMap;
use std::io::Write;

use anyhow::{Context, Result, bail, ensure};
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

use super::{AssetMap, AssetMapEntry, MAX_ENTRIES, MAX_STRING_BYTES, game_id, normalize_source};
use crate::{class_id, class_name};

#[derive(Default)]
struct Entry {
    name: Option<String>,
    container: Option<String>,
    source: Option<String>,
    kind: Option<String>,
    path_id: Option<i64>,
    class_id: Option<i32>,
    file_index: Option<usize>,
    size: u64,
    serialized_source: Option<String>,
}
impl Entry {
    fn field(
        &mut self,
        name: &str,
        value: String,
        attributes: &HashMap<String, String>,
    ) -> Result<()> {
        match name {
            "Name" => set(&mut self.name, value, "Name")?,
            "Container" => set(&mut self.container, value, "Container")?,
            "Source" => set(&mut self.source, value, "Source")?,
            "Type" => {
                if let Some(id) = attributes.get("id") {
                    let id = id.parse::<i32>()?;
                    ensure!(
                        self.class_id.is_none_or(|previous| previous == id),
                        "conflicting XML class IDs"
                    );
                    self.class_id = Some(id);
                }
                set(&mut self.kind, value, "Type")?;
            }
            "PathID" => {
                let id = value.trim().parse::<i64>()?;
                ensure!(
                    self.path_id.is_none_or(|previous| previous == id),
                    "conflicting XML PathIDs"
                );
                self.path_id = Some(id);
            }
            _ => bail!("unknown asset map XML field {name:?}"),
        }
        Ok(())
    }
    fn finish(self) -> Result<AssetMapEntry> {
        let kind = self.kind.unwrap_or_default();
        let known = if kind.trim().is_empty() {
            None
        } else {
            class_id(kind.trim())
        };
        ensure!(
            self.class_id.is_none() || known.is_none() || self.class_id == known,
            "XML Type id and name disagree"
        );
        let id = self
            .class_id
            .or(known)
            .context("asset map XML entry has no known Type/classId")?;
        let type_name = if kind.trim().is_empty() || kind.trim().parse::<i32>().is_ok() {
            class_name(id).to_owned()
        } else {
            kind
        };
        let mut source = self.source.context("asset map XML entry has no Source")?;
        let mut serialized_source = self.serialized_source;
        normalize_source(&mut source, &mut serialized_source);
        Ok(AssetMapEntry {
            name: self.name.context("asset map XML entry has no Name")?,
            container: self.container.unwrap_or_default(),
            source,
            path_id: self.path_id.context("asset map XML entry has no PathID")?,
            class_id: id,
            class_name: type_name,
            file_index: self.file_index,
            size: self.size,
            serialized_source,
        })
    }
}
fn set(target: &mut Option<String>, value: String, label: &str) -> Result<()> {
    ensure!(target.is_none(), "duplicate XML {label} field");
    *target = Some(value);
    Ok(())
}

pub(super) fn read(bytes: &[u8]) -> Result<AssetMap> {
    std::str::from_utf8(bytes).context("asset map XML must be UTF-8")?;
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().check_end_names = true;
    let mut depth = 0usize;
    let mut opened = false;
    let mut closed = false;
    let mut declaration = false;
    let mut game = None;
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    let mut field: Option<(String, String, HashMap<String, String>)> = None;
    let mut events = 0usize;
    loop {
        events += 1;
        ensure!(
            events <= MAX_ENTRIES * 24 + 100,
            "asset map XML exceeds its event limit"
        );
        match reader.read_event().context("invalid asset map XML")? {
            Event::Start(element) => {
                let name = element_name(&element)?;
                let attributes = attributes(&element, &reader)?;
                match depth {
                    0 => {
                        ensure!(
                            !opened && !closed && name == "Assets",
                            "asset map XML root must be one Assets element"
                        );
                        opened = true;
                        game = root_game(&attributes)?;
                    }
                    1 => {
                        ensure!(
                            name == "Asset" && current.is_none(),
                            "unexpected element under XML Assets"
                        );
                        ensure!(
                            entries.len() < MAX_ENTRIES,
                            "asset map exceeds 200,000 entries"
                        );
                        current = Some(entry_attributes(&attributes)?);
                    }
                    2 => {
                        ensure!(
                            matches!(
                                name.as_str(),
                                "Name" | "Container" | "Type" | "PathID" | "Source"
                            ),
                            "unknown XML Asset field"
                        );
                        ensure!(field.is_none(), "nested asset map field");
                        field = Some((name, String::new(), attributes));
                    }
                    _ => bail!("asset map XML nesting exceeds its known layout"),
                }
                depth += 1;
            }
            Event::Empty(element) => {
                let name = element_name(&element)?;
                let attributes = attributes(&element, &reader)?;
                match depth {
                    0 => {
                        ensure!(
                            !opened && !closed && name == "Assets",
                            "asset map XML root must be Assets"
                        );
                        opened = true;
                        closed = true;
                        game = root_game(&attributes)?;
                    }
                    2 => current.as_mut().context("XML field has no Asset")?.field(
                        &name,
                        String::new(),
                        &attributes,
                    )?,
                    _ => bail!("empty asset map XML element lacks required fields"),
                }
            }
            Event::End(element) => {
                let name = std::str::from_utf8(element.name().as_ref())?.to_owned();
                match depth {
                    3 => {
                        let (expected, value, attributes) = field
                            .take()
                            .context("asset map XML closing field has no opener")?;
                        ensure!(name == expected, "mismatched XML field");
                        current.as_mut().context("XML field has no Asset")?.field(
                            &name,
                            value,
                            &attributes,
                        )?;
                    }
                    2 => {
                        ensure!(name == "Asset", "mismatched XML Asset close");
                        entries.try_reserve(1)?;
                        entries.push(
                            current
                                .take()
                                .context("XML Asset close has no opener")?
                                .finish()?,
                        );
                    }
                    1 => {
                        ensure!(name == "Assets", "mismatched XML root close");
                        closed = true;
                    }
                    _ => bail!("unexpected XML closing element"),
                }
                depth -= 1;
            }
            Event::Text(text) => {
                let value = text.decode()?;
                if let Some((_, target, _)) = field.as_mut() {
                    append(target, &value)?;
                } else {
                    ensure!(
                        value
                            .bytes()
                            .all(|value| matches!(value, b' ' | b'\t' | b'\r' | b'\n')),
                        "asset map XML has text outside a field"
                    );
                }
            }
            Event::CData(text) => {
                let value = text.decode()?;
                let (_, target, _) = field.as_mut().context("CDATA outside an asset map field")?;
                append(target, &value)?;
            }
            Event::GeneralRef(reference) => {
                let reference = reference.decode()?;
                ensure!(reference.len() <= 32, "XML reference is too long");
                let reference = format!("&{reference};");
                let value = quick_xml::escape::unescape(&reference)
                    .context("unknown XML entity reference")?;
                let (_, target, _) = field
                    .as_mut()
                    .context("XML reference outside an asset map field")?;
                append(target, &value)?;
            }
            Event::DocType(_) => {
                bail!("DOCTYPE and external XML entity definitions are forbidden in asset maps")
            }
            Event::PI(_) => bail!("XML processing instructions are not accepted in asset maps"),
            Event::Decl(value) => {
                ensure!(!opened && !declaration, "unexpected XML declaration");
                declaration = true;
                ensure!(
                    value.version()?.as_ref() == b"1.0",
                    "only XML 1.0 asset maps are implemented"
                );
                if let Some(encoding) = value.encoding() {
                    let encoding = encoding?;
                    ensure!(
                        encoding.eq_ignore_ascii_case(b"utf-8")
                            || encoding.eq_ignore_ascii_case(b"us-ascii"),
                        "asset map XML encoding must be UTF-8"
                    );
                }
            }
            Event::Comment(_) => {}
            Event::Eof => {
                ensure!(
                    opened && closed && depth == 0 && current.is_none() && field.is_none(),
                    "asset map XML is incomplete"
                );
                break;
            }
        }
    }
    AssetMap::new(entries, game)
}

fn element_name(element: &BytesStart<'_>) -> Result<String> {
    let name = element.name();
    ensure!(name.as_ref().len() <= 64, "XML element name is too long");
    Ok(std::str::from_utf8(name.as_ref())?.to_owned())
}
fn attributes(element: &BytesStart<'_>, reader: &Reader<&[u8]>) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    for (index, attribute) in element.attributes().enumerate() {
        ensure!(index < 32, "too many XML attributes");
        let attribute = attribute?;
        ensure!(
            attribute.key.as_ref().len() <= 64 && attribute.value.len() <= MAX_STRING_BYTES * 6,
            "XML attribute exceeds its limit"
        );
        let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Explicit1_0, reader.decoder())?;
        ensure!(
            value.len() <= MAX_STRING_BYTES,
            "decoded XML attribute exceeds 4096 bytes"
        );
        result.insert(name, value.into_owned());
    }
    Ok(result)
}
fn root_game(attributes: &HashMap<String, String>) -> Result<Option<i32>> {
    attributes
        .get("gameType")
        .or_else(|| attributes.get("GameType"))
        .map(|value| {
            value
                .parse::<i32>()
                .ok()
                .or_else(|| game_id(value))
                .context("invalid XML gameType")
        })
        .transpose()
}
fn entry_attributes(attributes: &HashMap<String, String>) -> Result<Entry> {
    let mut entry = Entry::default();
    if let Some(value) = attributes.get("file") {
        entry.file_index = Some(value.parse()?);
    }
    if let Some(value) = attributes.get("pathId") {
        entry.path_id = Some(value.parse()?);
    }
    if let Some(value) = attributes.get("classId") {
        entry.class_id = Some(value.parse()?);
    }
    if let Some(value) = attributes.get("size") {
        entry.size = value.parse()?;
    }
    if let Some(value) = attributes.get("serializedSource") {
        entry.serialized_source = Some(value.clone());
    }
    Ok(entry)
}
fn append(target: &mut String, value: &str) -> Result<()> {
    ensure!(
        target
            .len()
            .checked_add(value.len())
            .is_some_and(|length| length <= MAX_STRING_BYTES),
        "asset map XML field exceeds 4096 bytes"
    );
    ensure!(value.chars().all(|ch|matches!(ch,'\u{9}'|'\u{a}'|'\u{d}'|'\u{20}'..='\u{d7ff}'|'\u{e000}'..='\u{fffd}'|'\u{10000}'..='\u{10ffff}')),"invalid XML 1.0 character");
    target.try_reserve(value.len())?;
    target.push_str(value);
    Ok(())
}

pub(super) fn write(map: &AssetMap, output: &mut impl Write) -> Result<()> {
    writeln!(output, "<?xml version=\"1.0\" encoding=\"utf-8\"?>")?;
    if let Some(game) = map.game_type {
        writeln!(output, "<Assets gameType=\"{game}\">")?;
    } else {
        writeln!(output, "<Assets>")?;
    }
    for entry in &map.entries {
        write!(output, "  <Asset")?;
        if let Some(source) = &entry.serialized_source {
            write!(output, " serializedSource=\"")?;
            escape(output, source)?;
            write!(output, "\"")?;
        }
        writeln!(output, ">")?;
        for (tag, value) in [
            ("Name", entry.name.as_str()),
            ("Container", entry.container.as_str()),
        ] {
            write!(output, "    <{tag}>")?;
            escape(output, value)?;
            writeln!(output, "</{tag}>")?;
        }
        write!(output, "    <Type id=\"{}\">", entry.class_id)?;
        escape(output, &entry.class_name)?;
        writeln!(output, "</Type>\n    <PathID>{}</PathID>", entry.path_id)?;
        write!(output, "    <Source>")?;
        escape(output, &entry.source)?;
        writeln!(output, "</Source>\n  </Asset>")?;
    }
    writeln!(output, "</Assets>")?;
    Ok(())
}
fn escape(output: &mut impl Write, value: &str) -> Result<()> {
    for ch in value.chars() {
        match ch {
            '&' => output.write_all(b"&amp;")?,
            '<' => output.write_all(b"&lt;")?,
            '>' => output.write_all(b"&gt;")?,
            '"' => output.write_all(b"&quot;")?,
            '\'' => output.write_all(b"&apos;")?,
            '\u{9}'
            | '\u{a}'
            | '\u{d}'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => {
                let mut bytes = [0; 4];
                output.write_all(ch.encode_utf8(&mut bytes).as_bytes())?;
            }
            _ => bail!("asset map string cannot be represented as XML 1.0"),
        }
    }
    Ok(())
}
