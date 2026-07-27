use std::path::PathBuf;

use crate::constants::MODERN_API_BASE_URL;
use crate::error::AppError;

pub async fn generate_avatar(game_path: PathBuf, nickname: String) -> Result<(), AppError> {
    let platform_dir = game_path.join("platform");
    tokio::fs::create_dir_all(&platform_dir).await?;

    let bytes = reqwest::Client::new()
        .get(format!("{MODERN_API_BASE_URL}/create_image"))
        .query(&[("nickname", nickname)])
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    tokio::fs::write(platform_dir.join("avatar.dat"), bytes).await?;
    Ok(())
}
