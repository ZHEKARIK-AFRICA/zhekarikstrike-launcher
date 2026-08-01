use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::models::ContentManifest;
use crate::services::content_journal_service::{atomic_json, content_root, load_completion_state};
use crate::utils::path_utils::safe_join;

pub const PREREQUISITE_CATALOG_VERSION: u32 = 1;
const ANALYSIS_SCHEMA_VERSION: u8 = 2;
const ANALYSIS_PATH: &str = ".zhekarik/prerequisites/analysis-v1.json";

pub type ServiceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, PrerequisiteError>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct PrerequisiteServiceProgress {
    pub stage: &'static str,
    pub component_id: Option<String>,
    pub progress: Option<f64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub restart_recommended: bool,
}

pub type PrerequisiteProgressCallback = Arc<dyn Fn(PrerequisiteServiceProgress) + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum PrerequisiteError {
    #[error("prerequisite download failed: {0}")]
    Download(String),
    #[error("prerequisite verification failed: {0}")]
    Verification(String),
    #[error("prerequisite install failed: {0}")]
    Install(String),
    #[error("prerequisite restart required: {0}")]
    RestartRequired(String),
    #[error("prerequisite is unsupported: {0}")]
    Unsupported(String),
    #[error("prerequisite operation canceled")]
    Canceled,
}

