//! Protects input files even when a destination uses a symlink or hardlink alias.
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Default)]
pub(crate) struct SourceProtection {
    paths: HashSet<PathBuf>,
    identities: HashSet<u64>,
    retained_path_bytes: usize,
}
impl SourceProtection {
    pub(crate) fn include_input(&mut self, input: &Path) -> Result<()> {
        let canonical = fs::canonicalize(input)
            .with_context(|| format!("resolve protected input {input:?}"))?;
        let mut pending_bytes = canonical.as_os_str().as_encoded_bytes().len();
        let mut pending = vec![(canonical, 0usize)];
        let mut entries = 0usize;
        let mut discovered = 1usize;
        while let Some((path, depth)) = pending.pop() {
            pending_bytes = pending_bytes
                .checked_sub(path.as_os_str().as_encoded_bytes().len())
                .context("queued source path accounting underflow")?;
            if depth > 256 {
                bail!("input traversal exceeds source protection depth limit");
            }
            entries = entries
                .checked_add(1)
                .context("source protection count overflow")?;
            if entries > 1_000_000 {
                bail!("input traversal exceeds source protection file limit");
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                if path.is_file() {
                    self.include_file(&path)?;
                }
                continue;
            }
            if metadata.is_dir() {
                for entry in fs::read_dir(&path)? {
                    discovered = discovered
                        .checked_add(1)
                        .context("source discovery count overflow")?;
                    if discovered > 1_000_000 {
                        bail!("input source discovery exceeds file limit");
                    }
                    let next = entry?.path();
                    pending_bytes = pending_bytes
                        .checked_add(next.as_os_str().as_encoded_bytes().len())
                        .context("queued source path size overflow")?;
                    if pending_bytes > 256 * 1024 * 1024 {
                        bail!("queued input source paths exceed byte limit");
                    }
                    pending.try_reserve(1)?;
                    pending.push((next, depth + 1));
                }
            } else if metadata.is_file() {
                self.include_file(&path)?;
            } else if depth == 0 {
                bail!("input is not a regular file or directory: {path:?}");
            }
        }
        self.include_split_companions(input)?;
        Ok(())
    }

    pub(crate) fn include_label(&mut self, label: &str) -> Result<()> {
        let mut outer = label;
        loop {
            let path = Path::new(outer);
            if path.is_file() {
                self.include_file(path)?;
                return self.include_split_companions(path);
            }
            let Some((parent, _)) = outer.rsplit_once("::") else {
                return self.include_split_companions(path);
            };
            outer = parent;
        }
    }

    fn include_split_companions(&mut self, input: &Path) -> Result<()> {
        let Some(name) = input.file_name().and_then(|value| value.to_str()) else {
            return Ok(());
        };
        let base = if let Some((base, suffix)) = name.rsplit_once(".split") {
            if suffix.is_empty() || !suffix.bytes().all(|ch| ch.is_ascii_digit()) {
                return Ok(());
            }
            base
        } else if !input.exists() {
            name
        } else {
            return Ok(());
        };
        let parent = input
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Ok(());
        }
        let mut entries = 0usize;
        for entry in fs::read_dir(parent)? {
            entries += 1;
            if entries > 1_000_000 {
                bail!("split companion discovery exceeds file limit");
            }
            let entry = entry?;
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            let Some(suffix) = filename
                .strip_prefix(base)
                .and_then(|value| value.strip_prefix(".split"))
            else {
                continue;
            };
            if !suffix.is_empty()
                && suffix.bytes().all(|ch| ch.is_ascii_digit())
                && entry.file_type()?.is_file()
            {
                self.include_file(&entry.path())?;
            }
        }
        Ok(())
    }

    pub(crate) fn include_file(&mut self, path: &Path) -> Result<()> {
        let path =
            fs::canonicalize(path).with_context(|| format!("resolve protected source {path:?}"))?;
        if self.paths.contains(&path) {
            return Ok(());
        }
        let next = self
            .retained_path_bytes
            .checked_add(path.as_os_str().as_encoded_bytes().len())
            .context("protected path size overflow")?;
        if next > 256 * 1024 * 1024 || self.paths.len() >= 1_000_000 {
            bail!("protected input metadata exceeds limit");
        }
        let identity = identity(&path)?;
        self.paths.try_reserve(1)?;
        self.identities.try_reserve(1)?;
        self.paths.insert(path);
        self.identities.insert(identity);
        self.retained_path_bytes = next;
        Ok(())
    }

    pub(crate) fn check(&self, target: &Path) -> Result<()> {
        // A missing source path may have been deleted since loading. It still
        // must not be re-created by an export that resembles its former name.
        if self.paths.contains(target) {
            bail!("refusing to overwrite original input {target:?}");
        }
        // Do not open a FIFO/device while querying identity: that can block.
        // Publication also rejects these, but the identity check runs first.
        match fs::symlink_metadata(target) {
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                bail!("output is not a regular file: {target:?}")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect output file kind"),
        }
        match fs::canonicalize(target) {
            Ok(canonical) => {
                if self.paths.contains(&canonical)
                    || self.identities.contains(&identity(&canonical)?)
                {
                    bail!("refusing to overwrite an original input or its file alias: {target:?}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect output identity"),
        }
        Ok(())
    }
}

fn identity(path: &Path) -> Result<u64> {
    let handle = same_file::Handle::from_path(path)
        .with_context(|| format!("read file identity {path:?}"))?;
    let mut hash = DefaultHasher::new();
    handle.hash(&mut hash);
    Ok(hash.finish())
    // Fingerprint collisions conservatively refuse an unrelated destination;
    // they can never permit an input overwrite. Handles are dropped immediately.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_and_hardlink_alias_are_protected_without_blocking_new_outputs() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::write(&source, b"original").unwrap();
        let alias = root.path().join("alias");
        fs::hard_link(&source, &alias).unwrap();
        let mut guard = SourceProtection::default();
        guard.include_input(&source).unwrap();
        assert!(guard.check(&source).is_err());
        assert!(guard.check(&alias).is_err());
        assert!(guard.check(&root.path().join("new")).is_ok());
        fs::remove_file(&source).unwrap();
        assert!(guard.check(&source).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn fifo_output_is_rejected_before_an_identity_open_can_block() {
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let guard = SourceProtection::default();
        assert!(guard.check(&fifo).is_err());
        let mut guard = SourceProtection::default();
        assert!(guard.include_input(&fifo).is_err());
    }
}
