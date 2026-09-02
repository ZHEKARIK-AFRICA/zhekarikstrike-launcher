use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::ContentInventory;
use crate::services::content_inventory_service::save_content_inventory;
use crate::services::content_journal_service::{
    atomic_json, backup_path, capture_content_file_identity, content_root,
    guarded_content_create_dir_all, guarded_content_metadata, guarded_content_rename,
    guarded_remove_file_if_exists, journal_path, recover_interrupted_commit,
    remove_empty_obsolete_directories, state_path, write_journal, ContentCompletionState,
    ContentFileIdentity, ContentJournal, ContentJournalAction, ContentJournalPhase,
    NoContentFsHooks,
};
use crate::utils::path_utils::safe_join;

#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub relative_path: PathBuf,
    pub temporary_path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone)]
pub struct StagingBudget {
    limit: u64,
    available: Arc<Mutex<u64>>,
    changed: Arc<Notify>,
}

impl StagingBudget {
    pub fn new(limit: u64) -> Result<Self, AppError> {
        if limit == 0 {
            return Err(AppError::InvalidData(
                "streaming staging budget cannot be zero".into(),
            ));
        }
        Ok(Self {
            limit,
            available: Arc::new(Mutex::new(limit)),
            changed: Arc::new(Notify::new()),
        })
    }

    pub async fn reserve(
        &self,
        bytes: u64,
        cancellation: &CancellationToken,
    ) -> Result<(), AppError> {
        if bytes > self.limit {
            return Err(AppError::InvalidData(
                "content file exceeds streaming staging budget".into(),
            ));
        }
        loop {
            {
                let mut available = self.available.lock().await;
                if *available >= bytes {
                    *available -= bytes;
                    return Ok(());
                }
            }
            tokio::select! {
                _ = cancellation.cancelled() => return Err(AppError::Canceled),
                _ = self.changed.notified() => {}
            }
        }
    }

    pub async fn release(&self, bytes: u64) {
        let mut available = self.available.lock().await;
        *available = available.saturating_add(bytes).min(self.limit);
        drop(available);
        self.changed.notify_waiters();
    }
}

pub struct CommitContext {
    pub game_path: PathBuf,
    pub journal: ContentJournal,
    pub inventory: ContentInventory,
    pub staging_budget: StagingBudget,
    pub committed: mpsc::Sender<u64>,
}

pub async fn run_streaming_commit(
    mut context: CommitContext,
    mut artifacts: mpsc::Receiver<VerifiedArtifact>,
    cancellation: CancellationToken,
) -> Result<ContentCompletionState, AppError> {
    context.inventory.validate()?;
    context.journal.validate()?;
    if context.journal.content_sha256 != context.inventory.content_sha256
        || context.journal.release_id != context.inventory.release_id
        || context.journal.phase != ContentJournalPhase::StreamingCommit
    {
        return Err(AppError::InvalidData(
            "streaming commit context identity is invalid".into(),
        ));
    }
    let inventory_files = context
        .inventory
        .files
        .iter()
        .map(|file| (file.path.to_ascii_lowercase(), file))
        .collect::<std::collections::HashMap<_, _>>();
    for entry in &context.journal.files {
        match entry.action {
            ContentJournalAction::Replace => {
                let Some(file) = inventory_files.get(&entry.path.to_ascii_lowercase()) else {
                    return Err(AppError::InvalidData(
                        "streaming journal replacement is absent from inventory".into(),
                    ));
                };
                if entry.path != file.path
                    || entry.target_size != Some(file.size)
                    || entry.target_sha256.as_deref() != Some(file.sha256.as_str())
                {
                    return Err(AppError::InvalidData(
                        "streaming journal replacement differs from inventory".into(),
                    ));
                }
            }
            ContentJournalAction::Remove => {
                if inventory_files.contains_key(&entry.path.to_ascii_lowercase()) {
                    return Err(AppError::InvalidData(
                        "streaming journal removes an active inventory file".into(),
                    ));
                }
            }
        }
    }

    let mut completed = HashSet::new();
    let mut cancellation_seen = false;
    loop {
        let next = if cancellation_seen {
            artifacts.recv().await
        } else {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    cancellation_seen = true;
                    continue;
                }
                artifact = artifacts.recv() => artifact,
            }
        };
        let Some(artifact) = next else { break };
        if cancellation_seen {
            context.staging_budget.release(artifact.size).await;
            continue;
        }
        let result = commit_artifact(&mut context, &mut completed, &artifact).await;
        context.staging_budget.release(artifact.size).await;
        if let Err(error) = result {
            cancellation.cancel();
            drain_artifacts(&mut artifacts, &context.staging_budget).await;
            return rollback(&context.game_path, &context.journal, error).await;
        }
        let _ = context.committed.send(artifact.size).await;
    }

    if cancellation_seen || cancellation.is_cancelled() {
        return rollback(&context.game_path, &context.journal, AppError::Canceled).await;
    }

    let expected = context
        .journal
        .files
        .iter()
        .filter(|entry| entry.action == ContentJournalAction::Replace)
        .count();
    if completed.len() != expected {
        return rollback(
            &context.game_path,
            &context.journal,
            AppError::InvalidData(
                "streaming materialization ended before every file was ready".into(),
            ),
        )
        .await;
    }

    if let Err(error) = commit_obsolete_entries(&mut context).await {
        return rollback(&context.game_path, &context.journal, error).await;
    }
    if let Err(error) = save_content_inventory(&context.game_path, &context.inventory).await {
        return rollback(&context.game_path, &context.journal, error).await;
    }

    let state = ContentCompletionState {
        schema_version: 1,
        transaction_id: Some(context.journal.transaction_id.clone()),
        content_sha256: context.inventory.content_sha256.clone(),
        release_id: context.inventory.release_id.clone(),
        game_version: context.inventory.game_version.clone(),
    };
    if let Err(error) = atomic_json(&state_path(&context.game_path), &state).await {
        return rollback(&context.game_path, &context.journal, error).await;
    }
    if let Err(error) =
        remove_empty_obsolete_directories(&context.game_path, &context.journal).await
    {
        crate::logger::warn(&format!(
            "committed content but could not remove obsolete empty directories: {error}"
        ));
    }
    Ok(state)
}

