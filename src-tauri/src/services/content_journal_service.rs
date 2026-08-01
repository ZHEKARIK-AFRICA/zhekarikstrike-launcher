use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::validate_game_path;
use crate::utils::hash_utils::sha256_file;
use crate::utils::path_utils::{ensure_safe_descendant, safe_join};

const CONTENT_DIRECTORY: &str = ".zhekarik/content";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentFsOperation {
    Boundary,
    Metadata,
    Read,
    ReadDir,
    CreateDir,
    Rename,
    RemoveFile,
    RemoveDir,
    WriteJournal,
}

pub(crate) trait ContentFsHooks: Send + Sync {
    fn check(&self, _operation: ContentFsOperation, _path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) struct NoContentFsHooks;

impl ContentFsHooks for NoContentFsHooks {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentJournalPhase {
    Materialize,
    Commit,
    RolledBack,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentJournalAction {
    Replace,
    Remove,
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentJournalEntry {
    pub path: String,
    pub action: ContentJournalAction,
    #[serde(default, skip_serializing_if = "is_false")]
    pub had_original: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sha256: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentJournal {
    pub schema_version: u8,
    pub transaction_id: String,
    pub release_id: String,
    pub content_sha256: String,
    pub phase: ContentJournalPhase,
    pub files: Vec<ContentJournalEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedContentJournal {
    schema_version: u8,
    transaction_id: String,
    release_id: String,
    content_sha256: String,
    phase: ContentJournalPhase,
    files: Vec<SerializedContentJournalEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedContentJournalEntry {
    path: String,
    #[serde(default)]
    action: Option<ContentJournalAction>,
    #[serde(default)]
    had_original: bool,
    #[serde(default)]
    target_size: Option<u64>,
    #[serde(default)]
    target_sha256: Option<String>,
    #[serde(default)]
    original_size: Option<u64>,
    #[serde(default)]
    original_sha256: Option<String>,
}

impl<'de> Deserialize<'de> for ContentJournal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let serialized = SerializedContentJournal::deserialize(deserializer)?;
        let files = serialized
            .files
            .into_iter()
            .map(|entry| {
                let action = match serialized.schema_version {
                    1 if entry.action.is_none() => ContentJournalAction::Replace,
                    1 => {
                        return Err(D::Error::custom(
                            "schema v1 content journal must not contain actions",
                        ))
                    }
                    2 => entry.action.ok_or_else(|| {
                        D::Error::custom("schema v2 content journal entry is missing action")
                    })?,
                    _ => entry.action.unwrap_or(ContentJournalAction::Replace),
                };
                Ok(ContentJournalEntry {
                    path: entry.path,
                    action,
                    had_original: entry.had_original,
                    target_size: entry.target_size,
                    target_sha256: entry.target_sha256,
                    original_size: entry.original_size,
                    original_sha256: entry.original_sha256,
                })
            })
            .collect::<Result<Vec<_>, D::Error>>()?;
        Ok(Self {
            schema_version: serialized.schema_version,
            transaction_id: serialized.transaction_id,
            release_id: serialized.release_id,
            content_sha256: serialized.content_sha256,
            phase: serialized.phase,
            files,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentCompletionState {
    pub schema_version: u8,
    /// Absent in v1 state files written before transactional completion was tracked.
    /// Those files remain readable, but can never complete a newer journal transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    pub content_sha256: String,
    pub release_id: String,
    pub game_version: String,
}

impl ContentJournal {
    pub fn validate(&self) -> Result<(), AppError> {
        if !matches!(self.schema_version, 1 | 2)
            || Uuid::parse_str(&self.transaction_id).is_err()
            || self.content_sha256.len() != 64
            || !self
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AppError::InvalidData("invalid content journal".into()));
        }
        let mut paths = HashSet::new();
        for entry in &self.files {
            if self.schema_version == 1 && entry.action != ContentJournalAction::Replace {
                return Err(AppError::InvalidData("invalid content journal".into()));
            }
            if !entry.path.is_ascii() {
                return Err(AppError::InvalidData(
                    "content journal paths must be ASCII".into(),
                ));
            }
            validate_game_path(&entry.path)?;
            if entry
                .path
                .split('/')
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case(".zhekarik"))
            {
                return Err(AppError::InvalidData(
                    "content journal path targets launcher state".into(),
                ));
            }
            if !paths.insert(entry.path.to_ascii_lowercase()) {
                return Err(AppError::InvalidData(
                    "content journal contains duplicate paths".into(),
                ));
            }
            validate_identity_pair(entry.target_size, entry.target_sha256.as_deref(), "target")?;
            validate_identity_pair(
                entry.original_size,
                entry.original_sha256.as_deref(),
                "original",
            )?;
            if self.schema_version == 2 && self.phase != ContentJournalPhase::RolledBack {
                match entry.action {
                    ContentJournalAction::Replace => {
                        if entry.target_size.is_none()
                            || entry.had_original != entry.original_size.is_some()
                        {
                            return Err(AppError::InvalidData(
                                "replace journal identity is incomplete".into(),
                            ));
                        }
                    }
                    ContentJournalAction::Remove => {
                        if entry.target_size.is_some()
                            || entry.had_original != entry.original_size.is_some()
                        {
                            return Err(AppError::InvalidData(
                                "remove journal identity is incomplete".into(),
                            ));
                        }
                    }
                }
            }
            safe_join(Path::new("."), &entry.path)?;
        }
        Ok(())
    }
}

impl ContentCompletionState {
    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1
            || self.content_sha256.len() != 64
            || !self
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.release_id.is_empty()
            || self.game_version.is_empty()
            || self
                .transaction_id
                .as_deref()
                .is_some_and(|value| Uuid::parse_str(value).is_err())
        {
            return Err(AppError::InvalidData("invalid content state".into()));
        }
        Ok(())
    }
}

fn validate_identity_pair(
    size: Option<u64>,
    sha256: Option<&str>,
    label: &str,
) -> Result<(), AppError> {
    if size.is_some() != sha256.is_some()
        || sha256.is_some_and(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Err(AppError::InvalidData(format!(
            "invalid content journal {label} identity"
        )));
    }
    Ok(())
}

pub fn content_root(game_path: &Path) -> PathBuf {
    game_path.join(CONTENT_DIRECTORY)
}

pub fn journal_path(game_path: &Path) -> PathBuf {
    content_root(game_path).join("journal.json")
}

pub fn state_path(game_path: &Path) -> PathBuf {
    content_root(game_path).join("state.json")
}

pub fn staging_path(game_path: &Path, transaction_id: &str) -> PathBuf {
    content_root(game_path).join("staging").join(transaction_id)
}

pub fn backup_path(game_path: &Path, transaction_id: &str) -> PathBuf {
    content_root(game_path).join("backup").join(transaction_id)
}

pub async fn write_journal(game_path: &Path, journal: &ContentJournal) -> Result<(), AppError> {
    write_journal_with_hooks(game_path, journal, &NoContentFsHooks).await
}

async fn write_journal_with_hooks(
    game_path: &Path,
    journal: &ContentJournal,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    journal.validate()?;
    if journal.schema_version != 2 {
        return Err(AppError::InvalidData(
            "new content journals must use schema v2".into(),
        ));
    }
    let path = journal_path(game_path);
    guard_content_path(game_path, &path, hooks).await?;
    hooks.check(ContentFsOperation::WriteJournal, &path)?;
    atomic_json(&path, journal).await?;
    guard_content_path(game_path, &path, hooks).await
}

pub async fn recover_pending_content(game_path: &Path) -> Result<bool, AppError> {
    recover_pending_content_with_hooks(game_path, &NoContentFsHooks).await
}

pub(crate) async fn recover_pending_content_with_hooks(
    game_path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<bool, AppError> {
    let path = journal_path(game_path);
    if guarded_content_metadata(game_path, &path, hooks)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    let bytes = guarded_content_read(game_path, &path, hooks).await?;
    let mut journal: ContentJournal = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InvalidData(format!("invalid content journal: {error}")))?;
    journal.validate()?;
    match journal.phase {
        ContentJournalPhase::RolledBack | ContentJournalPhase::Materialize => {
            cleanup_transaction_with_hooks(game_path, &journal.transaction_id, hooks).await?;
            guarded_remove_file_if_exists(game_path, &path, hooks).await?;
        }
        ContentJournalPhase::Commit => {
            let committed = load_completion_state_with_hooks(game_path, hooks)
                .await?
                .is_some_and(|state| {
                    state.transaction_id.as_deref() == Some(journal.transaction_id.as_str())
                        && state.content_sha256 == journal.content_sha256
                        && state.release_id == journal.release_id
                });
            if committed {
                remove_empty_obsolete_directories_with_hooks(game_path, &journal, hooks).await?;
                cleanup_transaction_with_hooks(game_path, &journal.transaction_id, hooks).await?;
                guarded_remove_file_if_exists(game_path, &path, hooks).await?;
            } else {
                rollback_content_transaction_with_hooks(game_path, &mut journal, hooks).await?;
            }
        }
    }
    Ok(true)
}

pub async fn load_completion_state(
    game_path: &Path,
) -> Result<Option<ContentCompletionState>, AppError> {
    load_completion_state_with_hooks(game_path, &NoContentFsHooks).await
}

async fn load_completion_state_with_hooks(
    game_path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<Option<ContentCompletionState>, AppError> {
    let path = state_path(game_path);
    let Some(metadata) = guarded_content_metadata(game_path, &path, hooks).await? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(AppError::InvalidData(
            "content state path is not a file".into(),
        ));
    }
    let bytes = guarded_content_read(game_path, &path, hooks).await?;
    let state: ContentCompletionState = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InvalidData(format!("invalid content state: {error}")))?;
    state.validate()?;
    Ok(Some(state))
}

pub async fn recover_interrupted_commit(
    game_path: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    let mut journal = journal.clone();
    rollback_content_transaction_with_hooks(game_path, &mut journal, &NoContentFsHooks).await
}

pub async fn remove_empty_obsolete_directories(
    game_path: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    remove_empty_obsolete_directories_with_hooks(game_path, journal, &NoContentFsHooks).await
}

pub(crate) async fn remove_empty_obsolete_directories_with_hooks(
    game_path: &Path,
    journal: &ContentJournal,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let mut directories = HashSet::new();
    for entry in journal
        .files
        .iter()
        .filter(|entry| entry.action == ContentJournalAction::Remove)
    {
        let target = safe_join(game_path, &entry.path)?;
        let mut parent = target.parent();
        while let Some(directory) = parent {
            if directory == game_path || !directory.starts_with(game_path) {
                break;
            }
            directories.insert(directory.to_path_buf());
            parent = directory.parent();
        }
    }
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|directory| Reverse(directory.components().count()));
    for directory in directories {
        guarded_remove_empty_dir_if_exists(game_path, &directory, hooks).await?;
    }
    Ok(())
}

pub async fn cleanup_transaction(game_path: &Path, transaction_id: &str) -> Result<(), AppError> {
    cleanup_transaction_with_hooks(game_path, transaction_id, &NoContentFsHooks).await
}

async fn cleanup_transaction_with_hooks(
    game_path: &Path,
    transaction_id: &str,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    remove_controlled_tree(game_path, &staging_path(game_path, transaction_id), hooks).await?;
    remove_controlled_tree(game_path, &backup_path(game_path, transaction_id), hooks).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContentFileIdentity {
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

pub(crate) async fn guard_content_path(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    hooks.check(ContentFsOperation::Boundary, path)?;
    ensure_safe_descendant(game_path, path).await
}

pub(crate) async fn guarded_content_metadata(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<Option<std::fs::Metadata>, AppError> {
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::Metadata, path)?;
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn capture_content_file_identity(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<Option<ContentFileIdentity>, AppError> {
    let Some(metadata) = guarded_content_metadata(game_path, path, hooks).await? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(AppError::InvalidData(format!(
            "managed content path is not a regular file: {}",
            path.display()
        )));
    }
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::Read, path)?;
    let sha256 = sha256_file(path).await?;
    let Some(after) = guarded_content_metadata(game_path, path, hooks).await? else {
        return Err(AppError::FileSystem(format!(
            "managed content file disappeared while hashing: {}",
            path.display()
        )));
    };
    if !after.is_file() || after.len() != metadata.len() {
        return Err(AppError::FileSystem(format!(
            "managed content file changed while hashing: {}",
            path.display()
        )));
    }
    Ok(Some(ContentFileIdentity {
        size: metadata.len(),
        sha256,
    }))
}

async fn guarded_content_read(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<Vec<u8>, AppError> {
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::Read, path)?;
    Ok(tokio::fs::read(path).await?)
}

pub(crate) async fn guarded_content_create_dir_all(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::CreateDir, path)?;
    tokio::fs::create_dir_all(path).await?;
    guard_content_path(game_path, path, hooks).await
}

pub(crate) async fn guarded_content_rename(
    game_path: &Path,
    source: &Path,
    target: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    guard_content_path(game_path, source, hooks).await?;
    guard_content_path(game_path, target, hooks).await?;
    hooks.check(ContentFsOperation::Rename, source)?;
    hooks.check(ContentFsOperation::Rename, target)?;
    tokio::fs::rename(source, target).await?;
    Ok(())
}

async fn guarded_content_remove_file(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::RemoveFile, path)?;
    tokio::fs::remove_file(path).await?;
    Ok(())
}

pub(crate) async fn guarded_remove_file_if_exists(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let Some(metadata) = guarded_content_metadata(game_path, path, hooks).await? else {
        return Ok(());
    };
    if !metadata.is_file() {
        return Err(AppError::InvalidData(format!(
            "managed cleanup path is not a file: {}",
            path.display()
        )));
    }
    guarded_content_remove_file(game_path, path, hooks).await
}

async fn guarded_content_remove_dir(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::RemoveDir, path)?;
    tokio::fs::remove_dir(path).await?;
    Ok(())
}

async fn guarded_remove_empty_dir_if_exists(
    game_path: &Path,
    path: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let Some(metadata) = guarded_content_metadata(game_path, path, hooks).await? else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return Err(AppError::InvalidData(format!(
            "obsolete content parent is not a directory: {}",
            path.display()
        )));
    }
    guard_content_path(game_path, path, hooks).await?;
    hooks.check(ContentFsOperation::RemoveDir, path)?;
    match tokio::fs::remove_dir(path).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

async fn remove_controlled_tree(
    game_path: &Path,
    root: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let Some(metadata) = guarded_content_metadata(game_path, root, hooks).await? else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return Err(AppError::InvalidData(format!(
            "content transaction path is not a directory: {}",
            root.display()
        )));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        guard_content_path(game_path, &directory, hooks).await?;
        hooks.check(ContentFsOperation::ReadDir, &directory)?;
        let mut entries = tokio::fs::read_dir(&directory).await?;
        directories.push(directory);
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = guarded_content_metadata(game_path, &path, hooks)
                .await?
                .ok_or_else(|| {
                    AppError::FileSystem(format!(
                        "content transaction entry disappeared: {}",
                        path.display()
                    ))
                })?;
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
            } else {
                return Err(AppError::InvalidData(format!(
                    "content transaction contains an unsafe entry: {}",
                    path.display()
                )));
            }
        }
    }
    for file in files {
        guarded_content_remove_file(game_path, &file, hooks).await?;
    }
    directories.sort_by_key(|path| Reverse(path.components().count()));
    for directory in directories {
        guarded_content_remove_dir(game_path, &directory, hooks).await?;
    }
    Ok(())
}

