#[cfg(test)]
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

#[cfg(test)]
const MIN_JUDGMENT_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(test)]
const MAX_TRIAL_BACKLOG: u64 = 256 * 1024 * 1024;
#[cfg(test)]
const OUTLIER_PEER_RATE: f64 = 512.0 * 1024.0;
#[cfg(test)]
const COOLDOWN: Duration = Duration::from_secs(20);
#[cfg(test)]
const PRESSURE_WINDOW: Duration = Duration::from_secs(30);

pub use content_pack_core::controller::{
    AdaptivePreemption, AttemptProgress, ControllerDecision, ControllerReason, ControllerSample,
    PackSource, PressureWindow,
};

#[derive(Debug, Clone)]
#[cfg(test)]
pub struct ControllerTrial {
    previous_target: usize,
    baseline: f64,
}

#[derive(Debug, Clone)]
#[cfg(test)]
struct PressureEvent {
    at: Instant,
}

#[derive(Debug, Clone)]
#[cfg(test)]
pub struct AdaptivePackController {
    target: usize,
    maximum: usize,
    ewma_bytes_per_second: Option<f64>,
    accepted_baselines: BTreeMap<usize, f64>,
    trial: Option<ControllerTrial>,
    cooldown_until: Option<Instant>,
    pressure_events: VecDeque<PressureEvent>,
    sample_ticks: u8,
    sample_useful_bytes: u64,
    window_started_at: Option<Instant>,
}