async fn commit_artifact(
    context: &mut CommitContext,
    completed: &mut HashSet<String>,
    artifact: &VerifiedArtifact,
) -> Result<(), AppError> {
    let entry_index = context
        .journal
        .files
        .iter()
        .position(|entry| {
            entry.action == ContentJournalAction::Replace
                && safe_join(&context.game_path, &entry.path)
                    .ok()
                    .and_then(|_| {
                        safe_join(
                            &crate::services::content_journal_service::staging_path(
                                &context.game_path,
                                &context.journal.transaction_id,
                            ),
                            &entry.path,
                        )
                        .ok()
                    })
                    .is_some_and(|path| path == artifact.temporary_path)
        })
        .ok_or_else(|| AppError::InvalidData("unrecognized verified content artifact".into()))?;
    let entry_path = context.journal.files[entry_index].path.clone();
    let expected_relative_path = entry_path.replace('/', std::path::MAIN_SEPARATOR_STR);
    if artifact.relative_path != Path::new(&expected_relative_path)
        || context.journal.files[entry_index].target_size != Some(artifact.size)
        || context.journal.files[entry_index].target_sha256.as_deref()
            != Some(artifact.sha256.as_str())
        || !completed.insert(entry_path.clone())
    {
        return Err(AppError::InvalidData(
            "verified content artifact identity is invalid".into(),
        ));
    }
    let metadata = guarded_content_metadata(
        &context.game_path,
        &artifact.temporary_path,
        &NoContentFsHooks,
    )
    .await?
    .ok_or_else(|| AppError::InvalidData("verified content artifact disappeared".into()))?;
    if !metadata.is_file() || metadata.len() != artifact.size {
        return Err(AppError::InvalidData(
            "verified content artifact is not a regular file of the expected size".into(),
        ));
    }

    amend_original_identity(context, entry_index).await?;
    let entry = &context.journal.files[entry_index];
    let target = safe_join(&context.game_path, &entry.path)?;
    let backup = safe_join(
        &backup_path(&context.game_path, &context.journal.transaction_id),
        &entry.path,
    )?;
    move_original_to_backup(&context.game_path, entry, &target, &backup).await?;
    if let Some(parent) = target.parent() {
        guarded_content_create_dir_all(&context.game_path, parent, &NoContentFsHooks).await?;
    }
    guarded_content_rename(
        &context.game_path,
        &artifact.temporary_path,
        &target,
        &NoContentFsHooks,
    )
    .await
}

