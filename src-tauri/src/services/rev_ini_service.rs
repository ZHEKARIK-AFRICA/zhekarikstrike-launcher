use std::path::PathBuf;

use crate::constants::REV_INI;
use crate::error::AppError;
use crate::models::GameData;
use crate::services::avatar_service::generate_avatar;

pub async fn read_rev_ini(game_path: PathBuf) -> Result<GameData, AppError> {
    let rev_ini_path = game_path.join(REV_INI);
    if !tokio::fs::try_exists(&rev_ini_path).await.unwrap_or(false) {
        return Ok(GameData {
            nickname: None,
            clan_tag: None,
            launch_params: None,
            game_path: game_path.to_string_lossy().to_string(),
        });
    }

    let content = tokio::fs::read_to_string(&rev_ini_path).await?;
    let mut nickname = None;
    let mut clan_tag = None;
    let mut launch_params = None;

    for line in content.lines() {
        if let Some(value) = line.strip_prefix("PlayerName=") {
            nickname = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("ClanTag=") {
            clan_tag = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("ProcName=") {
            let value = value.trim();
            launch_params = value
                .split_once("-steam")
                .map(|(_, params)| params.trim().to_string())
                .or_else(|| Some(String::new()));
        }
    }

    Ok(GameData {
        nickname,
        clan_tag,
        launch_params,
        game_path: game_path.to_string_lossy().to_string(),
    })
}

pub async fn update_rev_ini(
    game_path: PathBuf,
    nickname: String,
    clan_tag: String,
    launch_params: String,
    language: String,
) -> Result<(), AppError> {
    let rev_ini_path = game_path.join(REV_INI);
    if !tokio::fs::try_exists(&rev_ini_path).await.unwrap_or(false) {
        return Err(AppError::FileSystem(format!(
            "rev.ini not found: {}",
            rev_ini_path.display()
        )));
    }

    let language_value = if language == "ru" {
        "Russian"
    } else {
        "English"
    };
    let content = tokio::fs::read_to_string(&rev_ini_path).await?;
    let mut has_player_name = false;
    let mut has_clan_tag = false;
    let mut has_proc_name = false;
    let mut has_language = false;

    let mut lines = content
        .lines()
        .map(|line| {
            if line.starts_with("PlayerName=") {
                has_player_name = true;
                format!("PlayerName={nickname}")
            } else if line.starts_with("ClanTag=") {
                has_clan_tag = true;
                format!("ClanTag={clan_tag}")
            } else if line.starts_with("ProcName=") {
                has_proc_name = true;
                format!("ProcName=zhekarikstrike.exe -steam {launch_params}")
            } else if line.starts_with("Language = ") {
                has_language = true;
                format!("Language = {language_value}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();

    if !has_player_name {
        lines.push(format!("PlayerName={nickname}"));
    }
    if !has_clan_tag {
        lines.push(format!("ClanTag={clan_tag}"));
    }
    if !has_proc_name {
        lines.push(format!(
            "ProcName=zhekarikstrike.exe -steam {launch_params}"
        ));
    }
    if !has_language {
        lines.push(format!("Language = {language_value}"));
    }

    tokio::fs::write(&rev_ini_path, lines.join("\r\n")).await?;

    if let Err(error) = generate_avatar(game_path, nickname).await {
        eprintln!("avatar generation failed: {error}");
    }

    Ok(())
}