fn expected_identity(
    size: Option<u64>,
    sha256: Option<&str>,
) -> Result<Option<ContentFileIdentity>, AppError> {
    match (size, sha256) {
        (Some(size), Some(sha256)) => Ok(Some(ContentFileIdentity {
            size,
            sha256: sha256.to_string(),
        })),
        (None, None) => Ok(None),
        _ => Err(AppError::InvalidData(
            "content journal identity is incomplete".into(),
        )),
    }
}

fn ambiguous_rollback(entry: &ContentJournalEntry, reason: &str) -> AppError {
    AppError::FileSystem(format!(
        "ambiguous rollback state for {}: {reason}; recovery data was preserved",
        entry.path
    ))
}

async fn remove_expected_file(
    game_path: &Path,
    path: &Path,
    expected: &ContentFileIdentity,
    hooks: &dyn ContentFsHooks,
    entry: &ContentJournalEntry,
) -> Result<(), AppError> {
    if capture_content_file_identity(game_path, path, hooks)
        .await?
        .as_ref()
        != Some(expected)
    {
        return Err(ambiguous_rollback(
            entry,
            "target identity changed before removal",
        ));
    }
    guarded_content_remove_file(game_path, path, hooks).await
}

async fn restore_backup(
    game_path: &Path,
    backup: &Path,
    target: &Path,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    if let Some(parent) = target.parent() {
        guarded_content_create_dir_all(game_path, parent, hooks).await?;
    }
    guarded_content_rename(game_path, backup, target, hooks).await
}

