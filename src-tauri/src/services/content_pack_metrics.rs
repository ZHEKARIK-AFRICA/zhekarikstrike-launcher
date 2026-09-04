//! Operation-local diagnostics. No file paths, URLs, or credentials enter snapshots.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone)]
pub(crate) struct PackMetrics(Arc<Inner>);

struct Inner {
    started: Instant,
    operation: String,
    jobs: Counter,
    requests: Counter,
    materializers: Counter,
    network: AtomicU64,
    unique: AtomicU64,
    integrity_epoch: AtomicU64,
    work_us: AtomicU64,
    raw_ready: AtomicU64,
    verified_chunks: Mutex<std::collections::HashSet<String>>,
    verified: AtomicU64,
    materialized: AtomicU64,
    committed: AtomicU64,
    chunk_wait_us: AtomicU64,
    staging_wait_us: AtomicU64,
    state: Mutex<Snapshot>,
    #[cfg(test)]
    history: Mutex<Vec<Snapshot>>,
}

#[derive(Default)]
struct Counter {
    current: AtomicUsize,
    peak: AtomicUsize,
}

#[derive(Clone, Copy)]
pub(crate) enum Activity {
    Job,
    Request,
    Materializer,
}

pub(crate) struct ActivityGuard {
    metrics: PackMetrics,
    kind: Activity,
}

#[derive(Clone, Default, Serialize, Debug)]
pub(crate) struct Snapshot {
    pub operation: String,
    pub elapsed_sec: f64,
    pub pipeline_started_sec: Option<f64>,
    pub staging_available_bytes: u64,
    pub target_jobs: usize,
    pub active_jobs: usize,
    pub peak_jobs: usize,
    pub active_requests: usize,
    pub peak_requests: usize,
    pub pending_jobs: usize,
    pub ready_backlog_bytes: u64,
    pub received_bytes: u64,
    pub unique_bytes: u64,
    pub integrity_epoch: u64,
    pub active_work_sec: f64,
    pub raw_ready_bytes: u64,
    pub queue_limit_bytes: u64,
    pub queue_seconds: Option<f64>,
    pub first_materialized_sec: Option<f64>,
    pub verified_bytes: u64,
    pub materialized_bytes: u64,
    pub committed_bytes: u64,
    pub network_bytes_per_sec: f64,
    pub materialized_bytes_per_sec: f64,
    pub controller_reason: String,
    pub decision_counts: std::collections::BTreeMap<String, u64>,
    pub materializer_target: usize,
    pub materializer_maximum: usize,
    pub active_materializers: usize,
    pub cpu_percent: f32,
    pub available_memory: u64,
    pub chunk_wait_sec: f64,
    pub staging_wait_sec: f64,
    pub download_finished_sec: Option<f64>,
    pub materialization_finished_sec: Option<f64>,
    pub finished: Option<String>,
}