impl PrerequisiteError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Download(_) => "prerequisite_download_failed",
            Self::Verification(_) => "prerequisite_verification_failed",
            Self::Install(_) => "prerequisite_install_failed",
            Self::RestartRequired(_) => "prerequisite_restart_required",
            Self::Unsupported(_) => "prerequisite_unsupported",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeArchitecture {
    X86,
    X64,
    Arm64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeImage {
    pub architecture: PeArchitecture,
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerSource {
    Remote(&'static str),
    GameLocal(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogComponent {
    pub id: &'static str,
    pub source: InstallerSource,
    pub size: u64,
    pub sha256: &'static str,
    pub required_version: Option<&'static str>,
    pub architecture: PeArchitecture,
}

impl CatalogComponent {
    pub fn remote_url(&self) -> Option<&'static str> {
        match self.source {
            InstallerSource::Remote(url) => Some(url),
            InstallerSource::GameLocal(_) => None,
        }
    }

    pub fn local_source(&self) -> Option<PathBuf> {
        match self.source {
            InstallerSource::GameLocal(path) => Some(PathBuf::from(path)),
            InstallerSource::Remote(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrerequisiteRequirement {
    pub component_id: String,
    pub architecture: PeArchitecture,
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisCache {
    pub schema_version: u8,
    pub content_sha256: String,
    pub catalog_version: u32,
    pub requirements: Vec<PrerequisiteRequirement>,
}

impl AnalysisCache {
    pub fn matches(&self, content_sha256: &str) -> bool {
        self.schema_version == ANALYSIS_SCHEMA_VERSION
            && self.catalog_version == PREREQUISITE_CATALOG_VERSION
            && self.content_sha256 == content_sha256
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnsurePrerequisitesResult {
    pub ready: bool,
    pub installed: Vec<String>,
    pub already_present: Vec<String>,
    pub restart_status: RestartStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartStatus {
    None,
    Recommended,
    Initiated,
}

impl RestartStatus {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Initiated, _) | (_, Self::Initiated) => Self::Initiated,
            (Self::Recommended, _) | (_, Self::Recommended) => Self::Recommended,
            (Self::None, Self::None) => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InstallCompletion {
    ready: bool,
    restart_status: RestartStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallerExit {
    Success,
    RestartRecommended,
    RestartInitiated,
}

pub trait InstallerDownloader: Send + Sync {
    fn download<'a>(
        &'a self,
        component: &'a CatalogComponent,
        target: &'a Path,
        cancel: &'a CancellationToken,
    ) -> ServiceFuture<'a, ()>;
}

pub trait TrustVerifier: Send + Sync {
    fn verify_microsoft(&self, path: &Path) -> Result<(), PrerequisiteError>;
}

pub trait RuntimeProbe: Send + Sync {
    fn system_dependency_path(
        &self,
        import: &str,
        architecture: PeArchitecture,
    ) -> Result<Option<PathBuf>, PrerequisiteError>;

    fn dependency_file_version(&self, path: &Path) -> Result<Option<String>, PrerequisiteError>;

    fn component_satisfied(
        &self,
        component: &CatalogComponent,
        architecture: PeArchitecture,
        imports: &[String],
    ) -> Result<bool, PrerequisiteError>;
}

pub trait InstallerRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        arguments: &'a [String],
        cancel: &'a CancellationToken,
    ) -> ServiceFuture<'a, i32>;
}

pub struct PrerequisiteService {
    downloader: Arc<dyn InstallerDownloader>,
    trust: Arc<dyn TrustVerifier>,
    runtime: Arc<dyn RuntimeProbe>,
    runner: Arc<dyn InstallerRunner>,
    progress: Option<PrerequisiteProgressCallback>,
}

impl PrerequisiteService {
    pub fn new(
        downloader: Arc<dyn InstallerDownloader>,
        trust: Arc<dyn TrustVerifier>,
        runtime: Arc<dyn RuntimeProbe>,
        runner: Arc<dyn InstallerRunner>,
    ) -> Self {
        Self {
            downloader,
            trust,
            runtime,
            runner,
            progress: None,
        }
    }

    pub fn windows() -> Result<Self, PrerequisiteError> {
        Ok(Self::new(
            Arc::new(HttpInstallerDownloader::new()?),
            Arc::new(WindowsTrustVerifier),
            Arc::new(SystemRuntimeProbe),
            Arc::new(ProcessInstallerRunner),
        ))
    }

    pub fn windows_with_progress(
        progress: PrerequisiteProgressCallback,
    ) -> Result<Self, PrerequisiteError> {
        Ok(Self {
            downloader: Arc::new(HttpInstallerDownloader::new_with_progress(
                progress.clone(),
            )?),
            trust: Arc::new(WindowsTrustVerifier),
            runtime: Arc::new(SystemRuntimeProbe),
            runner: Arc::new(ProcessInstallerRunner),
            progress: Some(progress),
        })
    }

    pub fn with_progress(mut self, progress: PrerequisiteProgressCallback) -> Self {
        self.progress = Some(progress);
        self
    }

    fn emit_progress(
        &self,
        stage: &'static str,
        component_id: Option<&str>,
        progress: Option<f64>,
        downloaded_bytes: Option<u64>,
        total_bytes: Option<u64>,
        restart_recommended: bool,
    ) {
        if let Some(callback) = &self.progress {
            callback(PrerequisiteServiceProgress {
                stage,
                component_id: component_id.map(str::to_string),
                progress,
                downloaded_bytes,
                total_bytes,
                restart_recommended,
            });
        }
    }

    pub async fn analyze_active(
        &self,
        game_root: &Path,
        cancel: &CancellationToken,
    ) -> Result<AnalysisCache, PrerequisiteError> {
        let manifest = load_active_manifest(game_root).await?;
        self.analyze(game_root, &manifest, cancel).await
    }

    async fn analyze(
        &self,
        game_root: &Path,
        manifest: &ContentManifest,
        cancel: &CancellationToken,
    ) -> Result<AnalysisCache, PrerequisiteError> {
        if cancel.is_cancelled() {
            return Err(PrerequisiteError::Canceled);
        }
        let cache_path = game_root.join(ANALYSIS_PATH);
        if let Ok(bytes) = tokio::fs::read(&cache_path).await {
            if let Ok(cache) = serde_json::from_slice::<AnalysisCache>(&bytes) {
                if cache.matches(&manifest.content_sha256) {
                    return Ok(cache);
                }
            }
        }

        let mut requirements = BTreeMap::<(String, PeArchitecture), BTreeSet<String>>::new();
        for file in manifest.files.iter().filter(|file| {
            Path::new(&file.path).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("dll")
            })
        }) {
            if cancel.is_cancelled() {
                return Err(PrerequisiteError::Canceled);
            }
            let module_path = safe_join(game_root, &file.path)
                .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
            let bytes = tokio::fs::read(&module_path).await.map_err(|error| {
                PrerequisiteError::Verification(format!(
                    "cannot inspect {}: {error}",
                    module_path.display()
                ))
            })?;
            let image = parse_pe_image(&bytes)?;
            for import in image.imports {
                let Some(component_id) = component_id_for_import(&import) else {
                    continue;
                };
                let component = catalog_component(component_id)
                    .ok_or_else(|| PrerequisiteError::Unsupported(component_id.to_string()))?;
                let app_local = app_local_dependency_paths(game_root, &module_path, &import)?;
                let system = self
                    .runtime
                    .system_dependency_path(&import, image.architecture)?;
                let mut app_local_satisfied = false;
                for path in &app_local {
                    if dependency_satisfies(
                        path,
                        &component,
                        image.architecture,
                        self.runtime.as_ref(),
                    )? {
                        app_local_satisfied = true;
                        break;
                    }
                }
                let system_satisfied = match system.as_deref() {
                    Some(path) => dependency_satisfies(
                        path,
                        &component,
                        image.architecture,
                        self.runtime.as_ref(),
                    )?,
                    None => false,
                };
                if app_local_satisfied || system_satisfied {
                    continue;
                }
                if component.architecture != image.architecture {
                    return Err(PrerequisiteError::Unsupported(format!(
                        "{} has no {:?} catalog entry required by {}",
                        component_id,
                        image.architecture,
                        module_path.display()
                    )));
                }
                requirements
                    .entry((component_id.to_string(), image.architecture))
                    .or_default()
                    .insert(import.to_ascii_lowercase());
            }
        }
        let cache = AnalysisCache {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            content_sha256: manifest.content_sha256.clone(),
            catalog_version: PREREQUISITE_CATALOG_VERSION,
            requirements: requirements
                .into_iter()
                .map(
                    |((component_id, architecture), imports)| PrerequisiteRequirement {
                        component_id,
                        architecture,
                        imports: imports.into_iter().collect(),
                    },
                )
                .collect(),
        };
        atomic_json(&cache_path, &cache)
            .await
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
        Ok(cache)
    }

    pub async fn ensure_active(
        &self,
        game_root: &Path,
        cancel: &CancellationToken,
    ) -> Result<EnsurePrerequisitesResult, PrerequisiteError> {
        let manifest = load_active_manifest(game_root).await?;
        self.ensure_manifest(game_root, &manifest, cancel).await
    }

    pub async fn check_active(
        &self,
        game_root: &Path,
        cancel: &CancellationToken,
    ) -> Result<EnsurePrerequisitesResult, PrerequisiteError> {
        let manifest = load_active_manifest(game_root).await?;
        self.check_manifest(game_root, &manifest, cancel).await
    }

    async fn check_manifest(
        &self,
        game_root: &Path,
        manifest: &ContentManifest,
        cancel: &CancellationToken,
    ) -> Result<EnsurePrerequisitesResult, PrerequisiteError> {
        let analysis = self.analyze(game_root, manifest, cancel).await?;
        let mut result = EnsurePrerequisitesResult {
            ready: true,
            installed: Vec::new(),
            already_present: Vec::new(),
            restart_status: RestartStatus::None,
        };
        for requirement in analysis.requirements {
            if cancel.is_cancelled() {
                return Err(PrerequisiteError::Canceled);
            }
            let component = catalog_component(&requirement.component_id)
                .ok_or_else(|| PrerequisiteError::Unsupported(requirement.component_id.clone()))?;
            if component.architecture != requirement.architecture {
                return Err(PrerequisiteError::Unsupported(format!(
                    "{} has no {:?} catalog entry",
                    requirement.component_id, requirement.architecture
                )));
            }
            if self.runtime.component_satisfied(
                &component,
                requirement.architecture,
                &requirement.imports,
            )? {
                result.already_present.push(requirement.component_id);
            } else {
                result.ready = false;
            }
        }
        Ok(result)
    }

    async fn ensure_manifest(
        &self,
        game_root: &Path,
        manifest: &ContentManifest,
        cancel: &CancellationToken,
    ) -> Result<EnsurePrerequisitesResult, PrerequisiteError> {
        self.emit_progress("detecting", None, Some(0.0), None, None, false);
        let analysis = self.analyze(game_root, manifest, cancel).await?;
        self.emit_progress("detecting", None, Some(100.0), None, None, false);
        let mut result = EnsurePrerequisitesResult {
            ready: true,
            installed: Vec::new(),
            already_present: Vec::new(),
            restart_status: RestartStatus::None,
        };
        for requirement in analysis.requirements {
            if cancel.is_cancelled() {
                return Err(PrerequisiteError::Canceled);
            }
            let component = catalog_component(&requirement.component_id)
                .ok_or_else(|| PrerequisiteError::Unsupported(requirement.component_id.clone()))?;
            if component.architecture != requirement.architecture {
                return Err(PrerequisiteError::Unsupported(format!(
                    "{} has no {:?} catalog entry",
                    requirement.component_id, requirement.architecture
                )));
            }
            self.emit_progress(
                "verifying",
                Some(&requirement.component_id),
                Some(0.0),
                None,
                None,
                false,
            );
            if self.runtime.component_satisfied(
                &component,
                requirement.architecture,
                &requirement.imports,
            )? {
                self.emit_progress(
                    "verifying",
                    Some(&requirement.component_id),
                    Some(100.0),
                    None,
                    None,
                    false,
                );
                result.already_present.push(requirement.component_id);
                continue;
            }
            let exit = self
                .install_component(
                    game_root,
                    Some(manifest),
                    &component,
                    &requirement.imports,
                    cancel,
                )
                .await?;
            let completion = self
                .complete_install(
                    &component,
                    requirement.architecture,
                    &requirement.imports,
                    exit,
                )
                .await?;
            result.restart_status = result.restart_status.merge(completion.restart_status);
            result.ready &= completion.ready;
            result.installed.push(requirement.component_id);
        }
        self.emit_progress(
            "complete",
            None,
            Some(100.0),
            None,
            None,
            result.restart_status != RestartStatus::None,
        );
        Ok(result)
    }

    async fn install_component(
        &self,
        game_root: &Path,
        manifest: Option<&ContentManifest>,
        component: &CatalogComponent,
        _imports: &[String],
        cancel: &CancellationToken,
    ) -> Result<InstallerExit, PrerequisiteError> {
        if cancel.is_cancelled() {
            return Err(PrerequisiteError::Canceled);
        }
        let artifact = match component.source {
            InstallerSource::Remote(_) => {
                let cache = game_root.join(".zhekarik/prerequisites/cache");
                let target = cache.join(format!("{}.exe", component.id));
                self.emit_progress(
                    "downloading",
                    Some(component.id),
                    Some(0.0),
                    Some(0),
                    Some(component.size),
                    false,
                );
                self.downloader.download(component, &target, cancel).await?;
                self.emit_progress(
                    "downloading",
                    Some(component.id),
                    Some(100.0),
                    Some(component.size),
                    Some(component.size),
                    false,
                );
                target
            }
            InstallerSource::GameLocal(relative) => {
                let manifest = manifest.ok_or_else(|| {
                    PrerequisiteError::Verification("active content manifest is required".into())
                })?;
                let entry = manifest
                    .files
                    .iter()
                    .find(|file| file.path == relative)
                    .ok_or_else(|| {
                        PrerequisiteError::Verification(format!(
                            "{} is not in the active content manifest",
                            relative
                        ))
                    })?;
                if entry.size != component.size || entry.sha256 != component.sha256 {
                    return Err(PrerequisiteError::Verification(format!(
                        "active content metadata for {relative} does not match the catalog"
                    )));
                }
                safe_join(game_root, relative)
                    .map_err(|error| PrerequisiteError::Verification(error.to_string()))?
            }
        };
        if cancel.is_cancelled() {
            return Err(PrerequisiteError::Canceled);
        }
        self.emit_progress(
            "verifying",
            Some(component.id),
            Some(0.0),
            None,
            None,
            false,
        );
        verify_execution_artifact(
            &artifact,
            component.size,
            component.sha256,
            component.architecture,
            self.trust.as_ref(),
        )?;
        self.emit_progress(
            "verifying",
            Some(component.id),
            Some(100.0),
            None,
            None,
            false,
        );

        self.emit_progress(
            "installing",
            Some(component.id),
            Some(0.0),
            None,
            None,
            false,
        );

        let result = match component.id {
            "vc2010-sp1-x86" => {
                let arguments = vec!["/quiet".into(), "/norestart".into()];
                classify_installer_exit(self.runner.run(&artifact, &arguments, cancel).await?)
            }
            "directx-june-2010" => {
                self.install_directx(game_root, &artifact, component.architecture, cancel)
                    .await
            }
            _ => Err(PrerequisiteError::Unsupported(component.id.into())),
        };
        if let Ok(exit) = result {
            self.emit_progress(
                "installing",
                Some(component.id),
                Some(100.0),
                None,
                None,
                exit != InstallerExit::Success,
            );
        }
        result
    }

    async fn install_directx(
        &self,
        game_root: &Path,
        redist: &Path,
        architecture: PeArchitecture,
        cancel: &CancellationToken,
    ) -> Result<InstallerExit, PrerequisiteError> {
        let temp = game_root
            .join(".zhekarik/prerequisites/temp")
            .join(Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&temp)
            .await
            .map_err(|error| PrerequisiteError::Install(error.to_string()))?;
        let result = async {
            let arguments = vec!["/Q".into(), format!("/T:{}", temp.display())];
            let extract_exit =
                classify_installer_exit(self.runner.run(redist, &arguments, cancel).await?)?;
            if cancel.is_cancelled() {
                return Err(PrerequisiteError::Canceled);
            }
            let setup = temp.join("DXSETUP.exe");
            let metadata = inspect_pe_file(&setup)?;
            if metadata.architecture != architecture {
                return Err(PrerequisiteError::Verification(
                    "DXSETUP.exe architecture mismatch".into(),
                ));
            }
            self.trust.verify_microsoft(&setup)?;
            let setup_arguments = vec!["/silent".into()];
            let setup_exit =
                classify_installer_exit(self.runner.run(&setup, &setup_arguments, cancel).await?)?;
            Ok(combine_installer_exits(extract_exit, setup_exit))
        }
        .await;
        if let Err(error) = tokio::fs::remove_dir_all(&temp).await {
            if result.is_ok() {
                return Err(PrerequisiteError::Install(format!(
                    "failed to clean DirectX temporary files: {error}"
                )));
            }
        }
        result
    }

    async fn complete_install(
        &self,
        component: &CatalogComponent,
        architecture: PeArchitecture,
        imports: &[String],
        exit: InstallerExit,
    ) -> Result<InstallCompletion, PrerequisiteError> {
        let ready = self
            .runtime
            .component_satisfied(component, architecture, imports)?;
        if !ready && exit == InstallerExit::Success {
            return Err(PrerequisiteError::Install(format!(
                "{} completed but the required DLL architecture/version is still unavailable",
                component.id
            )));
        }
        let restart_status = match exit {
            InstallerExit::Success => RestartStatus::None,
            InstallerExit::RestartRecommended => RestartStatus::Recommended,
            InstallerExit::RestartInitiated => RestartStatus::Initiated,
        };
        Ok(InstallCompletion {
            ready,
            restart_status,
        })
    }
}

pub async fn load_active_manifest(game_root: &Path) -> Result<ContentManifest, PrerequisiteError> {
    let result = async {
        let state = load_completion_state(game_root)
            .await
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?
            .ok_or_else(|| {
                PrerequisiteError::Unsupported("active content state is missing".into())
            })?;
        let valid_sha = state.content_sha256.len() == 64
            && state
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if state.schema_version != 1 || !valid_sha {
            return Err(PrerequisiteError::Verification(
                "active content state is invalid".into(),
            ));
        }
        let path = content_root(game_root)
            .join("manifests")
            .join(format!("{}.json", state.content_sha256));
        let bytes = tokio::fs::read(&path).await.map_err(|error| {
            PrerequisiteError::Verification(format!(
                "active content manifest is unavailable: {error}"
            ))
        })?;
        let manifest: ContentManifest = serde_json::from_slice(&bytes).map_err(|error| {
            PrerequisiteError::Verification(format!("active content manifest is invalid: {error}"))
        })?;
        manifest
            .validate()
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
        if manifest.content_sha256 != state.content_sha256
            || manifest.release_id != state.release_id
            || manifest.game_version != state.game_version
        {
            return Err(PrerequisiteError::Verification(
                "active content state does not match its manifest".into(),
            ));
        }
        Ok(manifest)
    }
    .await;
    if result.is_err() {
        tokio::fs::remove_file(game_root.join(ANALYSIS_PATH))
            .await
            .ok();
    }
    result
}

pub fn catalog_component(id: &str) -> Option<CatalogComponent> {
    match id {
        "vc2010-sp1-x86" => Some(CatalogComponent {
            id: "vc2010-sp1-x86",
            source: InstallerSource::Remote("https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x86.exe"),
            size: 8_993_744,
            sha256: "99dce3c841cc6028560830f7866c9ce2928c98cf3256892ef8e6cf755147b0d8",
            required_version: Some("10.0.40219.325"),
            architecture: PeArchitecture::X86,
        }),
        "directx-june-2010" => Some(CatalogComponent {
            id: "directx-june-2010",
            source: InstallerSource::GameLocal("directx_installer/directx_jun2010_redist.exe"),
            size: 100_271_992,
            sha256: "8746ee1a84a083a90e37899d71d50d5c7c015e69688a466aa80447f011780c0d",
            required_version: None,
            architecture: PeArchitecture::X86,
        }),
        _ => None,
    }
}

fn component_id_for_import(import: &str) -> Option<&'static str> {
    let import = import.to_ascii_lowercase();
    let vc = matches!(
        import.as_str(),
        "msvcr100.dll"
            | "msvcp100.dll"
            | "atl100.dll"
            | "mfc100.dll"
            | "mfc100u.dll"
            | "mfcm100.dll"
            | "mfcm100u.dll"
    );
    if vc {
        return Some("vc2010-sp1-x86");
    }
    let directx = matches!(
        import.as_str(),
        "xinput1_3.dll" | "xaudio2_7.dll" | "d3dcompiler_43.dll"
    ) || numbered_import(&import, "d3dx9_", 24, 43)
        || import == "d3dx10.dll"
        || numbered_import(&import, "d3dx10_", 33, 43)
        || numbered_import(&import, "d3dx11_", 42, 43)
        || numbered_import(&import, "xapofx1_", 0, 5);
    directx.then_some("directx-june-2010")
}

fn numbered_import(import: &str, prefix: &str, minimum: u8, maximum: u8) -> bool {
    import
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_suffix(".dll"))
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (minimum..=maximum).contains(&number))
}

pub fn component_ids_for_imports(imports: &[String]) -> Vec<&'static str> {
    imports
        .iter()
        .filter_map(|import| component_id_for_import(import))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn parse_pe_image(bytes: &[u8]) -> Result<PeImage, PrerequisiteError> {
    if bytes.get(0..2) != Some(b"MZ") {
        return verification("file is not a DOS/PE image");
    }
    let pe_offset = read_u32(bytes, 0x3c)? as usize;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return verification("invalid PE signature");
    }
    let architecture = match read_u16(bytes, pe_offset + 4)? {
        0x014c => PeArchitecture::X86,
        0x8664 => PeArchitecture::X64,
        0xaa64 => PeArchitecture::Arm64,
        machine => return verification(&format!("unsupported PE machine 0x{machine:04x}")),
    };
    let section_count = read_u16(bytes, pe_offset + 6)? as usize;
    let optional_size = read_u16(bytes, pe_offset + 20)? as usize;
    let optional = pe_offset + 24;
    let directory = match read_u16(bytes, optional)? {
        0x10b => optional + 104,
        0x20b => optional + 120,
        _ => return verification("unsupported PE optional header"),
    };
    if directory + 8 > optional + optional_size {
        return verification("PE import directory is outside the optional header");
    }
    let import_rva = read_u32(bytes, directory)?;
    if import_rva == 0 {
        return Ok(PeImage {
            architecture,
            imports: Vec::new(),
        });
    }
    let sections = parse_sections(bytes, optional + optional_size, section_count)?;
    let mut descriptor = rva_to_offset(import_rva, &sections)?;
    let mut imports = BTreeSet::new();
    loop {
        let fields = bytes
            .get(descriptor..descriptor + 20)
            .ok_or_else(|| PrerequisiteError::Verification("truncated import descriptor".into()))?;
        if fields.iter().all(|byte| *byte == 0) {
            break;
        }
        let name_rva = read_u32(bytes, descriptor + 12)?;
        let name_offset = rva_to_offset(name_rva, &sections)?;
        let name = read_c_string(bytes, name_offset)?;
        if !name.is_empty() {
            imports.insert(name.to_ascii_lowercase());
        }
        descriptor = descriptor
            .checked_add(20)
            .ok_or_else(|| PrerequisiteError::Verification("import table overflow".into()))?;
    }
    Ok(PeImage {
        architecture,
        imports: imports.into_iter().collect(),
    })
}

#[derive(Clone, Copy)]
struct PeSection {
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

fn parse_sections(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<PeSection>, PrerequisiteError> {
    (0..count)
        .map(|index| {
            let section = offset + index * 40;
            Ok(PeSection {
                virtual_size: read_u32(bytes, section + 8)?,
                virtual_address: read_u32(bytes, section + 12)?,
                raw_size: read_u32(bytes, section + 16)?,
                raw_offset: read_u32(bytes, section + 20)?,
            })
        })
        .collect()
}

fn rva_to_offset(rva: u32, sections: &[PeSection]) -> Result<usize, PrerequisiteError> {
    sections
        .iter()
        .find_map(|section| {
            let size = section.virtual_size.max(section.raw_size);
            let relative = rva.checked_sub(section.virtual_address)?;
            (relative < size)
                .then(|| section.raw_offset.checked_add(relative))
                .flatten()
                .map(|offset| offset as usize)
        })
        .ok_or_else(|| PrerequisiteError::Verification(format!("unmapped PE RVA 0x{rva:08x}")))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PrerequisiteError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| PrerequisiteError::Verification("truncated PE image".into()))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PrerequisiteError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| PrerequisiteError::Verification("truncated PE image".into()))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_c_string(bytes: &[u8], offset: usize) -> Result<String, PrerequisiteError> {
    let tail = bytes
        .get(offset..)
        .ok_or_else(|| PrerequisiteError::Verification("invalid PE string offset".into()))?;
    let length = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| PrerequisiteError::Verification("unterminated PE import name".into()))?;
    std::str::from_utf8(&tail[..length])
        .map(|value| value.to_string())
        .map_err(|_| PrerequisiteError::Verification("non-ASCII PE import name".into()))
}

pub fn app_local_dependency_satisfied(
    game_root: &Path,
    module_path: &Path,
    import: &str,
    architecture: PeArchitecture,
) -> Result<bool, PrerequisiteError> {
    for path in app_local_dependency_paths(game_root, module_path, import)? {
        if inspect_pe_file(&path)?.architecture == architecture {
            return Ok(true);
        }
    }
    Ok(false)
}

fn app_local_dependency_paths(
    game_root: &Path,
    module_path: &Path,
    import: &str,
) -> Result<Vec<PathBuf>, PrerequisiteError> {
    if import.is_empty()
        || import.contains('/')
        || import.contains('\\')
        || import == "."
        || import == ".."
    {
        return verification("invalid imported DLL name");
    }
    let mut candidates = Vec::new();
    if let Some(parent) = module_path.parent() {
        candidates.push(parent.join(import));
    }
    candidates.push(game_root.join(import));
    candidates.push(game_root.join("bin").join(import));
    Ok(candidates
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .collect())
}

fn dependency_satisfies(
    path: &Path,
    component: &CatalogComponent,
    architecture: PeArchitecture,
    runtime: &dyn RuntimeProbe,
) -> Result<bool, PrerequisiteError> {
    if inspect_pe_file(path)?.architecture != architecture {
        return Ok(false);
    }
    let Some(required) = component.required_version else {
        return Ok(true);
    };
    let Some(actual) = runtime.dependency_file_version(path)? else {
        return Ok(false);
    };
    Ok(version_at_least(&actual, required))
}

fn inspect_pe_file(path: &Path) -> Result<PeImage, PrerequisiteError> {
    let bytes = std::fs::read(path).map_err(|error| {
        PrerequisiteError::Verification(format!("cannot read {}: {error}", path.display()))
    })?;
    parse_pe_image(&bytes)
}

pub fn validate_remote_source(
    component: &CatalogComponent,
    requested_url: &str,
) -> Result<(), PrerequisiteError> {
    let expected = component.remote_url().ok_or_else(|| {
        PrerequisiteError::Verification(format!("{} is not a remote component", component.id))
    })?;
    let parsed = reqwest::Url::parse(requested_url)
        .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
    if requested_url != expected
        || parsed.scheme() != "https"
        || parsed.host_str() != Some("download.microsoft.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return verification("remote prerequisite URL is not the pinned Microsoft URL");
    }
    Ok(())
}

pub fn validate_remote_response(
    requested_url: &str,
    final_url: &str,
    status: u16,
    content_length: Option<u64>,
    expected_size: u64,
) -> Result<(), PrerequisiteError> {
    if status != 200 {
        return Err(PrerequisiteError::Download(format!(
            "unexpected HTTP status {status}"
        )));
    }
    if final_url != requested_url {
        return Err(PrerequisiteError::Download(
            "redirected prerequisite response rejected".into(),
        ));
    }
    if content_length.is_some_and(|length| length != expected_size) {
        return Err(PrerequisiteError::Download(
            "prerequisite Content-Length mismatch".into(),
        ));
    }
    Ok(())
}

pub fn validate_artifact_integrity(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), PrerequisiteError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        PrerequisiteError::Verification(format!("cannot stat {}: {error}", path.display()))
    })?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return verification("prerequisite installer size mismatch");
    }
    let mut file = std::fs::File::open(path)
        .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != expected_sha256 {
        return verification("prerequisite installer SHA-256 mismatch");
    }
    Ok(())
}