async fn rollback_replace_entry(
    game_path: &Path,
    staging_root: &Path,
    backup_root: &Path,
    entry: &ContentJournalEntry,
    schema_version: u8,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let target = safe_join(game_path, &entry.path)?;
    let staged = safe_join(staging_root, &entry.path)?;
    let backup = safe_join(backup_root, &entry.path)?;
    let target_actual = capture_content_file_identity(game_path, &target, hooks).await?;
    let staged_actual = capture_content_file_identity(game_path, &staged, hooks).await?;
    let backup_actual = capture_content_file_identity(game_path, &backup, hooks).await?;
    let target_expected = expected_identity(entry.target_size, entry.target_sha256.as_deref())?;
    let original_expected =
        expected_identity(entry.original_size, entry.original_sha256.as_deref())?;

    if entry.had_original {
        if let Some(backup_actual) = backup_actual {
            if let Some(original_expected) = &original_expected {
                if &backup_actual != original_expected {
                    return Err(ambiguous_rollback(
                        entry,
                        "backup identity does not match original",
                    ));
                }
            }
            if let Some(staged_actual) = &staged_actual {
                if target_expected
                    .as_ref()
                    .is_some_and(|expected| staged_actual != expected)
                {
                    return Err(ambiguous_rollback(entry, "staged identity is unexpected"));
                }
                if target_actual.is_some() {
                    return Err(ambiguous_rollback(
                        entry,
                        "both staged and target files exist",
                    ));
                }
            }
            if let Some(target_actual) = &target_actual {
                if schema_version == 2 {
                    let Some(target_expected) = &target_expected else {
                        return Err(ambiguous_rollback(entry, "target identity is unavailable"));
                    };
                    if target_actual != target_expected {
                        return Err(ambiguous_rollback(
                            entry,
                            "target is not the committed file",
                        ));
                    }
                    remove_expected_file(game_path, &target, target_expected, hooks, entry).await?;
                } else {
                    guarded_content_remove_file(game_path, &target, hooks).await?;
                }
            }
            restore_backup(game_path, &backup, &target, hooks).await?;
            return Ok(());
        }

        let Some(target_actual) = target_actual else {
            return Err(ambiguous_rollback(
                entry,
                "original and backup are both missing",
            ));
        };
        if schema_version == 1 {
            return Err(ambiguous_rollback(
                entry,
                "legacy journal has no original identity",
            ));
        }
        let Some(original_expected) = original_expected else {
            return Err(ambiguous_rollback(
                entry,
                "original identity is unavailable",
            ));
        };
        if target_actual != original_expected {
            return Err(ambiguous_rollback(
                entry,
                "missing backup while target is not original",
            ));
        }
        if let Some(staged_actual) = staged_actual {
            if target_expected.as_ref() != Some(&staged_actual) {
                return Err(ambiguous_rollback(
                    entry,
                    "unstarted staged file identity changed",
                ));
            }
        }
        return Ok(());
    }

    if backup_actual.is_some() {
        return Err(ambiguous_rollback(
            entry,
            "no-original replacement has a backup",
        ));
    }
    if let Some(staged_actual) = staged_actual {
        if target_expected.as_ref() != Some(&staged_actual) || target_actual.is_some() {
            return Err(ambiguous_rollback(
                entry,
                "unstarted replacement facts conflict",
            ));
        }
        return Ok(());
    }
    let Some(target_actual) = target_actual else {
        return Ok(());
    };
    let Some(target_expected) = target_expected else {
        return Err(ambiguous_rollback(entry, "target identity is unavailable"));
    };
    if target_actual != target_expected {
        return Err(ambiguous_rollback(
            entry,
            "target is not the managed staged file",
        ));
    }
    remove_expected_file(game_path, &target, &target_expected, hooks, entry).await
}

