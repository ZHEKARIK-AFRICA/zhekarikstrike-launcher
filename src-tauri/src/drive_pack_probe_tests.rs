//! Explicit opt-in, disposable, real-Drive ABBA experiment. Never built into the launcher.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::Digest;
use tokio_util::sync::CancellationToken;

use crate::error::AppError;
use crate::models::{ContentFile, DrivePackManifest};
use crate::services::api_client::ApiClient;
use crate::services::content_pack_install_service::run_packed_probe;
use crate::services::content_pack_metrics::{PackProfile, PackRunOptions, ProbeBudget, Snapshot};
use crate::services::content_pack_plan_service::{
    plan_pack_fetches, PackFetchPlan, PackTransferMode,
};

const MIB: u64 = 1024 * 1024;
const TOTAL_BUDGET: u64 = 2048 * MIB;
const ROOT: &str = r"D:\zhekarik-adaptive-pack-probe";
const OWNER: &str = "adaptive-pack-probe-v1";
const FROZEN_SERIES: &str =
    r"D:\zhekarik-adaptive-pack-probe\series-442710e6-dffe-4da2-971f-58e9008523da";
const EXPECTED_MANIFEST: &str = "ac4ab8a152e3e0371c7a61f77f4e531d5ed163f7283ea5672c1fe47f182cc488";
const EXPECTED_CONTENT: &str = "01a13dfb3448ce6c55ec2051d70ad61775cbe1c2fa322330542d3b879d9675db";

fn failure(message: &str) -> AppError {
    AppError::InvalidData(message.into())
}

fn safe_directory(path: &Path) -> Result<(), AppError> {
    for ancestor in path.ancestors() {
        if !ancestor.exists() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(failure("probe directory has an unsafe ancestor"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(failure("probe refuses reparse-point directories"));
            }
        }
    }
    Ok(())
}

fn assert_owned_tree(path: &Path) -> Result<(), AppError> {
    safe_directory(path)?;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(failure("probe cleanup refuses reparse points"));
            }
        }
        if metadata.file_type().is_symlink() {
            return Err(failure("probe cleanup refuses symbolic links"));
        }
        if metadata.is_dir() {
            assert_owned_tree(&entry.path())?;
        }
    }
    Ok(())
}

fn cleanup_work(series: &Path, work: &Path, marker: &str) -> Result<(), AppError> {
    safe_directory(series)?;
    if work.parent() != Some(series)
        || !work
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("work-")
    {
        return Err(failure(
            "probe cleanup target is not a direct owned work directory",
        ));
    }
    if std::fs::read_to_string(series.join("owner.txt"))? != marker
        || std::fs::read_to_string(work.join("owner.txt"))? != marker
    {
        return Err(failure("probe cleanup owner marker mismatch"));
    }
    let resolved_series = series.canonicalize()?;
    let resolved_work = work.canonicalize()?;
    if resolved_work.parent() != Some(resolved_series.as_path()) {
        return Err(failure("probe cleanup escaped its series"));
    }
    assert_owned_tree(work)?;
    std::fs::remove_dir_all(&resolved_work)?;
    Ok(())
}

fn select_files(
    manifest: &DrivePackManifest,
    prior: &Value,
) -> Result<(Vec<ContentFile>, Vec<PackFetchPlan>, u64), AppError> {
    manifest.validate()?;
    let paths = prior["files"]
        .as_array()
        .ok_or_else(|| failure("prior report files missing"))?;
    let mut selected = Vec::with_capacity(paths.len());
    let mut seen = HashSet::new();
    for path in paths {
        let path = path
            .as_str()
            .ok_or_else(|| failure("invalid prior file path"))?;
        if !seen.insert(path) {
            return Err(failure("duplicate prior file"));
        }
        selected.push(
            manifest
                .files
                .iter()
                .find(|f| f.path == path)
                .cloned()
                .ok_or_else(|| failure("prior file missing from manifest"))?,
        );
    }
    let chunks = selected
        .iter()
        .flat_map(|file| file.chunks.iter().cloned())
        .collect::<Vec<_>>();
    let plans = plan_pack_fetches(manifest, &chunks)?;
    let mut planned = 0;
    for plan in &plans {
        planned += match &plan.mode {
            PackTransferMode::Full => manifest.packs[&plan.pack_sha256].size,
            PackTransferMode::Ranges(ranges) => ranges
                .iter()
                .try_fold(0, |sum, range| Ok::<_, AppError>(sum + range.len()?))?,
        };
    }
    if planned != 475532317
        || plans.len() != 8
        || selected.len() != 359
        || selected.is_empty()
        || manifest.manifest_sha256 != EXPECTED_MANIFEST
        || manifest.content_sha256 != EXPECTED_CONTENT
    {
        return Err(failure("invalid probe subset size"));
    }
    if prior["pack_order"] != json!(plans.iter().map(|p| &p.pack_sha256).collect::<Vec<_>>()) {
        return Err(failure("prior pack order mismatch"));
    }
    Ok((selected, plans, planned))
}

