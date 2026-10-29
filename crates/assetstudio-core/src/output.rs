use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

/// A bounded portable filename component. File-supplied paths never become absolute paths.
pub(crate) fn component(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        let ch = if ch.is_control()
            || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            '_'
        } else {
            ch
        };
        if result.len() + ch.len_utf8() > 140 {
            break;
        }
        result.push(ch);
    }
    result = result.trim_matches([' ', '.']).to_owned();
    if result.is_empty() {
        result.push('_');
    }
    let stem = result
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let serial = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || serial.is_some_and(|number| {
        matches!(
            number,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    });
    if reserved {
        result.insert(0, '_');
    }
    result
}

pub(crate) struct SafeOutput {
    root: PathBuf,
}
impl SafeOutput {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        // Resolve the explicit caller root once, including OS aliases such as
        // macOS's /var -> /private/var. Generated descendants stay checked.
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut existing = absolute.as_path();
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(
                        existing
                            .file_name()
                            .context("invalid output directory")?
                            .to_os_string(),
                    );
                    existing = existing
                        .parent()
                        .context("output directory has no existing ancestor")?;
                }
                Err(error) => return Err(error).context("inspect caller output directory"),
            }
        }
        let mut root = fs::canonicalize(existing).context("resolve caller output directory")?;
        for part in missing.into_iter().rev() {
            root.push(part);
        }
        ensure_directory(&root)?;
        Ok(Self { root })
    }
    pub(crate) fn directory(&self, relative: &Path) -> Result<PathBuf> {
        for part in relative.components() {
            if !matches!(part, Component::Normal(_)) {
                bail!("unsafe output directory {relative:?}");
            }
        }
        let path = self.root.join(relative);
        ensure_directory(&path)?;
        Ok(path)
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        bail!("output directory must not contain '..'");
    }
    // Iterating actual ancestors retains the root separator for Windows
    // verbatim drive/UNC prefixes; inspecting a bare `C:` prefix is incorrect.
    let ancestors: Vec<_> = absolute
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    for current in ancestors.into_iter().rev() {
        match fs::symlink_metadata(current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("output directory contains a symbolic link: {current:?}")
            }
            Ok(meta) if !meta.is_dir() => bail!("output parent is not a directory: {current:?}"),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(current)
                    .with_context(|| format!("create output directory {current:?}"))?;
            }
            Err(error) => return Err(error).context("inspect output directory"),
        }
    }
    Ok(())
}

/// Writes beside the destination and publishes atomically; false means no-clobber skip.
pub(crate) fn atomic_write(
    path: &Path,
    overwrite: bool,
    write: impl FnOnce(&mut fs::File) -> Result<()>,
) -> Result<bool> {
    let parent = path.parent().context("output has no parent")?;
    ensure_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            bail!("refusing non-regular output target {path:?}")
        }
        Ok(_) if !overwrite => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect output target"),
    }
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("create temporary output")?;
    write(temporary.as_file_mut())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    if overwrite {
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .context("publish output")?;
    } else {
        match temporary.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(false);
            }
            Err(error) => return Err(error.error).context("publish output without overwrite"),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_names_and_utf8_limits() {
        for name in [
            "..",
            "",
            "CON",
            "nul.txt",
            "COM1",
            "a/b:c",
            "... ",
            "😀".repeat(100).as_str(),
        ] {
            let sanitized = component(name);
            assert!(!sanitized.is_empty());
            assert!(sanitized.len() <= 141);
            assert!(!sanitized.contains(['/', '\\', ':']));
        }
        assert_eq!(component("CON.txt"), "_CON.txt");
        assert_eq!(component("CON .txt"), "_CON .txt");
        assert_eq!(component("LPT1 .bin"), "_LPT1 .bin");
        assert_eq!(component("COM¹.foo"), "_COM¹.foo");
        assert_eq!(component("LPT²"), "_LPT²");
    }
    #[test]
    fn atomic_no_clobber_and_failure_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(dir.path()).unwrap().join("x");
        assert!(
            atomic_write(&path, false, |out| {
                out.write_all(b"first")?;
                Ok(())
            })
            .unwrap()
        );
        assert!(!atomic_write(&path, false, |_| panic!("must not execute writer")).unwrap());
        assert!(
            atomic_write(&path, true, |out| {
                out.write_all(b"bad")?;
                bail!("failure")
            })
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), b"first");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn symlink_directory_and_file_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let output = SafeOutput::new(dir.path()).unwrap();
        assert!(output.directory(Path::new("link")).is_err());
        assert_eq!(
            SafeOutput::new(&dir.path().join("link")).unwrap().root(),
            fs::canonicalize(outside.path()).unwrap()
        );
        std::os::unix::fs::symlink(outside.path().join("x"), dir.path().join("x")).unwrap();
        assert!(atomic_write(&dir.path().join("x"), true, |_| Ok(())).is_err());
    }
}