pub fn verify_execution_artifact(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    expected_architecture: PeArchitecture,
    trust: &dyn TrustVerifier,
) -> Result<(), PrerequisiteError> {
    validate_artifact_integrity(path, expected_size, expected_sha256)?;
    if inspect_pe_file(path)?.architecture != expected_architecture {
        return verification("prerequisite installer architecture mismatch");
    }
    trust.verify_microsoft(path)
}

pub fn classify_installer_exit(code: i32) -> Result<InstallerExit, PrerequisiteError> {
    match code {
        0 => Ok(InstallerExit::Success),
        3010 => Ok(InstallerExit::RestartRecommended),
        1641 => Ok(InstallerExit::RestartInitiated),
        other => Err(PrerequisiteError::Install(format!(
            "installer exited with code {other}"
        ))),
    }
}

fn combine_installer_exits(left: InstallerExit, right: InstallerExit) -> InstallerExit {
    if left == InstallerExit::RestartInitiated || right == InstallerExit::RestartInitiated {
        InstallerExit::RestartInitiated
    } else if left == InstallerExit::RestartRecommended
        || right == InstallerExit::RestartRecommended
    {
        InstallerExit::RestartRecommended
    } else {
        InstallerExit::Success
    }
}

fn verification<T>(message: &str) -> Result<T, PrerequisiteError> {
    Err(PrerequisiteError::Verification(message.to_string()))
}

