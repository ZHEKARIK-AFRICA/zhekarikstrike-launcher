use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackSource {
    GoogleDrive,
}

#[derive(Debug, Clone)]
pub struct AttemptProgress {
    #[allow(dead_code)] // Kept explicit so future source controllers cannot mix measurements.
    pub source: PackSource,
    pub pack_sha256: String,
    pub replica_index: usize,
    pub current_offset: u64,
    pub useful_bytes: u64,
    pub header_latency: Option<Duration>,
    pub last_progress_at: Instant,
}

#[derive(Debug, Clone, Default)]
pub struct PressureWindow {
    pub throttled: bool,
    pub timeout_or_server_errors: usize,
}

#[derive(Debug, Clone)]
pub struct ControllerSample {
    #[cfg_attr(not(test), allow(dead_code))]
    // Historical baseline telemetry, test-only controller.
    pub useful_bytes: u64,
    /// Verified compressed bytes not yet consumed by a materializer; never the
    /// amount of content still waiting to be downloaded.
    #[cfg_attr(not(test), allow(dead_code))]
    pub ready_backlog_bytes: u64,
    pub pressure: PressureWindow,
    pub active_attempts: Vec<AttemptProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptivePreemption {
    pub pack_sha256: String,
    pub replica_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerReason {
    InsufficientMeasurements,
    ReadyBacklog,
    TrialIncrease,
    TrialAccepted,
    TrialRejected,
    Pressure,
    Cooldown,
    AtLimit,
}

impl ControllerReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InsufficientMeasurements => "insufficient_measurements",
            Self::ReadyBacklog => "ready_backlog",
            Self::TrialIncrease => "trial_increase",
            Self::TrialAccepted => "trial_accepted",
            Self::TrialRejected => "trial_rejected",
            Self::Pressure => "pressure",
            Self::Cooldown => "cooldown",
            Self::AtLimit => "at_limit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerDecision {
    pub target: usize,
    pub changed: bool,
    pub preempt: Option<AdaptivePreemption>,
    pub reason: ControllerReason,
}
