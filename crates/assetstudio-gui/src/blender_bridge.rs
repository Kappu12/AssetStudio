//! Local file-queue protocol shared with the standalone Blender extension.
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use assetstudio_core::{AssetKey, ModelExportOptions, OperationReport, Session};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Receiver {
    version: u32,
    receiver: String,
    time: f64,
    started: Option<f64>,
}

fn read_receiver(path: &std::path::Path, now: f64) -> Option<Receiver> {
    let value: Receiver = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (value.version == 1
        && path
            .extension()
            .is_some_and(|extension| extension == "json")
        && (0.0..=8.0).contains(&(now - value.time))
        && value.receiver.len() == 32
        && value.receiver.bytes().all(|b| b.is_ascii_hexdigit())
        && path
            .file_stem()
            .is_some_and(|stem| stem == value.receiver.as_str()))
    .then_some(value)
}

fn ensure_receiver(root: &std::path::Path, id: &str) -> Result<()> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    if read_receiver(&root.join("receivers").join(format!("{id}.json")), now).is_none() {
        bail!(
            "The selected Blender window stopped responding during export. Reopen Blender and retry."
        );
    }
    Ok(())
}

#[derive(Deserialize)]
struct ImportedModel {
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct Receipt {
    version: u32,
    imported: Vec<ImportedModel>,
    failed: Vec<String>,
}

fn apply_receipt(bytes: &[u8], expected: usize, report: &mut OperationReport) -> Result<usize> {
    let receipt: Receipt = serde_json::from_slice(bytes).context("Read Blender import receipt")?;
    if receipt.version != 1 {
        bail!("Unsupported Blender bridge receipt");
    }
    if receipt.failed.is_empty() && receipt.imported.len() != expected {
        bail!(
            "Blender acknowledged {} of {expected} models without reporting the missing imports",
            receipt.imported.len()
        );
    }
    let imported = receipt.imported.len();
    report.failed.extend(receipt.failed);
    report.skipped.extend(
        receipt
            .imported
            .into_iter()
            .flat_map(|model| model.warnings),
    );
    Ok(imported)
}

fn root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ASSETSTUDIO_BLENDER_BRIDGE") {
        return Ok(path.into());
    }
    let cache = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .context("Cannot find the local Blender bridge folder")?;
    Ok(cache.join("AssetStudio").join("BlenderBridge"))
}

fn receiver(root: &std::path::Path) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    let mut receivers = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("receivers")) {
        for entry in entries.flatten() {
            if let Some(value) = read_receiver(&entry.path(), now) {
                receivers.push((
                    value
                        .started
                        .filter(|started| started.is_finite())
                        .unwrap_or(value.time),
                    value.receiver,
                ));
            }
        }
    }
    receivers.sort_by(|a, b| b.0.total_cmp(&a.0));
    receivers.into_iter().next().map(|(_,id)| id).context("Open Blender and enable AssetStudio Rust Bridge first. Install assetstudio_bridge.zip from the Build folder using Blender Preferences → Get Extensions → Install from Disk.")
}