async fn rollback_remove_entry(
    game_path: &Path,
    backup_root: &Path,
    entry: &ContentJournalEntry,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    let target = safe_join(game_path, &entry.path)?;
    let backup = safe_join(backup_root, &entry.path)?;
    let backup_actual = capture_content_file_identity(game_path, &backup, hooks).await?;
    if !entry.had_original {
        if backup_actual.is_some() {
            return Err(ambiguous_rollback(
                entry,
                "absent removal unexpectedly has a backup",
            ));
        }
        return Ok(());
    }
    let target_actual = capture_content_file_identity(game_path, &target, hooks).await?;
    let original_expected =
        expected_identity(entry.original_size, entry.original_sha256.as_deref())?;
    if let Some(backup_actual) = backup_actual {
        if original_expected
            .as_ref()
            .is_some_and(|expected| &backup_actual != expected)
        {
            return Err(ambiguous_rollback(entry, "removal backup identity changed"));
        }
        if target_actual.is_some() {
            return Err(ambiguous_rollback(
                entry,
                "removal target and backup both exist",
            ));
        }
        return restore_backup(game_path, &backup, &target, hooks).await;
    }
    let Some(target_actual) = target_actual else {
        return Err(ambiguous_rollback(
            entry,
            "removed target and required backup are missing",
        ));
    };
    if let Some(original_expected) = original_expected {
        if target_actual != original_expected {
            return Err(ambiguous_rollback(
                entry,
                "restored target identity changed",
            ));
        }
        return Ok(());
    }
    Err(ambiguous_rollback(
        entry,
        "legacy removal identity is unavailable",
    ))
}

