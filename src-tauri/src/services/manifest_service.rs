use crate::error::AppError;
use crate::models::GameManifest;
use crate::services::api_client::ApiClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyMode {
    Full,
    AdditionalOnly,
    UpdateFromVersion(String),
}

pub fn update_mode_for_version(version: &str) -> VerifyMode {
    if version.trim().is_empty() || version == "0.0.0" {
        VerifyMode::Full
    } else {
        VerifyMode::UpdateFromVersion(version.to_string())
    }
}

pub async fn load_manifest(api: &ApiClient, mode: VerifyMode) -> Result<GameManifest, AppError> {
    match mode {
        VerifyMode::Full => api.get_full_manifest().await,
        VerifyMode::AdditionalOnly => api.get_additional_manifest().await,
        VerifyMode::UpdateFromVersion(version) => api.get_updates_from(&version).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{update_mode_for_version, VerifyMode};

    #[test]
    fn missing_version_requires_a_full_manifest() {
        assert!(matches!(update_mode_for_version(""), VerifyMode::Full));
        assert!(matches!(update_mode_for_version("0.0.0"), VerifyMode::Full));
    }

    #[test]
    fn known_version_uses_incremental_updates() {
        assert!(matches!(
            update_mode_for_version("1.5.0"),
            VerifyMode::UpdateFromVersion(version) if version == "1.5.0"
        ));
    }
}