pub struct HttpInstallerDownloader {
    client: reqwest::Client,
    progress: Option<PrerequisiteProgressCallback>,
}

impl HttpInstallerDownloader {
    pub fn new() -> Result<Self, PrerequisiteError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(15 * 60))
            .user_agent(concat!(
                "ZHEKARIK-STRIKE-Prerequisites/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
        Ok(Self {
            client,
            progress: None,
        })
    }

    fn new_with_progress(
        progress: PrerequisiteProgressCallback,
    ) -> Result<Self, PrerequisiteError> {
        let mut downloader = Self::new()?;
        downloader.progress = Some(progress);
        Ok(downloader)
    }
}

impl InstallerDownloader for HttpInstallerDownloader {
    fn download<'a>(
        &'a self,
        component: &'a CatalogComponent,
        target: &'a Path,
        cancel: &'a CancellationToken,
    ) -> ServiceFuture<'a, ()> {
        Box::pin(async move {
            let url = component.remote_url().ok_or_else(|| {
                PrerequisiteError::Download("component has no remote source".into())
            })?;
            validate_remote_source(component, url)?;
            if validate_artifact_integrity(target, component.size, component.sha256).is_ok() {
                return Ok(());
            }
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
            }
            if tokio::fs::try_exists(target).await.unwrap_or(false) {
                tokio::fs::remove_file(target)
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
            }
            let part = target.with_extension("exe.part");
            let operation = async {
                if cancel.is_cancelled() {
                    return Err(PrerequisiteError::Canceled);
                }
                let response = tokio::select! {
                    _ = cancel.cancelled() => return Err(PrerequisiteError::Canceled),
                    response = self.client.get(url).send() => response
                        .map_err(|error| PrerequisiteError::Download(error.to_string()))?,
                };
                validate_remote_response(
                    url,
                    response.url().as_str(),
                    response.status().as_u16(),
                    response.content_length(),
                    component.size,
                )?;
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(&part)
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                let mut stream = response.bytes_stream();
                let mut size = 0_u64;
                let mut hasher = Sha256::new();
                loop {
                    let next = tokio::select! {
                        _ = cancel.cancelled() => return Err(PrerequisiteError::Canceled),
                        next = stream.next() => next,
                    };
                    let Some(chunk) = next else { break };
                    let chunk =
                        chunk.map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                    size = size.checked_add(chunk.len() as u64).ok_or_else(|| {
                        PrerequisiteError::Download("download size overflow".into())
                    })?;
                    if size > component.size {
                        return Err(PrerequisiteError::Download(
                            "download exceeded the pinned size".into(),
                        ));
                    }
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                    if let Some(callback) = &self.progress {
                        callback(PrerequisiteServiceProgress {
                            stage: "downloading",
                            component_id: Some(component.id.to_string()),
                            progress: Some((size as f64 / component.size as f64) * 100.0),
                            downloaded_bytes: Some(size),
                            total_bytes: Some(component.size),
                            restart_recommended: false,
                        });
                    }
                }
                file.flush()
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                file.sync_all()
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                drop(file);
                if size != component.size || hex::encode(hasher.finalize()) != component.sha256 {
                    return Err(PrerequisiteError::Verification(
                        "downloaded prerequisite size or SHA-256 mismatch".into(),
                    ));
                }
                tokio::fs::rename(&part, target)
                    .await
                    .map_err(|error| PrerequisiteError::Download(error.to_string()))?;
                Ok(())
            }
            .await;
            if operation.is_err() {
                tokio::fs::remove_file(&part).await.ok();
            }
            operation
        })
    }
}