impl PackMetrics {
    pub fn new(operation: &str) -> Self {
        Self(Arc::new(Inner {
            started: Instant::now(),
            operation: operation.to_owned(),
            jobs: Counter::default(),
            requests: Counter::default(),
            materializers: Counter::default(),
            network: AtomicU64::new(0),
            unique: AtomicU64::new(0),
            integrity_epoch: AtomicU64::new(0),
            work_us: AtomicU64::new(0),
            raw_ready: AtomicU64::new(0),
            verified_chunks: Mutex::new(std::collections::HashSet::new()),
            verified: AtomicU64::new(0),
            materialized: AtomicU64::new(0),
            committed: AtomicU64::new(0),
            chunk_wait_us: AtomicU64::new(0),
            staging_wait_us: AtomicU64::new(0),
            state: Mutex::new(Snapshot::default()),
            #[cfg(test)]
            history: Mutex::new(Vec::new()),
        }))
    }
    fn counter(&self, kind: Activity) -> &Counter {
        match kind {
            Activity::Job => &self.0.jobs,
            Activity::Request => &self.0.requests,
            Activity::Materializer => &self.0.materializers,
        }
    }
    pub fn activity(&self, kind: Activity) -> ActivityGuard {
        let counter = self.counter(kind);
        let active = counter.current.fetch_add(1, Relaxed) + 1;
        counter.peak.fetch_max(active, Relaxed);
        ActivityGuard {
            metrics: self.clone(),
            kind,
        }
    }
    pub fn received(&self, bytes: u64) {
        self.0.network.fetch_add(bytes, Relaxed);
    }
    pub fn unique(&self, bytes: u64) {
        self.0.unique.fetch_add(bytes, Relaxed);
    }
    pub fn integrity_failed(&self) {
        self.0.integrity_epoch.fetch_add(1, Relaxed);
    }
    pub fn raw_ready(&self, bytes: u64) {
        self.0.raw_ready.fetch_add(bytes, Relaxed);
    }
    pub fn raw_consumed(&self, bytes: u64) {
        self.0.raw_ready.fetch_sub(bytes, Relaxed);
    }
    pub fn worked(&self, duration: Duration) {
        let workers = self.0.materializers.current.load(Relaxed).max(1) as u128;
        self.0.work_us.fetch_add(
            (duration.as_micros() / workers).min(u64::MAX as u128) as u64,
            Relaxed,
        );
    }
    pub fn queue_estimate(&self, limit: u64, seconds: Option<f64>) {
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        state.queue_limit_bytes = limit;
        state.queue_seconds = seconds;
    }
    pub fn verified(&self, bytes: u64) {
        self.0.verified.fetch_add(bytes, Relaxed);
    }
    pub fn verified_chunk(&self, sha: &str, bytes: u64) {
        if self
            .0
            .verified_chunks
            .lock()
            .expect("verified chunks")
            .insert(sha.to_owned())
        {
            self.verified(bytes);
        }
    }
    pub fn materialized(&self, bytes: u64) {
        self.0.materialized.fetch_add(bytes, Relaxed);
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        state
            .first_materialized_sec
            .get_or_insert_with(|| self.0.started.elapsed().as_secs_f64());
    }
    pub fn committed(&self, bytes: u64) {
        self.0.committed.fetch_add(bytes, Relaxed);
    }
    pub fn waited(&self, staging: bool, duration: Duration) {
        let counter = if staging {
            &self.0.staging_wait_us
        } else {
            &self.0.chunk_wait_us
        };
        counter.fetch_add(duration.as_micros().min(u64::MAX as u128) as u64, Relaxed);
    }
    pub fn materializer(&self, target: usize, maximum: usize, cpu: f32, memory: u64) {
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        let changed = state.materializer_target != 0 && state.materializer_target != target;
        state.materializer_target = target;
        state.materializer_maximum = maximum;
        state.cpu_percent = cpu;
        state.available_memory = memory;
        drop(state);
        if changed {
            self.log_sample(&mut Snapshot::default());
        }
    }
    pub fn controller(&self, target: usize, pending: usize, backlog: u64, reason: &str) {
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        state.target_jobs = target;
        state.pending_jobs = pending;
        state.ready_backlog_bytes = backlog;
        state.controller_reason = reason.to_owned();
        *state.decision_counts.entry(reason.to_owned()).or_default() += 1;
        drop(state);
        #[cfg(test)]
        self.0
            .history
            .lock()
            .expect("metrics history")
            .push(self.snapshot());
    }
    pub fn phase_finished(&self, download: bool) {
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        let at = Some(self.0.started.elapsed().as_secs_f64());
        if download {
            state.download_finished_sec = at;
        } else {
            state.materialization_finished_sec = at;
        }
    }
    pub fn pipeline_started(&self) {
        self.0
            .state
            .lock()
            .expect("pack metrics mutex")
            .pipeline_started_sec = Some(self.0.started.elapsed().as_secs_f64());
    }
    pub fn queues(&self, backlog: u64, staging_available: u64) {
        let mut state = self.0.state.lock().expect("pack metrics mutex");
        state.ready_backlog_bytes = backlog;
        state.staging_available_bytes = staging_available;
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.0.state.lock().expect("pack metrics mutex").clone();
        s.operation = self.0.operation.clone();
        s.elapsed_sec = self.0.started.elapsed().as_secs_f64();
        s.active_jobs = self.0.jobs.current.load(Relaxed);
        s.peak_jobs = self.0.jobs.peak.load(Relaxed);
        s.active_requests = self.0.requests.current.load(Relaxed);
        s.peak_requests = self.0.requests.peak.load(Relaxed);
        s.active_materializers = self.0.materializers.current.load(Relaxed);
        s.received_bytes = self.0.network.load(Relaxed);
        s.unique_bytes = self.0.unique.load(Relaxed);
        s.integrity_epoch = self.0.integrity_epoch.load(Relaxed);
        s.active_work_sec = self.0.work_us.load(Relaxed) as f64 / 1e6;
        s.raw_ready_bytes = self.0.raw_ready.load(Relaxed);
        s.verified_bytes = self.0.verified.load(Relaxed);
        s.materialized_bytes = self.0.materialized.load(Relaxed);
        s.committed_bytes = self.0.committed.load(Relaxed);
        s.chunk_wait_sec = self.0.chunk_wait_us.load(Relaxed) as f64 / 1e6;
        s.staging_wait_sec = self.0.staging_wait_us.load(Relaxed) as f64 / 1e6;
        s
    }
    pub fn log_sample(&self, previous: &mut Snapshot) {
        let mut s = self.snapshot();
        let elapsed = (s.elapsed_sec - previous.elapsed_sec).max(0.001);
        s.network_bytes_per_sec =
            s.received_bytes.saturating_sub(previous.received_bytes) as f64 / elapsed;
        s.materialized_bytes_per_sec =
            s.materialized_bytes
                .saturating_sub(previous.materialized_bytes) as f64
                / elapsed;
        if let Ok(json) = serde_json::to_string(&s) {
            crate::logger::info(&format!("pack-pipeline {json}"));
        }
        #[cfg(test)]
        self.0
            .history
            .lock()
            .expect("metrics history")
            .push(s.clone());
        *previous = s;
    }
    pub fn finish(&self, outcome: &str) {
        self.0.state.lock().expect("pack metrics mutex").finished = Some(outcome.to_owned());
        self.log_sample(&mut Snapshot::default());
    }
    #[cfg(test)]
    pub fn history(&self) -> Vec<Snapshot> {
        self.0.history.lock().unwrap().clone()
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.metrics
            .counter(self.kind)
            .current
            .fetch_sub(1, Relaxed);
    }
}

