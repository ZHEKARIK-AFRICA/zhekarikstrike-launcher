use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::constants::MODERN_API_BASE_URL;
use crate::error::AppError;
use crate::services::content_journal_service::{atomic_bytes, atomic_json};

const AVATAR_STATE_PATH: &str = ".zhekarik/avatar-state.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AvatarState {
    schema_version: u8,
    nickname: String,
}

pub async fn generate_avatar(game_path: PathBuf, nickname: String) -> Result<(), AppError> {
    if avatar_is_current(&game_path, &nickname).await {
        return Ok(());
    }

    let bytes = reqwest::Client::new()
        .get(format!("{MODERN_API_BASE_URL}/launcher/avatar"))
        .query(&[("nickname", nickname.as_str())])
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    persist_avatar(&game_path, &nickname, &bytes).await
}

async fn avatar_is_current(game_path: &Path, nickname: &str) -> bool {
    if !tokio::fs::try_exists(game_path.join("platform/avatar.dat"))
        .await
        .unwrap_or(false)
    {
        return false;
    }
    let Ok(bytes) = tokio::fs::read(game_path.join(AVATAR_STATE_PATH)).await else {
        return false;
    };
    serde_json::from_slice::<AvatarState>(&bytes)
        .ok()
        .is_some_and(|state| state.schema_version == 1 && state.nickname == nickname)
}

async fn persist_avatar(game_path: &Path, nickname: &str, bytes: &[u8]) -> Result<(), AppError> {
    atomic_bytes(&game_path.join("platform/avatar.dat"), bytes).await?;
    atomic_json(
        &game_path.join(AVATAR_STATE_PATH),
        &AvatarState {
            schema_version: 1,
            nickname: nickname.to_string(),
        },
    )
    .await
}

#[cfg(test)]
mod release_1_6_11_tests {
    use tempfile::tempdir;

    use super::{avatar_is_current, persist_avatar};

    #[tokio::test]
    async fn release_1_6_11_avatar_state_changes_only_after_successful_persist() {
        let directory = tempdir().expect("temporary game path should be created");
        assert!(!avatar_is_current(directory.path(), "player").await);

        persist_avatar(directory.path(), "player", b"png-bytes")
            .await
            .expect("avatar and state should be persisted");

        assert!(avatar_is_current(directory.path(), "player").await);
        assert!(!avatar_is_current(directory.path(), "other").await);
        assert_eq!(
            tokio::fs::read(directory.path().join("platform/avatar.dat"))
                .await
                .expect("avatar should exist"),
            b"png-bytes"
        );
    }
}