#[cfg(test)]
impl AdaptivePackController {
    pub fn new(maximum: usize) -> Self {
        let maximum = maximum.clamp(2, 6);
        Self {
            target: 2.min(maximum),
            maximum,
            ewma_bytes_per_second: None,
            accepted_baselines: BTreeMap::new(),
            trial: None,
            cooldown_until: None,
            pressure_events: VecDeque::new(),
            sample_ticks: 0,
            sample_useful_bytes: 0,
            window_started_at: None,
        }
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn observe(&mut self, now: Instant, sample: ControllerSample) -> ControllerDecision {
        let previous_target = self.target;
        let mut reason = ControllerReason::InsufficientMeasurements;
        if self.window_started_at.is_none() {
            self.window_started_at = Some(now);
            self.sample_useful_bytes = sample.useful_bytes;
        }
        if sample.pressure.throttled {
            self.pressure_events.push_back(PressureEvent { at: now });
            self.pressure_events.push_back(PressureEvent { at: now });
            self.pressure_events.push_back(PressureEvent { at: now });
        } else {
            for _ in 0..sample.pressure.timeout_or_server_errors {
                self.pressure_events.push_back(PressureEvent { at: now });
            }
        }
        while self
            .pressure_events
            .front()
            .is_some_and(|event| now.duration_since(event.at) > PRESSURE_WINDOW)
        {
            self.pressure_events.pop_front();
        }
        if self.pressure_events.len() >= 3 {
            reason = ControllerReason::Pressure;
            self.target = (self.target / 2).max(1);
            self.trial = None;
            self.cooldown_until = Some(now + COOLDOWN);
            self.pressure_events.clear();
            self.reset_window(now, sample.useful_bytes);
        } else {
            self.sample_ticks = self.sample_ticks.saturating_add(1);
            let useful_delta = sample.useful_bytes.saturating_sub(self.sample_useful_bytes);
            if self.sample_ticks >= 3 && useful_delta >= MIN_JUDGMENT_BYTES {
                let elapsed = now
                    .duration_since(self.window_started_at.unwrap_or(now))
                    .as_secs_f64()
                    .max(0.001);
                let speed = useful_delta as f64 / elapsed;
                let ewma = self
                    .ewma_bytes_per_second
                    .map_or(speed, |previous| previous * 0.70 + speed * 0.30);
                self.ewma_bytes_per_second = Some(ewma);
                if let Some(trial) = self.trial.take() {
                    if ewma >= trial.baseline * 1.05 {
                        reason = ControllerReason::TrialAccepted;
                        self.accepted_baselines.insert(self.target, ewma);
                    } else {
                        reason = ControllerReason::TrialRejected;
                        self.target = trial.previous_target;
                        self.cooldown_until = Some(now + COOLDOWN);
                    }
                } else {
                    self.accepted_baselines.insert(self.target, ewma);
                    let cooldown_complete =
                        self.cooldown_until.is_none_or(|cooldown| now >= cooldown);
                    if self.target >= self.maximum {
                        reason = ControllerReason::AtLimit;
                    } else if sample.ready_backlog_bytes >= MAX_TRIAL_BACKLOG {
                        reason = ControllerReason::ReadyBacklog;
                    } else if !cooldown_complete {
                        reason = ControllerReason::Cooldown;
                    } else {
                        reason = ControllerReason::TrialIncrease;
                        self.trial = Some(ControllerTrial {
                            previous_target: self.target,
                            baseline: ewma,
                        });
                        self.target += 1;
                    }
                }
                self.reset_window(now, sample.useful_bytes);
            }
        }
        let peer_advancing = self.ewma_bytes_per_second.unwrap_or_default() >= OUTLIER_PEER_RATE;
        let preempt = peer_advancing
            .then(|| {
                sample
                    .active_attempts
                    .iter()
                    .filter(|attempt| {
                        now.duration_since(attempt.last_progress_at) >= Duration::from_secs(20)
                    })
                    .min_by_key(|attempt| attempt.last_progress_at)
                    .map(|attempt| AdaptivePreemption {
                        pack_sha256: attempt.pack_sha256.clone(),
                        replica_index: attempt.replica_index,
                    })
            })
            .flatten();

        ControllerDecision {
            target: self.target,
            changed: self.target != previous_target,
            preempt,
            reason,
        }
    }

    fn reset_window(&mut self, now: Instant, useful_bytes: u64) {
        self.sample_ticks = 0;
        self.sample_useful_bytes = useful_bytes;
        self.window_started_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(useful_bytes: u64, ready_backlog_bytes: u64) -> ControllerSample {
        ControllerSample {
            useful_bytes,
            ready_backlog_bytes,
            pressure: PressureWindow::default(),
            active_attempts: Vec::new(),
        }
    }

    #[test]
    fn drive_pack_reason_sequence_tracks_target_transitions() {
        let start = Instant::now();
        let mut controller = AdaptivePackController::new(4);
        assert_eq!(
            controller.observe(start, sample(0, 0)).reason,
            ControllerReason::InsufficientMeasurements
        );
        assert_eq!(
            controller
                .observe(start + Duration::from_secs(1), sample(32 * 1024 * 1024, 0))
                .reason,
            ControllerReason::InsufficientMeasurements
        );
        assert_eq!(
            controller
                .observe(start + Duration::from_secs(2), sample(64 * 1024 * 1024, 0))
                .reason,
            ControllerReason::TrialIncrease
        );
        assert_eq!(controller.target(), 3);
        assert_eq!(
            controller
                .observe(start + Duration::from_secs(3), sample(80 * 1024 * 1024, 0))
                .reason,
            ControllerReason::InsufficientMeasurements
        );
        assert_eq!(
            controller
                .observe(start + Duration::from_secs(4), sample(96 * 1024 * 1024, 0))
                .reason,
            ControllerReason::InsufficientMeasurements
        );
        let rejected =
            controller.observe(start + Duration::from_secs(5), sample(128 * 1024 * 1024, 0));
        assert_eq!(rejected.reason, ControllerReason::TrialRejected);
        assert_eq!(rejected.target, 2);
        assert!(rejected.changed);
    }

    #[test]
    fn drive_pack_pressure_reason_wins() {
        let mut controller = AdaptivePackController::new(4);
        let now = Instant::now();
        let mut pressured = sample(0, 0);
        pressured.pressure.throttled = true;
        let decision = controller.observe(now, pressured);
        assert_eq!(decision.reason, ControllerReason::Pressure);
        assert_eq!(decision.target, 1);
    }
}