fn seconds(snapshot: &Snapshot) -> f64 {
    snapshot.elapsed_sec - snapshot.pipeline_started_sec.unwrap_or(0.0)
}

fn protected_snapshots() -> Result<Value, AppError> {
    let paths = [
        r"D:\zhekarik-e2e-v3-1.6.16\appdata\ZHEKARIKSTRIKE\config.json",
        r"D:\zhekarik-e2e-v3-1.6.16\game\ZHEKARIKSTRIKE\.zhekarik\content\state.json",
    ];
    let mut out = serde_json::Map::new();
    for path in paths.iter().chain(std::iter::once(
        &r"%LOCALAPPDATA%\ZHEKARIKSTRIKE\config.json",
    )) {
        let expanded = if path.starts_with('%') {
            std::env::var("LOCALAPPDATA")
                .ok()
                .map(|v| PathBuf::from(v).join(r"ZHEKARIKSTRIKE\config.json"))
        } else {
            Some(PathBuf::from(path))
        };
        if let Some(path) = expanded {
            if path.exists() {
                let bytes = std::fs::read(&path)?;
                let digest = sha2::Sha256::digest(&bytes);
                let m = std::fs::metadata(&path)?
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                out.insert(path.to_string_lossy().to_string(), json!({"sha256":format!("{digest:x}"),"mtime_sec":m.as_secs(),"mtime_nanos":m.subsec_nanos()}));
            }
        }
    }
    Ok(Value::Object(out))
}

fn conclusion(runs: &[Value]) -> Value {
    if runs.len() != 4 || runs.iter().any(|run| run["error"].is_string()) {
        return json!({"outcome":"incomplete", "acceleration_confirmed":false});
    }
    let duration = |i: usize| runs[i]["pipeline_seconds"].as_f64().unwrap();
    let pair_one = 1.0 - duration(1) / duration(0);
    let pair_two = 1.0 - duration(2) / duration(3);
    let mean = 1.0 - (duration(1) + duration(2)) / (duration(0) + duration(3));
    json!({"outcome":if pair_one > 0.0 && pair_two > 0.0 && mean >= 0.10 { "acceleration_confirmed" } else { "no_gain_or_unstable" },
        "acceleration_confirmed":pair_one > 0.0 && pair_two > 0.0 && mean >= 0.10,
        "pair_one_gain":pair_one, "pair_two_gain":pair_two, "mean_gain":mean})
}