async fn commit_obsolete_entries(context: &mut CommitContext) -> Result<(), AppError> {
    let indexes = context
        .journal
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (entry.action == ContentJournalAction::Remove).then_some(index)
        })
        .collect::<Vec<_>>();
    for index in indexes {
        amend_original_identity(context, index).await?;
        let entry = &context.journal.files[index];
        let target = safe_join(&context.game_path, &entry.path)?;
        let backup = safe_join(
            &backup_path(&context.game_path, &context.journal.transaction_id),
            &entry.path,
        )?;
        move_original_to_backup(&context.game_path, entry, &target, &backup).await?;
    }
    Ok(())
}

async fn amend_original_identity(
    context: &mut CommitContext,
    entry_index: usize,
) -> Result<(), AppError> {
    let entry = &context.journal.files[entry_index];
    let target = safe_join(&context.game_path, &entry.path)?;
    let actual =
        capture_content_file_identity(&context.game_path, &target, &NoContentFsHooks).await?;
    let expected = entry
        .original_size
        .zip(entry.original_sha256.clone())
        .map(|(size, sha256)| ContentFileIdentity { size, sha256 });
    if actual == expected {
        return Ok(());
    }
    let entry = &mut context.journal.files[entry_index];
    entry.had_original = actual.is_some();
    entry.original_size = actual.as_ref().map(|identity| identity.size);
    entry.original_sha256 = actual.map(|identity| identity.sha256);
    write_journal(&context.game_path, &context.journal).await
}

async fn move_original_to_backup(
    game_path: &Path,
    entry: &crate::services::content_journal_service::ContentJournalEntry,
    target: &Path,
    backup: &Path,
) -> Result<(), AppError> {
    if !entry.had_original {
        return Ok(());
    }
    if guarded_content_metadata(game_path, backup, &NoContentFsHooks)
        .await?
        .is_some()
    {
        return Err(AppError::InvalidData(format!(
            "transaction backup already exists: {}",
            entry.path
        )));
    }
    if let Some(parent) = backup.parent() {
        guarded_content_create_dir_all(game_path, parent, &NoContentFsHooks).await?;
    }
    guarded_content_rename(game_path, target, backup, &NoContentFsHooks).await
}

async fn drain_artifacts(artifacts: &mut mpsc::Receiver<VerifiedArtifact>, budget: &StagingBudget) {
    while let Some(artifact) = artifacts.recv().await {
        budget.release(artifact.size).await;
    }
}

async fn rollback<T>(
    game_path: &Path,
    journal: &ContentJournal,
    error: AppError,
) -> Result<T, AppError> {
    match recover_interrupted_commit(game_path, journal).await {
        Ok(()) => Err(error),
        Err(rollback_error) => Err(AppError::FileSystem(format!(
            "streaming content commit failed ({error}); rollback also failed ({rollback_error}); recovery data was preserved"
        ))),
    }
}

pub async fn queue_success_cleanup(
    game_path: &Path,
    transaction_id: &str,
    content_sha256: &str,
) -> Result<(), AppError> {
    crate::models::validate_sha256(content_sha256, "content cleanup")?;
    let root = content_root(game_path);
    let cleanup = root.join("cleanup").join(Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&cleanup).await?;
    let candidates = [
        (root.join("staging").join(transaction_id), "staging"),
        (root.join("backup").join(transaction_id), "backup"),
        (root.join("pack-cache").join(content_sha256), "pack-cache"),
        (root.join("chunks"), "legacy-chunks"),
    ];
    for (source, name) in candidates {
        match tokio::fs::symlink_metadata(&source).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                tokio::fs::rename(source, cleanup.join(name)).await?;
            }
            Ok(_) => {
                return Err(AppError::InvalidData(format!(
                    "content cleanup source is not a regular directory: {}",
                    source.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    guarded_remove_file_if_exists(game_path, &journal_path(game_path), &NoContentFsHooks).await?;
    spawn_cleanup(cleanup);
    Ok(())
}

pub async fn retry_background_cleanup(game_path: &Path) -> Result<(), AppError> {
    let root = content_root(game_path).join("cleanup");
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() || file_type.is_symlink() {
            return Err(AppError::InvalidData(format!(
                "content cleanup entry is not a regular directory: {}",
                entry.path().display()
            )));
        }
        spawn_cleanup(entry.path());
    }
    Ok(())
}

fn spawn_cleanup(path: PathBuf) {
    tokio::spawn(async move {
        if let Err(error) = tokio::fs::remove_dir_all(&path).await {
            crate::logger::warn(&format!(
                "deferred content cleanup failed for {}: {error}",
                path.display()
            ));
        }
    });
}
