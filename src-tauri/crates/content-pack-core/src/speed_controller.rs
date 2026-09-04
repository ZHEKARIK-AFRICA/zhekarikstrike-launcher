//! Trial thresholds compare raw unique-byte window rates, never smoothed rates.
use crate::controller::{
    AdaptivePreemption, AttemptProgress, ControllerDecision, ControllerReason, PressureWindow,
};
use std::time::{Duration, Instant};
const MIB: u64 = 1024 * 1024;
const WINDOW: Duration = Duration::from_secs(10);
const COOLDOWN: Duration = Duration::from_secs(20);
pub struct SpeedSample {
    pub unique_bytes: u64,
    pub active_jobs: usize,
    pub pending_jobs: usize,
    pub allow_increase: bool,
    pub reduce_for_backlog: bool,
    pub integrity_epoch: u64,
    pub pressure: PressureWindow,
    pub active_attempts: Vec<AttemptProgress>,
}
#[derive(Clone, Copy)]
struct Window {
    at: Instant,
    bytes: u64,
}
impl Window {
    fn enough(self, now: Instant, bytes: u64) -> bool {
        now.saturating_duration_since(self.at) >= WINDOW
            && bytes.saturating_sub(self.bytes) >= 64 * MIB
    }
    fn rate(self, now: Instant, bytes: u64) -> f64 {
        bytes.saturating_sub(self.bytes) as f64
            / now
                .saturating_duration_since(self.at)
                .as_secs_f64()
                .max(0.001)
    }
}
struct Trial {
    previous: usize,
    baseline: f64,
    actual_started: Option<Instant>,
    measurement: Option<Window>,
    extension: Option<Window>,
}
pub struct OptimizedPackController {
    target: usize,
    maximum: usize,
    epoch: Option<u64>,
    window: Option<Window>,
    trial: Option<Trial>,
    cooldown: Option<Instant>,
    pressure: Vec<Instant>,
    ewma: Option<f64>,
}
impl OptimizedPackController {
    pub fn new(maximum: usize) -> Self {
        Self {
            target: 2,
            maximum: maximum.clamp(2, 6),
            epoch: None,
            window: None,
            trial: None,
            cooldown: None,
            pressure: Vec::new(),
            ewma: None,
        }
    }
    pub fn target(&self) -> usize {
        self.target
    }
    fn reset(&mut self, now: Instant, bytes: u64) {
        self.window = Some(Window { at: now, bytes });
        self.trial = None;
    }
    pub fn observe(&mut self, now: Instant, s: SpeedSample) -> ControllerDecision {
        let previous = self.target;
        if self.epoch != Some(s.integrity_epoch) {
            let changed = self.epoch.is_some();
            self.epoch = Some(s.integrity_epoch);
            if let Some(trial) = &self.trial {
                self.target = trial.previous;
            }
            self.reset(now, s.unique_bytes);
            if changed {
                self.cooldown = Some(now + COOLDOWN);
            }
        }
        let count = if s.pressure.throttled {
            3
        } else {
            s.pressure.timeout_or_server_errors.min(3)
        };
        self.pressure.extend(std::iter::repeat_n(now, count));
        self.pressure
            .retain(|at| now.saturating_duration_since(*at) <= Duration::from_secs(30));
        let reason = if self.pressure.len() >= 3 {
            self.target = (self.target / 2).max(1);
            self.reset(now, s.unique_bytes);
            self.cooldown = Some(now + COOLDOWN);
            self.pressure.clear();
            ControllerReason::Pressure
        } else if s.reduce_for_backlog {
            self.target = self.target.saturating_sub(1).max(1);
            self.reset(now, s.unique_bytes);
            self.cooldown = Some(now + COOLDOWN);
            ControllerReason::ReadyBacklog
        } else if s.active_jobs + s.pending_jobs < self.target {
            self.reset(now, s.unique_bytes);
            ControllerReason::InsufficientMeasurements
        } else if self.trial.is_some() {
            self.observe_trial(now, &s)
        } else if !s.allow_increase {
            self.window = Some(Window {
                at: now,
                bytes: s.unique_bytes,
            });
            ControllerReason::ReadyBacklog
        } else if self.cooldown.is_some_and(|until| now < until) {
            ControllerReason::Cooldown
        } else if self.target >= self.maximum {
            ControllerReason::AtLimit
        } else {
            let window = *self.window.get_or_insert(Window {
                at: now,
                bytes: s.unique_bytes,
            });
            if window.enough(now, s.unique_bytes)
                && s.active_jobs >= self.target
                && s.pending_jobs > 0
            {
                let baseline = window.rate(now, s.unique_bytes);
                self.smooth(baseline);
                self.trial = Some(Trial {
                    previous: self.target,
                    baseline,
                    actual_started: None,
                    measurement: None,
                    extension: None,
                });
                self.target += 1;
                self.window = None;
                ControllerReason::TrialIncrease
            } else {
                ControllerReason::InsufficientMeasurements
            }
        };
        let preempt = if self.ewma.unwrap_or(0.) >= 512. * 1024. {
            s.active_attempts
                .iter()
                .filter(|a| {
                    now.saturating_duration_since(a.last_progress_at) >= Duration::from_secs(20)
                })
                .min_by_key(|a| a.last_progress_at)
                .map(|a| AdaptivePreemption {
                    pack_sha256: a.pack_sha256.clone(),
                    replica_index: a.replica_index,
                })
        } else {
            None
        };
        ControllerDecision {
            target: self.target,
            changed: self.target != previous,
            preempt,
            reason,
        }
    }
    fn smooth(&mut self, rate: f64) {
        self.ewma = Some(self.ewma.map_or(rate, |old| old * 0.7 + rate * 0.3));
    }
    fn observe_trial(&mut self, now: Instant, s: &SpeedSample) -> ControllerReason {
        let trial = self.trial.as_mut().expect("trial exists");
        if trial.actual_started.is_none() && s.active_jobs < self.target {
            return ControllerReason::InsufficientMeasurements;
        }
        // Once the extra worker actually ran, ordinary task turnover must not
        // restart the measurement. A real exhausted tail is handled by observe.
        let started = *trial.actual_started.get_or_insert(now);
        if now.saturating_duration_since(started) < Duration::from_secs(2) {
            return ControllerReason::InsufficientMeasurements;
        }
        let measurement = *trial.measurement.get_or_insert(Window {
            at: now,
            bytes: s.unique_bytes,
        });
        if !trial
            .extension
            .unwrap_or(measurement)
            .enough(now, s.unique_bytes)
        {
            return ControllerReason::InsufficientMeasurements;
        }
        let rate = measurement.rate(now, s.unique_bytes);
        let accepted = rate >= trial.baseline * 1.05;
        let rejected = rate < trial.baseline * 0.90 || trial.extension.is_some();
        if accepted || rejected {
            let old = trial.previous;
            self.smooth(rate);
            self.reset(now, s.unique_bytes);
            if accepted {
                ControllerReason::TrialAccepted
            } else {
                self.target = old;
                self.cooldown = Some(now + COOLDOWN);
                ControllerReason::TrialRejected
            }
        } else {
            trial.extension = Some(Window {
                at: now,
                bytes: s.unique_bytes,
            });
            ControllerReason::InsufficientMeasurements
        }
    }
}
pub struct QueueDecision {
    pub allow_increase: bool,
    pub reduce: bool,
    pub limit_bytes: u64,
    pub seconds: Option<f64>,
}
#[derive(Default)]
pub struct WorkQueueController {
    sample: (u64, f64),
    rate: Option<f64>,
    full_since: Option<Instant>,
    latched: bool,
}
impl WorkQueueController {
    pub fn new() -> Self {
        Self {
            sample: (0, 0.),
            rate: None,
            full_since: None,
            latched: false,
        }
    }
    pub fn observe(
        &mut self,
        now: Instant,
        raw: u64,
        compressed: u64,
        total: u64,
        work: f64,
        memory: u64,
    ) -> QueueDecision {
        let bytes = total.saturating_sub(self.sample.0);
        let busy = (work - self.sample.1).max(0.);
        if bytes >= 8 * MIB && busy >= 1. {
            let rate = bytes as f64 / busy;
            self.rate = Some(self.rate.map_or(rate, |old| old * 0.7 + rate * 0.3));
            self.sample = (total, work);
        }
        let limit = if let Some(rate) = self.rate {
            ((rate * 8.) as u64)
                .clamp(64 * MIB, 512 * MIB)
                .min(memory / 4)
        } else {
            (256 * MIB).min(memory / 4)
        };
        let ready = if self.rate.is_some() { raw } else { compressed };
        let mut reduce = false;
        if ready >= limit {
            self.latched = true;
            let since = self.full_since.get_or_insert(now);
            if now.saturating_duration_since(*since) >= WINDOW {
                reduce = true;
                *since = now;
            }
        } else {
            self.full_since = None;
            if ready < limit / 2 {
                self.latched = false;
            }
        }
        QueueDecision {
            allow_increase: memory >= 512 * MIB && !self.latched,
            reduce,
            limit_bytes: limit,
            seconds: self.rate.map(|rate| raw as f64 / rate),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn s(bytes: u64, jobs: usize) -> SpeedSample {
        SpeedSample {
            unique_bytes: bytes * MIB,
            active_jobs: jobs,
            pending_jobs: 8,
            allow_increase: true,
            reduce_for_backlog: false,
            integrity_epoch: 0,
            pressure: PressureWindow::default(),
            active_attempts: vec![],
        }
    }
    #[test]
    fn drive_pack_real_scheduler_window_survives_refill_and_uses_combined_mean() {
        let t = Instant::now();
        let mut c = OptimizedPackController::new(6);
        assert_eq!(c.observe(t, s(0, 2)).target, 2);
        assert_eq!(
            c.observe(t + Duration::from_secs(10), s(100, 2)).reason,
            ControllerReason::TrialIncrease
        );
        c.observe(t + Duration::from_secs(14), s(140, 2));
        c.observe(t + Duration::from_secs(15), s(150, 3));
        c.observe(t + Duration::from_secs(17), s(170, 3));
        c.observe(t + Duration::from_secs(20), s(200, 2)); // brief refill with pending work
        assert_eq!(
            c.observe(t + Duration::from_secs(27), s(270, 3)).reason,
            ControllerReason::InsufficientMeasurements
        );
        assert_eq!(
            c.observe(t + Duration::from_secs(37), s(382, 3)).reason,
            ControllerReason::TrialAccepted
        );
        assert_eq!(c.target(), 3);
    }
    #[test]
    fn drive_pack_trial_rejects_loss_but_not_tail_and_integrity_cannot_accept() {
        for mode in 0..3 {
            let t = Instant::now();
            let mut c = OptimizedPackController::new(6);
            c.observe(t, s(0, 2));
            c.observe(t + Duration::from_secs(10), s(100, 2));
            c.observe(t + Duration::from_secs(11), s(110, 3));
            c.observe(t + Duration::from_secs(13), s(130, 3));
            let mut sample = s(210, 3);
            if mode == 1 {
                sample.active_jobs = 1;
                sample.pending_jobs = 0;
            }
            if mode == 2 {
                sample.integrity_epoch = 1;
                sample.unique_bytes = 300 * MIB;
            }
            let result = c.observe(t + Duration::from_secs(23), sample);
            assert_ne!(result.reason, ControllerReason::TrialAccepted);
            if mode == 0 {
                assert_eq!(result.reason, ControllerReason::TrialRejected);
                assert_eq!(result.target, 2);
            }
            if mode == 1 {
                assert_eq!(result.reason, ControllerReason::InsufficientMeasurements);
            }
        }
    }
    #[test]
    fn drive_pack_work_queue_latches_and_ignores_network_starvation() {
        let t = Instant::now();
        let mut cold = WorkQueueController::new();
        let constrained = cold.observe(t, 0, 200 * MIB, 0, 0., 512 * MIB);
        assert_eq!(constrained.limit_bytes, 128 * MIB);
        assert!(!constrained.allow_increase);
        let scarce = cold.observe(t, 0, 0, 0, 0., 256 * MIB);
        assert_eq!(scarce.limit_bytes, 64 * MIB);
        assert!(!scarce.allow_increase);
        let mut q = WorkQueueController::new();
        assert!(!q.observe(t, 0, 256 * MIB, 0, 0., 4096 * MIB).allow_increase);
        assert!(
            !q.observe(t + WINDOW, 0, 200 * MIB, 0, 0., 4096 * MIB)
                .allow_increase
        );
        assert!(
            q.observe(t + WINDOW, 0, 100 * MIB, 0, 0., 4096 * MIB)
                .allow_increase
        );
        let measured = q.observe(t + WINDOW, 100 * MIB, 0, 16 * MIB, 2., 4096 * MIB);
        assert_eq!(measured.limit_bytes, 64 * MIB);
        assert_eq!(measured.seconds, Some(12.5));
        let starved = q.observe(t + WINDOW + WINDOW, 100 * MIB, 0, 16 * MIB, 2., 4096 * MIB);
        assert!(starved.reduce);
        assert_eq!(starved.limit_bytes, 64 * MIB);
        assert!(
            !q.observe(t + WINDOW + WINDOW, 0, 0, 16 * MIB, 2., 256 * MIB)
                .allow_increase
        );
    }
}