fn save_report(path: &Path, report: &Value) -> Result<(), AppError> {
    // Reports only: no user config, launcher logging initialization, or installed state.
    std::fs::write(path, serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit real Google Drive traffic; run scripts/test-adaptive-packs.ps1"]
async fn drive_pack_live_adaptive_probe() -> Result<(), AppError> {
    if std::env::var("ZHEKARIK_ADAPTIVE_PACK_PROBE").as_deref() != Ok("1") {
        return Err(failure("real probe requires explicit script opt-in"));
    }
    if !cfg!(windows) {
        return Err(failure(
            "real probe is restricted to the Windows D: workspace",
        ));
    }
    let root = PathBuf::from(ROOT);
    safe_directory(&root)?;
    std::fs::create_dir_all(&root)?;
    safe_directory(&root)?;
    let id = uuid::Uuid::new_v4().to_string();
    let marker = format!("{OWNER}:{id}");
    let series = root.join(format!("series-{id}"));
    std::fs::create_dir(&series)?;
    std::fs::write(series.join("owner.txt"), &marker)?;
    let report_path = series.join("report.json");
    eprintln!("ADAPTIVE_PROBE_REPORT={}", report_path.display());
    let mut report = json!({"schema_version":1, "id":id, "status":"fetching_manifest", "budget_bytes":TOTAL_BUDGET,
        "order":["A1-baseline-adaptive","B1-optimized","B2-optimized","A2-baseline-adaptive"], "runs":[]});
    save_report(&report_path, &report)?;
    let frozen = PathBuf::from(FROZEN_SERIES);
    let prior: Value = serde_json::from_slice(&std::fs::read(frozen.join("report.json"))?)?;
    let manifest: DrivePackManifest =
        serde_json::from_slice(&std::fs::read(frozen.join("manifest.json"))?)?;
    manifest.validate()?;
    let manifest = Arc::new(manifest);
    let (files, plans, planned) = select_files(&manifest, &prior)?;
    std::fs::write(
        series.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    report["content_sha256"] = json!(manifest.content_sha256);
    report["manifest_sha256"] = json!(manifest.manifest_sha256);
    report["planned_bytes_per_run"] = json!(planned);
    report["raw_bytes_per_run"] = json!(files.iter().map(|f| f.size).sum::<u64>());
    report["files"] = json!(files.iter().map(|f| &f.path).collect::<Vec<_>>());
    report["pack_order"] = json!(plans.iter().map(|p| &p.pack_sha256).collect::<Vec<_>>());
    report["replica_seed"] = json!("461ea0b9-971f-4b65-bdea-0a7495a0b813");
    report["status"] = json!("running");
    let protected_before = protected_snapshots()?;
    report["protected_before"] = protected_before.clone();
    save_report(&report_path, &report)?;
    eprintln!(
        "PROBE subset: {} files, {} packs, {:.1} MiB planned/run",
        files.len(),
        plans.len(),
        planned as f64 / MIB as f64
    );

    let api = ApiClient::new()?;
    let shared_budget = ProbeBudget::new(TOTAL_BUDGET);
    let series_cancel = CancellationToken::new();
    let signal_cancel = series_cancel.clone();
    let signal = tokio::spawn(async move {
        tokio::select! {
            _ = signal_cancel.cancelled() => {},
            _ = tokio::signal::ctrl_c() => signal_cancel.cancel(),
        }
    });
    let mut terminal_error = None;
    for (index, (name, profile)) in [
        ("A1-baseline-adaptive", PackProfile::Baseline),
        ("B1-optimized", PackProfile::Optimized),
        ("B2-optimized", PackProfile::Optimized),
        ("A2-baseline-adaptive", PackProfile::Baseline),
    ]
    .into_iter()
    .enumerate()
    {
        if protected_snapshots()? != protected_before {
            terminal_error = Some(failure("protected file changed during probe"));
            break;
        }
        if series_cancel.is_cancelled() {
            terminal_error = Some(failure("probe interrupted"));
            break;
        }
        if shared_budget
            .received
            .load(Ordering::Relaxed)
            .saturating_add(planned)
            > TOTAL_BUDGET
        {
            terminal_error = Some(failure("remaining probe budget cannot fund the next run"));
            break;
        }
        let work = series.join(format!("work-{index}"));
        std::fs::create_dir(&work)?;
        std::fs::write(work.join("owner.txt"), &marker)?;
        let mut options = PackRunOptions::new(name);
        options.profile = profile;
        options.fixed_jobs = None;
        options.budget = Some(shared_budget.clone());
        let metrics = options.metrics.clone();
        let cancellation = series_cancel.child_token();
        let deadline_token = cancellation.clone();
        let deadline = tokio::spawn(async move {
            tokio::select! {
                _ = deadline_token.cancelled() => {},
                _ = tokio::time::sleep(Duration::from_secs(15 * 60)) => deadline_token.cancel(),
            }
        });
        report["current_run"] = json!(name);
        save_report(&report_path, &report)?;
        eprintln!("PROBE beginning {name}");
        let result = run_packed_probe(
            &api,
            manifest.clone(),
            work.join("game"),
            files.clone(),
            options,
            cancellation.clone(),
        )
        .await;
        cancellation.cancel();
        let _ = deadline.await;
        let snapshot = metrics.snapshot();
        let history = metrics.history();
        let cleanup = cleanup_work(&series, &work, &marker);
        let error = result.err().or_else(|| cleanup.err());
        let start = snapshot.pipeline_started_sec.unwrap_or(0.0);
        let decision_snapshot = snapshot.decision_counts.clone();
        let run = json!({"name":name, "profile":format!("{profile:?}"), "fixed_jobs":null, "pipeline_seconds":seconds(&snapshot),
            "download_seconds":snapshot.download_finished_sec.map(|t|t-start),
            "first_materialized_seconds":snapshot.first_materialized_sec.map(|t|t-start),
            "materialization_seconds":snapshot.materialization_finished_sec.map(|t|t-start),
            "materialized_before_download_finished":history.iter().any(|s| s.materialized_bytes > 0 && s.download_finished_sec.is_none()),
            "snapshot":snapshot, "decisions":decision_snapshot, "history":history,
            "error":error.as_ref().map(ToString::to_string), "temporary_data_removed":!work.exists()});
        eprintln!(
            "PROBE {name}: {:.2}s, {:.1} MiB received, peak {} jobs/{} requests, error={:?}",
            seconds(&snapshot),
            snapshot.received_bytes as f64 / MIB as f64,
            snapshot.peak_jobs,
            snapshot.peak_requests,
            error.as_ref().map(ToString::to_string)
        );
        report["runs"].as_array_mut().unwrap().push(run);
        report["received_bytes"] = json!(shared_budget.received.load(Ordering::Relaxed));
        report["conclusion"] = conclusion(report["runs"].as_array().unwrap());
        report["status"] = json!(if error.is_some() { "failed" } else { "running" });
        save_report(&report_path, &report)?;
        if error.is_some() {
            terminal_error = error;
            break;
        }
    }
    series_cancel.cancel();
    let _ = signal.await;
    let protected_after = protected_snapshots()?;
    report["protected_after"] = protected_after.clone();
    if protected_after != protected_before && terminal_error.is_none() {
        terminal_error = Some(failure("protected file changed during probe"));
    }
    report["status"] = json!(if terminal_error.is_some() {
        "incomplete"
    } else {
        "complete"
    });
    report["error"] = json!(terminal_error.as_ref().map(ToString::to_string));
    report["conclusion"] = conclusion(report["runs"].as_array().unwrap());
    save_report(&report_path, &report)?;
    eprintln!("PROBE conclusion: {}", report["conclusion"]);
    if let Some(error) = terminal_error {
        return Err(error);
    }
    Ok(())
}