pub struct ProcessInstallerRunner;

impl InstallerRunner for ProcessInstallerRunner {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        arguments: &'a [String],
        cancel: &'a CancellationToken,
    ) -> ServiceFuture<'a, i32> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(PrerequisiteError::Canceled);
            }
            let mut command = tokio::process::Command::new(program);
            command.args(arguments);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.as_std_mut().creation_flags(0x0800_0000);
            }
            let mut child = command
                .spawn()
                .map_err(|error| PrerequisiteError::Install(error.to_string()))?;
            // Cancellation is intentionally ignored after spawn: Windows runtime installers
            // must be allowed to finish. The close lifecycle waits up to ten seconds and a
            // later launch performs a fresh runtime check.
            let status = child
                .wait()
                .await
                .map_err(|error| PrerequisiteError::Install(error.to_string()))?;
            Ok(status.code().unwrap_or(-1))
        })
    }
}

pub struct SystemRuntimeProbe;

impl RuntimeProbe for SystemRuntimeProbe {
    fn system_dependency_path(
        &self,
        import: &str,
        architecture: PeArchitecture,
    ) -> Result<Option<PathBuf>, PrerequisiteError> {
        Ok(system_dll_path(import, architecture))
    }

    fn dependency_file_version(&self, path: &Path) -> Result<Option<String>, PrerequisiteError> {
        windows_file_version(path)
    }

    fn component_satisfied(
        &self,
        component: &CatalogComponent,
        architecture: PeArchitecture,
        imports: &[String],
    ) -> Result<bool, PrerequisiteError> {
        for import in imports {
            let Some(path) = system_dll_path(import, architecture) else {
                return Ok(false);
            };
            if inspect_pe_file(&path)?.architecture != architecture {
                return Ok(false);
            }
            if let Some(required) = component.required_version {
                let Some(actual) = self.dependency_file_version(&path)? else {
                    return Ok(false);
                };
                if !version_at_least(&actual, required) {
                    return Ok(false);
                }
            }
        }
        Ok(!imports.is_empty())
    }
}

fn system_dll_path(import: &str, architecture: PeArchitecture) -> Option<PathBuf> {
    if import.contains('/') || import.contains('\\') {
        return None;
    }
    let windows = PathBuf::from(std::env::var_os("WINDIR")?);
    let directory = match architecture {
        PeArchitecture::X86
            if std::env::var_os("PROCESSOR_ARCHITEW6432").is_some()
                || std::env::var("PROCESSOR_ARCHITECTURE")
                    .ok()
                    .is_some_and(|value| value.contains("64")) =>
        {
            windows.join("SysWOW64")
        }
        _ => windows.join("System32"),
    };
    let path = directory.join(import);
    path.is_file().then_some(path)
}

fn version_at_least(actual: &str, required: &str) -> bool {
    fn parts(value: &str) -> Option<Vec<u32>> {
        value
            .split('.')
            .map(|part| part.trim().parse::<u32>().ok())
            .collect()
    }
    match (parts(actual), parts(required)) {
        (Some(mut actual), Some(mut required)) => {
            let length = actual.len().max(required.len());
            actual.resize(length, 0);
            required.resize(length, 0);
            actual >= required
        }
        _ => false,
    }
}

fn windows_file_version(path: &Path) -> Result<Option<String>, PrerequisiteError> {
    #[cfg(windows)]
    {
        let output = StdCommand::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.Write([Diagnostics.FileVersionInfo]::GetVersionInfo($env:ZHEKARIK_PREREQUISITE_FILE).FileVersion)",
            ])
            .env("ZHEKARIK_PREREQUISITE_FILE", path)
            .output()
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
        if !output.status.success() {
            return Ok(None);
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!value.is_empty()).then_some(value));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(None)
    }
}

pub struct WindowsTrustVerifier;

impl TrustVerifier for WindowsTrustVerifier {
    fn verify_microsoft(&self, path: &Path) -> Result<(), PrerequisiteError> {
        win_verify_trust(path)?;
        let subject = signer_subject(path)?;
        if !is_microsoft_signer(&subject) {
            return verification("prerequisite signer is not Microsoft");
        }
        Ok(())
    }
}

fn is_microsoft_signer(subject: &str) -> bool {
    subject.split(',').map(str::trim).any(|attribute| {
        attribute.eq_ignore_ascii_case("CN=Microsoft Corporation")
            || attribute.eq_ignore_ascii_case("CN=Microsoft Windows")
            || attribute.eq_ignore_ascii_case("CN=Microsoft Windows Publisher")
    })
}

