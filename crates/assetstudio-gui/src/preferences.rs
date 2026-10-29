//! Bounded, atomic local preferences. Raw decryption keys are excluded by the UI.
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    data: Value,
}

#[derive(Serialize)]
struct DocumentRef<'a> {
    version: u32,
    data: &'a Value,
}

pub fn load() -> Result<Option<Value>, String> {
    load_from(&settings_path()?)
}

pub fn save(value: &Value) -> Result<(), String> {
    save_to(&settings_path()?, value)
}

pub fn settings_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "The Windows application settings folder is unavailable.".to_owned())?;
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library/Application Support"))
        .ok_or_else(|| "The application settings folder is unavailable.".to_owned())?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".config"))
        })
        .ok_or_else(|| "The application settings folder is unavailable.".to_owned())?;
    if !root.is_absolute() {
        return Err("The application settings folder must be an absolute path.".to_owned());
    }
    Ok(root.join("AssetStudio").join("settings.json"))
}

fn load_from(path: &Path) -> Result<Option<Value>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot inspect saved settings: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
        return Err("Saved settings must be a regular file smaller than 64 KiB.".to_owned());
    }
    let file = File::open(path).map_err(|error| format!("Cannot read saved settings: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read saved settings: {error}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Saved settings exceed 64 KiB.".to_owned());
    }
    let document: Document = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse saved settings: {error}"))?;
    if document.version != 1 || !document.data.is_object() {
        return Err("Saved settings use an unknown format.".to_owned());
    }
    Ok(Some(document.data))
}

struct Limited<'a> {
    output: &'a mut File,
    written: u64,
}

impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("Settings size overflow"))?;
        if end > MAX_BYTES {
            return Err(io::Error::other("Settings exceed 64 KiB"));
        }
        let count = self.output.write(bytes)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

fn save_to(path: &Path, value: &Value) -> Result<(), String> {
    if !value.is_object() {
        return Err("Settings must be an object.".to_owned());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Settings folder is missing.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create settings folder: {error}"))?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err("Refusing to replace a non-regular settings file.".to_owned());
        }
        Ok(_) => {
            // A malformed or newer file may belong to another application or
            // version. Showing defaults must not grant permission to destroy it.
            load_from(path)
                .map_err(|error| format!("Existing settings were preserved: {error}"))?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Cannot inspect settings file: {error}")),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Cannot prepare saved settings: {error}"))?;
    let mut output = Limited {
        output: temporary.as_file_mut(),
        written: 0,
    };
    serde_json::to_writer(
        &mut output,
        &DocumentRef {
            version: 1,
            data: value,
        },
    )
    .map_err(|error| format!("Cannot encode saved settings: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("Cannot flush saved settings: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("Cannot finish saved settings: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("Cannot publish saved settings: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_defaults_and_round_trip_preserve_preferences() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nested/settings.json");
        assert!(load_from(&path).unwrap().is_none());
        let data = serde_json::json!({"mode":"Raw","image_zoom":2.0,"future":{"value":1}});
        save_to(&path, &data).unwrap();
        assert_eq!(load_from(&path).unwrap(), Some(data));
        save_to(&path, &serde_json::json!({"mode":"Convert"})).unwrap();
        assert_eq!(load_from(&path).unwrap().unwrap()["mode"], "Convert");
    }

    #[test]
    fn malformed_or_newer_settings_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        for bytes in [
            b"not JSON".as_slice(),
            br#"{"version":2,"data":{}}"#,
            br#"{"version":1,"data":[]}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(load_from(&path).is_err());
            assert!(save_to(&path, &serde_json::json!({"mode":"Raw"})).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        assert!(load_from(root.path()).is_err());
    }

    #[test]
    fn oversized_save_keeps_previous_settings_and_cleans_temporary() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let original = serde_json::json!({"mode":"Raw"});
        save_to(&path, &original).unwrap();
        let large = serde_json::json!({"text":"x".repeat(70_000)});
        assert!(save_to(&path, &large).is_err());
        assert_eq!(load_from(&path).unwrap(), Some(original));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_settings_do_not_overwrite_an_unrelated_file() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::write(&target, b"original").unwrap();
        let path = root.path().join("settings.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(load_from(&path).is_err());
        assert!(save_to(&path, &serde_json::json!({})).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }
}