#[derive(Clone)]
pub(crate) struct PackRunOptions {
    pub metrics: PackMetrics,
    pub traffic: Arc<super::content_pack_stream::UniqueTraffic>,
    #[cfg(test)]
    pub profile: PackProfile,
    #[cfg(test)]
    pub fixed_jobs: Option<usize>,
    #[cfg(test)]
    pub local_transport: Option<reqwest::Url>,
    #[cfg(test)]
    pub tick_interval: Duration,
    #[cfg(test)]
    pub clock_ms: Option<Arc<AtomicU64>>,
    #[cfg(test)]
    pub budget: Option<Arc<ProbeBudget>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackProfile {
    Baseline,
    Optimized,
}

impl PackRunOptions {
    pub fn new(operation: &str) -> Self {
        Self {
            metrics: PackMetrics::new(operation),
            traffic: Arc::new(super::content_pack_stream::UniqueTraffic::default()),
            #[cfg(test)]
            profile: PackProfile::Optimized,
            #[cfg(test)]
            fixed_jobs: None,
            #[cfg(test)]
            local_transport: None,
            #[cfg(test)]
            tick_interval: Duration::from_secs(2),
            #[cfg(test)]
            clock_ms: None,
            #[cfg(test)]
            budget: None,
        }
    }
    pub fn optimized(&self) -> bool {
        #[cfg(test)]
        {
            self.profile == PackProfile::Optimized
        }
        #[cfg(not(test))]
        {
            true
        }
    }
    pub fn target(&self, adaptive: usize) -> usize {
        #[cfg(test)]
        if let Some(fixed) = self.fixed_jobs {
            return fixed;
        }
        adaptive
    }
    pub fn interval(&self) -> Duration {
        #[cfg(test)]
        {
            return self.tick_interval;
        }
        #[cfg(not(test))]
        {
            Duration::from_secs(2)
        }
    }
    pub fn request_url(&self, url: &reqwest::Url) -> reqwest::Url {
        #[cfg(test)]
        if let Some(local) = &self.local_transport {
            assert_eq!(local.host_str(), Some("127.0.0.1"));
            let mut mapped = local.clone();
            mapped.set_query(url.query());
            return mapped;
        }
        url.clone()
    }
    pub fn request(&self, bytes: u64) -> Result<RequestGuard, crate::error::AppError> {
        #[cfg(not(test))]
        let _ = bytes;
        #[cfg(test)]
        let permit = self.budget.as_ref().map(|b| b.reserve(bytes)).transpose()?;
        Ok(RequestGuard {
            metrics: self.metrics.clone(),
            activity: Some(self.metrics.activity(Activity::Request)),
            #[cfg(test)]
            permit,
        })
    }
}

pub(crate) struct RequestGuard {
    metrics: PackMetrics,
    activity: Option<ActivityGuard>,
    #[cfg(test)]
    permit: Option<BudgetPermit>,
}
impl RequestGuard {
    pub fn received(&mut self, bytes: u64) -> Result<(), crate::error::AppError> {
        self.metrics.received(bytes);
        #[cfg(test)]
        if let Some(permit) = &mut self.permit {
            permit.received(bytes)?;
        }
        Ok(())
    }
    pub fn response_finished(&mut self) {
        self.activity.take();
    }
}

#[cfg(test)]
pub(crate) struct ProbeBudget {
    remaining: AtomicU64,
    pub received: AtomicU64,
}
#[cfg(test)]
impl ProbeBudget {
    pub fn new(bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            remaining: AtomicU64::new(bytes),
            received: AtomicU64::new(0),
        })
    }
    fn reserve(self: &Arc<Self>, bytes: u64) -> Result<BudgetPermit, crate::error::AppError> {
        self.remaining
            .fetch_update(Relaxed, Relaxed, |n| n.checked_sub(bytes))
            .map_err(|_| {
                crate::error::AppError::InvalidData("probe network budget exhausted".into())
            })?;
        Ok(BudgetPermit {
            budget: self.clone(),
            remaining: bytes,
        })
    }
}
#[cfg(test)]
struct BudgetPermit {
    budget: Arc<ProbeBudget>,
    remaining: u64,
}
#[cfg(test)]
impl BudgetPermit {
    fn received(&mut self, bytes: u64) -> Result<(), crate::error::AppError> {
        self.budget.received.fetch_add(bytes, Relaxed);
        let remaining = self.remaining.checked_sub(bytes);
        self.remaining = remaining.unwrap_or(0);
        if remaining.is_none() {
            return Err(crate::error::AppError::InvalidData(
                "probe response exceeded budget reservation".into(),
            ));
        }
        Ok(())
    }
}
#[cfg(test)]
impl Drop for BudgetPermit {
    fn drop(&mut self) {
        self.budget.remaining.fetch_add(self.remaining, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_pack_budget_counts_failed_attempts_and_parallel_reservations() {
        let budget = ProbeBudget::new(100);
        let mut options = PackRunOptions::new("budget");
        options.budget = Some(budget.clone());
        let mut first = options.request(60).unwrap();
        let second = options.request(40).unwrap();
        assert!(options.request(1).is_err());
        first.received(30).unwrap();
        drop(first); // Interrupted response refunds only the unread reservation.
        drop(second);
        let mut retry = options.request(70).unwrap();
        retry.received(70).unwrap();
        drop(retry);
        assert_eq!(budget.received.load(Relaxed), 100);
        assert!(options.request(1).is_err());
        assert_eq!(options.metrics.snapshot().active_requests, 0);
        options.metrics.queues(10, 20);
        options.metrics.queues(0, 100);
        options.metrics.materializer(2, 6, 30.0, 1000);
        options.metrics.materializer(3, 6, 40.0, 900);
        let snapshot = options.metrics.snapshot();
        assert_eq!(snapshot.ready_backlog_bytes, 0);
        assert_eq!(snapshot.staging_available_bytes, 100);
        assert_eq!(
            options
                .metrics
                .history()
                .last()
                .unwrap()
                .materializer_target,
            3
        );
    }
}
