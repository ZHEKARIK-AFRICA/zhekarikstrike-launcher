use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::validate_game_path;
use crate::utils::path_utils::safe_join;

const CONTENT_DIRECTORY: &str = ".zhekarik/content";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContentJournalPhase {
    Materialize,
    Commit,
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
            safe_join(Path::new("."), &entry.path)?;
        }
        Ok(())
    }
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
    journal.validate()?;
    if journal.schema_version != 2 {
        return Err(AppError::InvalidData(
            "new content journals must use schema v2".into(),
        ));
    }
    atomic_json(&journal_path(game_path), journal).await
}

pub async fn recover_pending_content(game_path: &Path) -> Result<bool, AppError> {
    let path = journal_path(game_path);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(false);
    }
    let bytes = tokio::fs::read(&path).await?;
    let journal: ContentJournal = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InvalidData(format!("invalid content journal: {error}")))?;
    journal.validate()?;
    let committed = load_completion_state(game_path)
        .await
        .ok()
        .flatten()
        .is_some_and(|state| {
            state.schema_version == 1
                && state.transaction_id.as_deref() == Some(journal.transaction_id.as_str())
                && state.content_sha256 == journal.content_sha256
                && state.release_id == journal.release_id
        });
    if !committed {
        recover_interrupted_commit(game_path, &journal).await?;
    } else {
        remove_empty_obsolete_directories(game_path, &journal).await?;
    }
    cleanup_transaction(game_path, &journal.transaction_id).await?;
    remove_file_if_exists(&path).await?;
    Ok(true)
}

pub async fn load_completion_state(
    game_path: &Path,
) -> Result<Option<ContentCompletionState>, AppError> {
    let path = state_path(game_path);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(None);
    }
    let bytes = tokio::fs::read(path).await?;
    let state = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InvalidData(format!("invalid content state: {error}")))?;
    Ok(Some(state))
}

pub async fn recover_interrupted_commit(
    game_path: &Path,
    journal: &ContentJournal,
) -> Result<(), AppError> {
    if journal.phase != ContentJournalPhase::Commit {
        return Ok(());
    }
    let backup_root = backup_path(game_path, &journal.transaction_id);
    for entry in journal.files.iter().rev() {
        let target = safe_join(game_path, &entry.path)?;
        let backup = safe_join(&backup_root, &entry.path)?;
        match entry.action {
            ContentJournalAction::Replace if entry.had_original => {
                if !tokio::fs::try_exists(&backup).await.unwrap_or(false) {
                    continue;
                }
                remove_path_if_exists(&target).await?;
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::rename(&backup, &target).await?;
            }
            ContentJournalAction::Replace => remove_path_if_exists(&target).await?,
            ContentJournalAction::Remove => {
                if !tokio::fs::try_exists(&backup).await.unwrap_or(false) {
                    continue;
                }
                remove_path_if_exists(&target).await?;
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::rename(&backup, &target).await?;
            }
        }
    }
    Ok(())
}

pub async fn remove_empty_obsolete_directories(
    game_path: &Path,
    journal: &ContentJournal,
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
        match tokio::fs::remove_dir(&directory).await {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub async fn cleanup_transaction(game_path: &Path, transaction_id: &str) -> Result<(), AppError> {
    remove_directory_if_exists(&staging_path(game_path, transaction_id)).await?;
    remove_directory_if_exists(&backup_path(game_path, transaction_id)).await?;
    Ok(())
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

pub async fn remove_path_if_exists(path: &Path) -> Result<(), AppError> {
    let Ok(metadata) = tokio::fs::symlink_metadata(path).await else {
        return Ok(());
    };
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await?;
    } else {
        tokio::fs::remove_file(path).await?;
    }
    Ok(())
}

pub async fn remove_directory_if_exists(path: &Path) -> Result<(), AppError> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(path).await?;
    }
    Ok(())
}

pub async fn remove_file_if_exists(path: &Path) -> Result<(), AppError> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        tokio::fs::remove_file(path).await?;
    }
    Ok(())
}
