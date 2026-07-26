use crate::error::AppError;
use crate::models::GameManifest;
use crate::services::api_client::ApiClient;

#[derive(Debug, Clone)]
pub enum VerifyMode {
    Full,
    AdditionalOnly,
    UpdateFromVersion(String),
}

pub async fn load_manifest(api: &ApiClient, mode: VerifyMode) -> Result<GameManifest, AppError> {
    match mode {
        VerifyMode::Full => api.get_full_manifest().await,
        VerifyMode::AdditionalOnly => api.get_additional_manifest().await,
        VerifyMode::UpdateFromVersion(version) => api.get_updates_from(&version).await,
    }
}