#[cfg(windows)]
fn win_verify_trust(path: &Path) -> Result<(), PrerequisiteError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_IGNORE, WTD_UI_NONE,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut file = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_IGNORE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status =
        unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _) };
    if status != 0 {
        return verification(&format!(
            "WinVerifyTrust rejected installer: 0x{status:08x}"
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn win_verify_trust(_path: &Path) -> Result<(), PrerequisiteError> {
    Err(PrerequisiteError::Unsupported(
        "WinVerifyTrust is only available on Windows".into(),
    ))
}

fn signer_subject(path: &Path) -> Result<String, PrerequisiteError> {
    #[cfg(windows)]
    {
        let output = StdCommand::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$s=Get-AuthenticodeSignature -LiteralPath $env:ZHEKARIK_PREREQUISITE_FILE; if ($null -eq $s.SignerCertificate) { exit 2 }; [Console]::Out.Write($s.SignerCertificate.Subject)",
            ])
            .env("ZHEKARIK_PREREQUISITE_FILE", path)
            .output()
            .map_err(|error| PrerequisiteError::Verification(error.to_string()))?;
        if !output.status.success() {
            return verification("installer has no readable Authenticode signer");
        }
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(PrerequisiteError::Unsupported(
            "Authenticode is only available on Windows".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::models::{
        ContentChunking, ContentCompression, ContentDelivery, ContentFile, ContentManifest,
    };

    fn analysis_manifest(content_sha256: &str, paths: &[&str]) -> ContentManifest {
        ContentManifest {
            schema_version: 2,
            content_sha256: content_sha256.to_string(),
            release_id: "1.0.0-r1".into(),
            game_version: "1.0.0".into(),
            generated_at: "2026-08-01T00:00:00Z".into(),
            source_archive_sha256: "f".repeat(64),
            delivery: ContentDelivery {
                chunk_base_url: "https://zhekarik.africa/chunks".into(),
                recommended_concurrency: 1,
            },
            chunking: ContentChunking {
                profile: "fixed-v1".into(),
                chunk_size: crate::models::CONTENT_CHUNK_SIZE,
            },
            compression: ContentCompression {
                profile: "zstd-v1".into(),
                level: 6,
                frame_checksum: true,
            },
            download_size: 0,
            unpacked_size: 0,
            chunks: HashMap::new(),
            files: paths
                .iter()
                .map(|path| ContentFile {
                    path: (*path).to_string(),
                    size: 0,
                    sha256: "e".repeat(64),
                    excluded_from_hash_check: false,
                    temporary: false,
                    additional_check: false,
                    chunks: Vec::new(),
                })
                .collect(),
        }
    }

    fn fixture_pe(architecture: PeArchitecture, imports: &[&str]) -> Vec<u8> {
        let pe_offset = 0x80_usize;
        let optional_size = match architecture {
            PeArchitecture::X86 => 0xe0_usize,
            PeArchitecture::X64 | PeArchitecture::Arm64 => 0xf0_usize,
        };
        let optional_offset = pe_offset + 24;
        let section_offset = optional_offset + optional_size;
        let raw_offset = 0x200_usize;
        let mut bytes = vec![0_u8; 0x800];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(pe_offset as u32).to_le_bytes());
        bytes[pe_offset..pe_offset + 4].copy_from_slice(b"PE\0\0");
        let machine = match architecture {
            PeArchitecture::X86 => 0x014c_u16,
            PeArchitecture::X64 => 0x8664_u16,
            PeArchitecture::Arm64 => 0xaa64_u16,
        };
        bytes[pe_offset + 4..pe_offset + 6].copy_from_slice(&machine.to_le_bytes());
        bytes[pe_offset + 6..pe_offset + 8].copy_from_slice(&1_u16.to_le_bytes());
        bytes[pe_offset + 20..pe_offset + 22]
            .copy_from_slice(&(optional_size as u16).to_le_bytes());
        let (magic, directory_offset) = match architecture {
            PeArchitecture::X86 => (0x10b_u16, optional_offset + 104),
            PeArchitecture::X64 | PeArchitecture::Arm64 => (0x20b_u16, optional_offset + 120),
        };
        bytes[optional_offset..optional_offset + 2].copy_from_slice(&magic.to_le_bytes());
        bytes[directory_offset..directory_offset + 4].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[directory_offset + 4..directory_offset + 8]
            .copy_from_slice(&(((imports.len() + 1) * 20) as u32).to_le_bytes());
        bytes[section_offset..section_offset + 8].copy_from_slice(b".rdata\0\0");
        bytes[section_offset + 8..section_offset + 12].copy_from_slice(&0x600_u32.to_le_bytes());
        bytes[section_offset + 12..section_offset + 16].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[section_offset + 16..section_offset + 20].copy_from_slice(&0x600_u32.to_le_bytes());
        bytes[section_offset + 20..section_offset + 24]
            .copy_from_slice(&(raw_offset as u32).to_le_bytes());

        let mut string_rva = 0x1200_u32;
        for (index, import) in imports.iter().enumerate() {
            let descriptor = raw_offset + index * 20;
            bytes[descriptor + 12..descriptor + 16].copy_from_slice(&string_rva.to_le_bytes());
            let string_offset = raw_offset + (string_rva - 0x1000) as usize;
            bytes[string_offset..string_offset + import.len()].copy_from_slice(import.as_bytes());
            bytes[string_offset + import.len()] = 0;
            string_rva += import.len() as u32 + 1;
        }
        bytes
    }

    #[test]
    fn pe_scanner_detects_architecture_and_maps_only_allowlisted_imports() {
        let image = parse_pe_image(&fixture_pe(
            PeArchitecture::X86,
            &["MSVCR100.dll", "xinput1_3.DLL", "unknown_vendor.dll"],
        ))
        .expect("fixture PE should parse");

        assert_eq!(image.architecture, PeArchitecture::X86);
        assert_eq!(
            component_ids_for_imports(&image.imports),
            vec!["directx-june-2010", "vc2010-sp1-x86"]
        );
    }

    #[test]
    fn app_local_xinput_in_bin_satisfies_a_matching_import() {
        let directory = tempdir().expect("temp directory should exist");
        let module = directory.path().join("game").join("client.dll");
        fs::create_dir_all(module.parent().unwrap()).expect("module directory should exist");
        fs::create_dir_all(directory.path().join("bin")).expect("bin directory should exist");
        fs::write(
            directory.path().join("bin").join("xinput1_3.dll"),
            fixture_pe(PeArchitecture::X86, &[]),
        )
        .expect("local DLL should be written");

        assert!(app_local_dependency_satisfied(
            directory.path(),
            &module,
            "xinput1_3.dll",
            PeArchitecture::X86,
        )
        .expect("local resolution should succeed"));
    }

    #[test]
    fn analysis_cache_requires_matching_content_and_catalog_versions() {
        let cache = AnalysisCache {
            schema_version: ANALYSIS_SCHEMA_VERSION,
            content_sha256: "a".repeat(64),
            catalog_version: PREREQUISITE_CATALOG_VERSION,
            requirements: vec![],
        };

        assert!(cache.matches(&"a".repeat(64)));
        assert!(!cache.matches(&"b".repeat(64)));
        let mut old_catalog = cache.clone();
        old_catalog.catalog_version -= 1;
        assert!(!old_catalog.matches(&"a".repeat(64)));
        let mut invalid_schema = cache;
        invalid_schema.schema_version = 0;
        assert!(!invalid_schema.matches(&"a".repeat(64)));
    }

    #[tokio::test]
    async fn missing_active_content_state_invalidates_an_existing_analysis_cache() {
        let directory = tempdir().expect("temp directory should exist");
        let cache_path = directory
            .path()
            .join(".zhekarik/prerequisites/analysis-v1.json");
        fs::create_dir_all(cache_path.parent().unwrap()).expect("cache directory should exist");
        fs::write(&cache_path, b"stale analysis").expect("stale cache should be written");

        assert!(load_active_manifest(directory.path()).await.is_err());
        assert!(!cache_path.exists());
    }

    struct VersionedDependencyProbe {
        system_path: Option<PathBuf>,
        versions: HashMap<PathBuf, String>,
    }

    impl RuntimeProbe for VersionedDependencyProbe {
        fn system_dependency_path(
            &self,
            _import: &str,
            _architecture: PeArchitecture,
        ) -> Result<Option<PathBuf>, PrerequisiteError> {
            Ok(self.system_path.clone())
        }

        fn dependency_file_version(
            &self,
            path: &Path,
        ) -> Result<Option<String>, PrerequisiteError> {
            Ok(self.versions.get(path).cloned())
        }

        fn component_satisfied(
            &self,
            _component: &CatalogComponent,
            _architecture: PeArchitecture,
            _imports: &[String],
        ) -> Result<bool, PrerequisiteError> {
            Ok(false)
        }
    }

    fn analysis_service(runtime: Arc<dyn RuntimeProbe>) -> PrerequisiteService {
        PrerequisiteService::new(
            Arc::new(RecordingDownloader::default()),
            Arc::new(AcceptingTrust),
            runtime,
            Arc::new(RecordingRunner::default()),
        )
    }

    #[tokio::test]
    async fn outdated_system_vc100_dll_does_not_satisfy_analysis() {
        let directory = tempdir().expect("temp directory should exist");
        let importer = directory.path().join("game.exe");
        let system_dll = directory.path().join("system/msvcr100.dll");
        fs::create_dir_all(system_dll.parent().unwrap()).expect("system fixture should exist");
        fs::write(
            &importer,
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("importer should be written");
        fs::write(&system_dll, fixture_pe(PeArchitecture::X86, &[]))
            .expect("system DLL should be written");
        let runtime = VersionedDependencyProbe {
            system_path: Some(system_dll.clone()),
            versions: HashMap::from([(system_dll, "10.0.30319.1".into())]),
        };

        let analysis = analysis_service(Arc::new(runtime))
            .analyze(
                directory.path(),
                &analysis_manifest(&"a".repeat(64), &["game.exe"]),
                &CancellationToken::new(),
            )
            .await
            .expect("analysis should succeed");

        assert_eq!(analysis.requirements.len(), 1);
        assert_eq!(analysis.requirements[0].component_id, "vc2010-sp1-x86");
    }

    #[tokio::test]
    async fn outdated_app_local_vc100_dll_does_not_satisfy_analysis() {
        let directory = tempdir().expect("temp directory should exist");
        let importer = directory.path().join("game.exe");
        let local_dll = directory.path().join("bin/msvcr100.dll");
        fs::create_dir_all(local_dll.parent().unwrap()).expect("bin fixture should exist");
        fs::write(
            &importer,
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("importer should be written");
        fs::write(&local_dll, fixture_pe(PeArchitecture::X86, &[]))
            .expect("local DLL should be written");
        let runtime = VersionedDependencyProbe {
            system_path: None,
            versions: HashMap::from([(local_dll, "10.0.30319.1".into())]),
        };

        let analysis = analysis_service(Arc::new(runtime))
            .analyze(
                directory.path(),
                &analysis_manifest(&"b".repeat(64), &["game.exe"]),
                &CancellationToken::new(),
            )
            .await
            .expect("analysis should succeed");

        assert_eq!(analysis.requirements.len(), 1);
        assert_eq!(analysis.requirements[0].component_id, "vc2010-sp1-x86");
    }

    #[tokio::test]
    async fn analysis_uses_a_later_current_app_local_vc100_candidate() {
        let directory = tempdir().expect("temp directory should exist");
        let importer = directory.path().join("game/game.exe");
        let adjacent_dll = directory.path().join("game/msvcr100.dll");
        let bin_dll = directory.path().join("bin/msvcr100.dll");
        fs::create_dir_all(importer.parent().unwrap()).expect("game directory should exist");
        fs::create_dir_all(bin_dll.parent().unwrap()).expect("bin directory should exist");
        fs::write(
            &importer,
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("importer should be written");
        fs::write(&adjacent_dll, fixture_pe(PeArchitecture::X86, &[]))
            .expect("adjacent DLL should be written");
        fs::write(&bin_dll, fixture_pe(PeArchitecture::X86, &[]))
            .expect("bin DLL should be written");
        let runtime = VersionedDependencyProbe {
            system_path: None,
            versions: HashMap::from([
                (adjacent_dll, "10.0.30319.1".into()),
                (bin_dll, "10.0.40219.325".into()),
            ]),
        };

        let analysis = analysis_service(Arc::new(runtime))
            .analyze(
                directory.path(),
                &analysis_manifest(&"f".repeat(64), &["game/game.exe"]),
                &CancellationToken::new(),
            )
            .await
            .expect("analysis should succeed");

        assert!(analysis.requirements.is_empty());
    }

    #[tokio::test]
    async fn x86_requirement_preserves_importer_architecture_in_cache() {
        let directory = tempdir().expect("temp directory should exist");
        fs::write(
            directory.path().join("game.exe"),
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("importer should be written");

        let analysis = analysis_service(Arc::new(VersionedDependencyProbe {
            system_path: None,
            versions: HashMap::new(),
        }))
        .analyze(
            directory.path(),
            &analysis_manifest(&"c".repeat(64), &["game.exe"]),
            &CancellationToken::new(),
        )
        .await
        .expect("x86 prerequisite should be supported");

        assert_eq!(analysis.requirements.len(), 1);
        assert_eq!(analysis.requirements[0].architecture, PeArchitecture::X86);
        let serialized = serde_json::to_value(&analysis).expect("cache should serialize");
        assert_eq!(serialized["requirements"][0]["architecture"], "x86");
    }

    #[tokio::test]
    async fn x64_importer_with_only_x86_catalog_entry_is_explicitly_unsupported() {
        let directory = tempdir().expect("temp directory should exist");
        fs::write(
            directory.path().join("game.exe"),
            fixture_pe(PeArchitecture::X64, &["msvcr100.dll"]),
        )
        .expect("importer should be written");

        let result = analysis_service(Arc::new(VersionedDependencyProbe {
            system_path: None,
            versions: HashMap::new(),
        }))
        .analyze(
            directory.path(),
            &analysis_manifest(&"d".repeat(64), &["game.exe"]),
            &CancellationToken::new(),
        )
        .await;

        assert!(matches!(result, Err(PrerequisiteError::Unsupported(_))));
    }

    #[tokio::test]
    async fn mixed_x86_and_x64_importers_are_explicitly_unsupported() {
        let directory = tempdir().expect("temp directory should exist");
        fs::write(
            directory.path().join("game-x86.exe"),
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("x86 importer should be written");
        fs::write(
            directory.path().join("game-x64.exe"),
            fixture_pe(PeArchitecture::X64, &["msvcr100.dll"]),
        )
        .expect("x64 importer should be written");

        let result = analysis_service(Arc::new(VersionedDependencyProbe {
            system_path: None,
            versions: HashMap::new(),
        }))
        .analyze(
            directory.path(),
            &analysis_manifest(&"e".repeat(64), &["game-x86.exe", "game-x64.exe"]),
            &CancellationToken::new(),
        )
        .await;

        assert!(matches!(result, Err(PrerequisiteError::Unsupported(_))));
    }

    #[test]
    fn remote_source_rejects_url_changes_redirects_and_wrong_lengths() {
        let component = catalog_component("vc2010-sp1-x86").expect("catalog entry should exist");
        validate_remote_source(&component, component.remote_url().unwrap())
            .expect("pinned URL should pass");
        assert!(validate_remote_source(
            &component,
            "http://download.microsoft.com/vcredist_x86.exe"
        )
        .is_err());
        assert!(validate_remote_response(
            component.remote_url().unwrap(),
            "https://aka.ms/vcredist_x86.exe",
            302,
            Some(component.size),
            component.size,
        )
        .is_err());
        assert!(validate_remote_response(
            component.remote_url().unwrap(),
            component.remote_url().unwrap(),
            200,
            Some(component.size - 1),
            component.size,
        )
        .is_err());
    }

    #[test]
    fn artifact_integrity_rejects_size_and_hash_changes() {
        let directory = tempdir().expect("temp directory should exist");
        let path = directory.path().join("installer.exe");
        fs::write(&path, b"test").expect("fixture should be written");
        let expected = hex::encode(Sha256::digest(b"test"));
        validate_artifact_integrity(&path, 4, &expected).expect("exact artifact should pass");
        assert!(validate_artifact_integrity(&path, 5, &expected).is_err());
        assert!(validate_artifact_integrity(&path, 4, &"0".repeat(64)).is_err());
    }

    #[test]
    fn installer_exit_codes_have_distinct_outcomes() {
        assert_eq!(classify_installer_exit(0).unwrap(), InstallerExit::Success);
        assert_eq!(
            classify_installer_exit(3010).unwrap(),
            InstallerExit::RestartRecommended
        );
        assert_eq!(
            classify_installer_exit(1641).unwrap(),
            InstallerExit::RestartInitiated
        );
        assert!(classify_installer_exit(1603).is_err());
    }

    #[test]
    fn public_result_serializes_3010_and_1641_as_distinct_restart_statuses() {
        let result_with = |restart_status| EnsurePrerequisitesResult {
            ready: false,
            installed: vec!["vc2010-sp1-x86".into()],
            already_present: Vec::new(),
            restart_status,
        };

        let recommended = serde_json::to_value(result_with(RestartStatus::Recommended))
            .expect("3010 result should serialize");
        let initiated = serde_json::to_value(result_with(RestartStatus::Initiated))
            .expect("1641 result should serialize");

        assert_eq!(recommended["restartStatus"], "recommended");
        assert_eq!(initiated["restartStatus"], "initiated");
        assert_ne!(recommended, initiated);
    }

    struct RejectingTrust;

    impl TrustVerifier for RejectingTrust {
        fn verify_microsoft(&self, _path: &Path) -> Result<(), PrerequisiteError> {
            Err(PrerequisiteError::Verification(
                "signer is not Microsoft".into(),
            ))
        }
    }

    #[test]
    fn execution_verification_rejects_an_untrusted_signer() {
        let directory = tempdir().expect("temp directory should exist");
        let path = directory.path().join("installer.exe");
        let bytes = fixture_pe(PeArchitecture::X86, &[]);
        fs::write(&path, &bytes).expect("fixture should be written");
        let hash = hex::encode(Sha256::digest(&bytes));

        let error = verify_execution_artifact(
            &path,
            bytes.len() as u64,
            &hash,
            PeArchitecture::X86,
            &RejectingTrust,
        )
        .expect_err("untrusted signer must be rejected");
        assert!(matches!(error, PrerequisiteError::Verification(_)));
    }

    #[test]
    fn microsoft_signer_check_requires_the_exact_certificate_common_name() {
        assert!(is_microsoft_signer(
            "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond"
        ));
        assert!(!is_microsoft_signer(
            "CN=Not Microsoft Corporation, O=Example"
        ));
        assert!(!is_microsoft_signer("CN=Contoso, OU=Microsoft Partner"));
    }

    #[derive(Default)]
    struct RecordingDownloader {
        calls: AtomicUsize,
    }

    impl InstallerDownloader for RecordingDownloader {
        fn download<'a>(
            &'a self,
            _component: &'a CatalogComponent,
            _target: &'a Path,
            _cancel: &'a CancellationToken,
        ) -> ServiceFuture<'a, ()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct AcceptingTrust;

    impl TrustVerifier for AcceptingTrust {
        fn verify_microsoft(&self, _path: &Path) -> Result<(), PrerequisiteError> {
            Ok(())
        }
    }

    struct FixedProbe {
        satisfied: bool,
    }

    impl RuntimeProbe for FixedProbe {
        fn system_dependency_path(
            &self,
            _import: &str,
            _architecture: PeArchitecture,
        ) -> Result<Option<PathBuf>, PrerequisiteError> {
            Ok(None)
        }

        fn dependency_file_version(
            &self,
            _path: &Path,
        ) -> Result<Option<String>, PrerequisiteError> {
            Ok(None)
        }

        fn component_satisfied(
            &self,
            _component: &CatalogComponent,
            _architecture: PeArchitecture,
            _imports: &[String],
        ) -> Result<bool, PrerequisiteError> {
            Ok(self.satisfied)
        }
    }

    #[derive(Default)]
    struct RecordingRunner {
        calls: AtomicUsize,
    }

    impl InstallerRunner for RecordingRunner {
        fn run<'a>(
            &'a self,
            _program: &'a Path,
            _arguments: &'a [String],
            _cancel: &'a CancellationToken,
        ) -> ServiceFuture<'a, i32> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(0) })
        }
    }

    #[tokio::test]
    async fn cancellation_stops_before_download_or_execution() {
        let downloader = Arc::new(RecordingDownloader::default());
        let runner = Arc::new(RecordingRunner::default());
        let service = PrerequisiteService::new(
            downloader.clone(),
            Arc::new(AcceptingTrust),
            Arc::new(FixedProbe { satisfied: false }),
            runner.clone(),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = service
            .install_component(
                Path::new("unused"),
                None,
                &catalog_component("vc2010-sp1-x86").unwrap(),
                &["msvcr100.dll".into()],
                &cancel,
            )
            .await;

        assert!(matches!(result, Err(PrerequisiteError::Canceled)));
        assert_eq!(downloader.calls.load(Ordering::SeqCst), 0);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn successful_installer_must_pass_the_runtime_post_check() {
        let service = PrerequisiteService::new(
            Arc::new(RecordingDownloader::default()),
            Arc::new(AcceptingTrust),
            Arc::new(FixedProbe { satisfied: false }),
            Arc::new(RecordingRunner::default()),
        );

        let result = service
            .complete_install(
                &catalog_component("vc2010-sp1-x86").unwrap(),
                PeArchitecture::X86,
                &["msvcr100.dll".into()],
                InstallerExit::Success,
            )
            .await;

        assert!(matches!(result, Err(PrerequisiteError::Install(_))));
    }

    #[tokio::test]
    async fn complete_install_preserves_distinct_3010_and_1641_statuses() {
        let service = PrerequisiteService::new(
            Arc::new(RecordingDownloader::default()),
            Arc::new(AcceptingTrust),
            Arc::new(FixedProbe { satisfied: false }),
            Arc::new(RecordingRunner::default()),
        );
        let component = catalog_component("vc2010-sp1-x86").unwrap();
        let imports = ["msvcr100.dll".into()];

        let recommended = service
            .complete_install(
                &component,
                PeArchitecture::X86,
                &imports,
                InstallerExit::RestartRecommended,
            )
            .await
            .expect("3010 should remain observable while post-check is pending");
        let initiated = service
            .complete_install(
                &component,
                PeArchitecture::X86,
                &imports,
                InstallerExit::RestartInitiated,
            )
            .await
            .expect("1641 should remain observable while post-check is pending");

        assert!(!recommended.ready);
        assert_eq!(recommended.restart_status, RestartStatus::Recommended);
        assert!(!initiated.ready);
        assert_eq!(initiated.restart_status, RestartStatus::Initiated);
    }

    #[test]
    fn catalog_contains_the_pinned_vc_and_directx_artifacts() {
        let vc = catalog_component("vc2010-sp1-x86").unwrap();
        assert_eq!(vc.size, 8_993_744);
        assert_eq!(
            vc.sha256,
            "99dce3c841cc6028560830f7866c9ce2928c98cf3256892ef8e6cf755147b0d8"
        );
        assert_eq!(vc.required_version.as_deref(), Some("10.0.40219.325"));

        let directx = catalog_component("directx-june-2010").unwrap();
        assert_eq!(directx.size, 100_271_992);
        assert_eq!(
            directx.sha256,
            "8746ee1a84a083a90e37899d71d50d5c7c015e69688a466aa80447f011780c0d"
        );
        assert_eq!(
            directx.local_source().unwrap(),
            PathBuf::from("directx_installer/directx_jun2010_redist.exe")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_after_system_installer_spawn_does_not_kill_it() {
        let cancel = CancellationToken::new();
        let cancel_after_spawn = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_after_spawn.cancel();
        });
        let shell = PathBuf::from(std::env::var("COMSPEC").expect("Windows command shell"));
        let arguments = vec![
            "/C".to_string(),
            "ping 127.0.0.1 -n 2 > nul & exit /b 0".to_string(),
        ];

        let exit = ProcessInstallerRunner
            .run(&shell, &arguments, &cancel)
            .await
            .expect("installer owns its lifecycle after spawn");

        assert_eq!(exit, 0);
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn prerequisite_status_flow_reports_detection_verification_and_completion() {
        let directory = tempdir().expect("temporary game directory");
        fs::write(
            directory.path().join("game.exe"),
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("PE fixture should be written");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = events.clone();
        let service = PrerequisiteService::new(
            Arc::new(RecordingDownloader::default()),
            Arc::new(AcceptingTrust),
            Arc::new(FixedProbe { satisfied: true }),
            Arc::new(RecordingRunner::default()),
        )
        .with_progress(Arc::new(move |event| {
            recorded.lock().expect("progress recording lock").push((
                event.stage,
                event.component_id,
                event.progress,
            ));
        }));

        let result = service
            .ensure_manifest(
                directory.path(),
                &analysis_manifest(&"a".repeat(64), &["game.exe"]),
                &CancellationToken::new(),
            )
            .await
            .expect("already installed runtime should be ready");

        assert!(result.ready);
        assert_eq!(result.already_present, vec!["vc2010-sp1-x86"]);
        assert_eq!(
            *events.lock().expect("progress recording lock"),
            vec![
                ("detecting", None, Some(0.0)),
                ("detecting", None, Some(100.0)),
                ("verifying", Some("vc2010-sp1-x86".into()), Some(0.0)),
                ("verifying", Some("vc2010-sp1-x86".into()), Some(100.0)),
                ("complete", None, Some(100.0)),
            ]
        );
    }

    #[tokio::test]
    async fn fast_check_never_downloads_or_runs_a_missing_prerequisite() {
        let directory = tempdir().expect("temporary game directory");
        fs::write(
            directory.path().join("game.exe"),
            fixture_pe(PeArchitecture::X86, &["msvcr100.dll"]),
        )
        .expect("PE fixture should be written");
        let downloader = Arc::new(RecordingDownloader::default());
        let runner = Arc::new(RecordingRunner::default());
        let service = PrerequisiteService::new(
            downloader.clone(),
            Arc::new(AcceptingTrust),
            Arc::new(FixedProbe { satisfied: false }),
            runner.clone(),
        );

        let result = service
            .check_manifest(
                directory.path(),
                &analysis_manifest(&"b".repeat(64), &["game.exe"]),
                &CancellationToken::new(),
            )
            .await
            .expect("fast check should report missing state without installing");

        assert!(!result.ready);
        assert!(result.installed.is_empty());
        assert_eq!(downloader.calls.load(Ordering::SeqCst), 0);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    }
}