pub fn send(
    session: &Session,
    roots: &[AssetKey],
    options: &ModelExportOptions,
    progress: &crate::progress::Progress,
) -> Result<OperationReport> {
    let root = root()?;
    let receiver_id = receiver(&root)?;
    fs::create_dir_all(root.join("jobs"))?;
    let job = tempfile::Builder::new()
        .prefix("model-")
        .tempdir_in(root.join("jobs"))?
        .keep();
    // SafeOutput publishes canonical Windows paths (including the long-path
    // prefix). Use the same representation when deriving queue-relative paths.
    let job = fs::canonicalize(job)?;
    let mut report =
        session.export_blender_models(&job, roots, options, Some(&progress.observer()))?;
    let models: Vec<_> = report
        .written
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "fbx"))
        .map(|path| {
            path.strip_prefix(&job)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect::<std::result::Result<_, _>>()?;
    if models.is_empty() {
        bail!("No model was exported: {}", report.failed.join("; "));
    }
    // A receiver must still be running when the potentially lengthy export ends.
    ensure_receiver(&root, &receiver_id)
        .with_context(|| format!("The model package is saved at {}", job.display()))?;
    let temporary = job.join("request.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&json!({"version":1,"receiver":receiver_id,"models":models}))?,
    )?;
    fs::rename(temporary, job.join("request.ready.json"))?;
    progress.update("Importing into Blender", 0, None);
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(job.join("result.json")) {
            let imported = apply_receipt(&bytes, models.len(), &mut report)?;
            progress.update("Imported into Blender", imported, Some(models.len()));
            return Ok(report);
        }
        if let Ok(bytes) = fs::read(job.join("progress.json"))
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            progress.update(
                "Importing into Blender",
                value["completed"].as_u64().unwrap_or(0) as usize,
                Some(models.len()),
            );
        }
        if started.elapsed() > Duration::from_secs(600) {
            let cancelled = fs::rename(
                job.join("request.ready.json"),
                job.join("request.cancelled.json"),
            )
            .is_ok();
            bail!(
                "{} Model files and import receipt are in {}.",
                if cancelled {
                    "Blender did not receive the export within ten minutes; the queued request was cancelled."
                } else {
                    "Blender is still processing this export. Check Blender before sending it again."
                },
                job.display()
            );
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receiver_requires_a_recent_compatible_heartbeat() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("receivers")).unwrap();
        let path = root
            .path()
            .join("receivers/01234567890123456789012345678901.json");
        fs::write(
            &path,
            r#"{"version":1,"receiver":"01234567890123456789012345678901","time":0}"#,
        )
        .unwrap();
        assert!(receiver(root.path()).is_err());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        fs::write(
            &path,
            serde_json::to_vec(
                &json!({"version":1,"receiver":"01234567890123456789012345678901","time":now}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            receiver(root.path()).unwrap(),
            "01234567890123456789012345678901"
        );
        fs::rename(&path, path.with_extension("tmp")).unwrap();
        assert!(
            receiver(root.path()).is_err(),
            "unpublished heartbeats must be ignored"
        );
    }

    #[test]
    fn receiver_stays_pinned_when_another_window_starts_or_heartbeats() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("receivers")).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let first = "01234567890123456789012345678901";
        let second = "11234567890123456789012345678901";
        for (id, started, heartbeat) in [(first, now - 10.0, now), (second, now - 5.0, now - 1.0)] {
            fs::write(
                root.path().join("receivers").join(format!("{id}.json")),
                serde_json::to_vec(
                    &json!({"version":1,"receiver":id,"started":started,"time":heartbeat}),
                )
                .unwrap(),
            )
            .unwrap();
        }
        assert_eq!(receiver(root.path()).unwrap(), second);
        assert!(ensure_receiver(root.path(), first).is_ok());
        assert!(ensure_receiver(root.path(), second).is_ok());
        fs::remove_file(root.path().join("receivers").join(format!("{first}.json"))).unwrap();
        assert!(ensure_receiver(root.path(), first).is_err());
    }

    #[test]
    fn incomplete_or_malformed_receipts_cannot_report_success() {
        for bytes in [
            r#"{"version":1,"imported":[],"failed":[]}"#,
            r#"{"version":1,"imported":[{}]}"#,
            r#"{"version":1,"imported":[{}],"failed":[1]}"#,
        ] {
            assert!(apply_receipt(bytes.as_bytes(), 1, &mut OperationReport::default()).is_err());
        }
        let mut report = OperationReport::default();
        assert_eq!(
            apply_receipt(
                br#"{"version":1,"imported":[{"warnings":["ambiguous map"]}],"failed":[]}"#,
                1,
                &mut report
            )
            .unwrap(),
            1
        );
        assert_eq!(report.skipped, ["ambiguous map"]);
        assert_eq!(
            apply_receipt(
                br#"{"version":1,"imported":[],"failed":["invalid model"]}"#,
                1,
                &mut report
            )
            .unwrap(),
            0
        );
        assert_eq!(report.failed, ["invalid model"]);
    }
}