async fn rollback_content_transaction_with_hooks(
    game_path: &Path,
    journal: &mut ContentJournal,
    hooks: &dyn ContentFsHooks,
) -> Result<(), AppError> {
    if journal.phase == ContentJournalPhase::RolledBack {
        cleanup_transaction_with_hooks(game_path, &journal.transaction_id, hooks).await?;
        return guarded_remove_file_if_exists(game_path, &journal_path(game_path), hooks).await;
    }
    if journal.phase != ContentJournalPhase::Commit {
        return Err(AppError::InvalidData(
            "content rollback requires commit phase".into(),
        ));
    }
    let schema_version = journal.schema_version;
    let staging_root = staging_path(game_path, &journal.transaction_id);
    let backup_root = backup_path(game_path, &journal.transaction_id);
    for entry in journal.files.iter().rev() {
        match entry.action {
            ContentJournalAction::Replace => {
                rollback_replace_entry(
                    game_path,
                    &staging_root,
                    &backup_root,
                    entry,
                    schema_version,
                    hooks,
                )
                .await?;
            }
            ContentJournalAction::Remove => {
                rollback_remove_entry(game_path, &backup_root, entry, hooks).await?;
            }
        }
    }
    journal.schema_version = 2;
    journal.phase = ContentJournalPhase::RolledBack;
    write_journal_with_hooks(game_path, journal, hooks).await?;
    cleanup_transaction_with_hooks(game_path, &journal.transaction_id, hooks).await?;
    guarded_remove_file_if_exists(game_path, &journal_path(game_path), hooks).await
}

pub async fn atomic_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(value)?;
    atomic_bytes(path, &bytes).await
}

pub async fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        atomic_replace(&temporary, path).await?;
        Ok::<(), AppError>(())
    }
    .await;
    if result.is_err() {
        tokio::fs::remove_file(&temporary).await.ok();
    }
    result
}

#[cfg(target_os = "windows")]
async fn atomic_replace(source: &Path, target: &Path) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| AppError::FileSystem(format!("atomic file replace failed: {error}")))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
async fn atomic_replace(source: &Path, target: &Path) -> Result<(), AppError> {
    tokio::fs::rename(source, target).await?;
    Ok(())
}

pub async fn remove_file_if_exists(path: &Path) -> Result<(), AppError> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(AppError::InvalidData(format!(
            "managed cleanup path is not a file: {}",
            path.display()
        )));
    }
    tokio::fs::remove_file(path).await?;
    Ok(())
}
