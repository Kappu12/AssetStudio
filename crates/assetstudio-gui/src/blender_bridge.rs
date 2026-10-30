//! Local file-queue protocol shared with the standalone Blender extension.
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use assetstudio_core::{AssetKey, ModelExportOptions, OperationReport, Session};
use serde_json::{Value, json};

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
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let Some(time) = value["time"].as_f64() else {
                continue;
            };
            let Some(id) = value["receiver"].as_str() else {
                continue;
            };
            if value["version"] == 1
                && (0.0..=8.0).contains(&(now - time))
                && id.len() == 32
                && id.bytes().all(|b| b.is_ascii_hexdigit())
            {
                receivers.push((time, id.to_owned()));
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
    if receiver(&root)? != receiver_id {
        bail!(
            "The active Blender receiver changed during export. The model package is saved at {}. Retry with the intended Blender window open.",
            job.display()
        );
    }
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
            let result: Value =
                serde_json::from_slice(&bytes).context("Read Blender import receipt")?;
            if result["version"] != 1 {
                bail!("Unsupported Blender bridge receipt");
            }
            if let Some(failures) = result["failed"].as_array() {
                report
                    .failed
                    .extend(failures.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            let imported = result["imported"]
                .as_array()
                .context("Missing Blender import result")?;
            for model in imported {
                if let Some(warnings) = model["warnings"].as_array() {
                    report
                        .skipped
                        .extend(warnings.iter().filter_map(Value::as_str).map(str::to_owned));
                }
            }
            progress.update("Imported into Blender", imported.len(), Some(models.len()));
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
        let path = root.path().join("receivers/test.json");
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
    }
}
