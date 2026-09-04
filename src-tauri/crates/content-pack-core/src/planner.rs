use crate::integrity::CoreError;
use std::collections::VecDeque;
pub const REQUEST_SIZE: u64 = 16 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}
impl ByteRange {
    pub fn is_empty(self) -> bool {
        self.start > self.end_inclusive
    }
    pub fn len(self) -> Result<u64, CoreError> {
        self.end_inclusive
            .checked_sub(self.start)
            .and_then(|n| n.checked_add(1))
            .ok_or(CoreError::Range)
    }
    pub fn contains(self, start: u64, size: u64) -> bool {
        size.checked_sub(1)
            .and_then(|n| start.checked_add(n))
            .is_some_and(|end| start >= self.start && end <= self.end_inclusive)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransferMode {
    Full,
    Ranges(Vec<ByteRange>),
}
#[derive(Clone, Copy, Debug)]
pub struct CostEstimate {
    pub bytes_per_second: f64,
    pub header_seconds: f64,
}
impl Default for CostEstimate {
    fn default() -> Self {
        Self {
            bytes_per_second: 8. * 1024. * 1024.,
            header_seconds: 0.25,
        }
    }
}
impl CostEstimate {
    pub fn seconds(self, bytes: u64, requests: usize) -> f64 {
        bytes as f64 / self.bytes_per_second + requests as f64 * self.header_seconds
    }
}
#[derive(Default)]
pub struct RequestCalibration {
    total_bytes: u64,
    total_requests: u64,
    recent: VecDeque<CostEstimate>,
}
impl RequestCalibration {
    /// Only fully successful responses, after their containing hashes pass.
    pub fn success(&mut self, bytes: u64, body_seconds: f64, header_seconds: f64) {
        if bytes == 0
            || !body_seconds.is_finite()
            || body_seconds <= 0.
            || !header_seconds.is_finite()
            || header_seconds < 0.
        {
            return;
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.total_requests += 1;
        self.recent.push_back(CostEstimate {
            bytes_per_second: bytes as f64 / body_seconds,
            header_seconds,
        });
        if self.recent.len() > 8 {
            self.recent.pop_front();
        }
    }
    pub fn estimate(&self) -> CostEstimate {
        if self.total_requests < 4 || self.total_bytes < 64 * 1024 * 1024 {
            return CostEstimate::default();
        }
        fn median(mut values: Vec<f64>) -> f64 {
            values.sort_by(f64::total_cmp);
            let n = values.len();
            if n.is_multiple_of(2) {
                (values[n / 2 - 1] + values[n / 2]) / 2.
            } else {
                values[n / 2]
            }
        }
        CostEstimate {
            bytes_per_second: median(self.recent.iter().map(|s| s.bytes_per_second).collect()),
            header_seconds: median(self.recent.iter().map(|s| s.header_seconds).collect()),
        }
    }
}
pub fn full_range(offset: u64, size: u64) -> Result<ByteRange, CoreError> {
    if offset >= size {
        return Err(CoreError::Range);
    }
    Ok(ByteRange {
        start: offset,
        end_inclusive: offset.saturating_add(REQUEST_SIZE - 1).min(size - 1),
    })
}
pub fn choose_plan(
    pack_size: u64,
    required: &[ByteRange],
    estimate: CostEstimate,
) -> Result<TransferMode, CoreError> {
    if pack_size == 0
        || !estimate.bytes_per_second.is_finite()
        || estimate.bytes_per_second <= 0.
        || !estimate.header_seconds.is_finite()
        || estimate.header_seconds < 0.
    {
        return Err(CoreError::Range);
    }
    let mut spans = required.to_vec();
    spans.sort_unstable_by_key(|s| s.start);
    spans.dedup();
    for (i, span) in spans.iter().enumerate() {
        if span.len()? > REQUEST_SIZE
            || span.end_inclusive >= pack_size
            || (i > 0 && spans[i - 1].end_inclusive >= span.start)
        {
            return Err(CoreError::Range);
        }
    }
    if spans.is_empty() {
        return Ok(TransferMode::Ranges(vec![]));
    }
    let n = spans.len();
    let mut cost = vec![f64::INFINITY; n + 1];
    let mut next = vec![n; n];
    cost[n] = 0.;
    for i in (0..n).rev() {
        for j in i..n {
            let span = ByteRange {
                start: spans[i].start,
                end_inclusive: spans[j].end_inclusive,
            };
            let len = span.len()?;
            if len > REQUEST_SIZE {
                break;
            }
            let candidate = estimate.seconds(len, 1) + cost[j + 1];
            if candidate < cost[i] {
                cost[i] = candidate;
                next[i] = j + 1;
            }
        }
    }
    let requests = pack_size.div_ceil(REQUEST_SIZE);
    if cost[0]
        > estimate.seconds(
            pack_size,
            usize::try_from(requests).map_err(|_| CoreError::Overflow)?,
        ) * 0.95
    {
        return Ok(TransferMode::Full);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let j = next[i];
        out.push(ByteRange {
            start: spans[i].start,
            end_inclusive: spans[j - 1].end_inclusive,
        });
        i = j;
    }
    Ok(TransferMode::Ranges(out))
}
pub fn validate_plan(
    mode: &TransferMode,
    pack_size: u64,
    required: &[ByteRange],
) -> Result<(), CoreError> {
    if pack_size == 0 {
        return Err(CoreError::Range);
    }
    for span in required {
        if span.len()? == 0 || span.end_inclusive >= pack_size {
            return Err(CoreError::Range);
        }
    }
    if let TransferMode::Ranges(ranges) = mode {
        for (i, r) in ranges.iter().enumerate() {
            if r.len()? > REQUEST_SIZE
                || r.end_inclusive >= pack_size
                || (i > 0 && ranges[i - 1].end_inclusive >= r.start)
            {
                return Err(CoreError::Range);
            }
        }
        if required.iter().any(|s| {
            !ranges
                .iter()
                .any(|r| r.contains(s.start, s.len().unwrap_or(0)))
        }) {
            return Err(CoreError::Integrity(
                "saved pack plan does not cover required chunks",
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_pack_cost_plans_sparse_dense_and_overflow() {
        let mib = 1024 * 1024;
        let sparse = vec![
            ByteRange {
                start: 0,
                end_inclusive: 2 * mib - 1,
            },
            ByteRange {
                start: 32 * mib,
                end_inclusive: 34 * mib - 1,
            },
        ];
        let chosen = choose_plan(64 * mib, &sparse, CostEstimate::default()).unwrap();
        assert!(matches!(chosen,TransferMode::Ranges(ref r) if r.len()==2));
        validate_plan(&chosen, 64 * mib, &sparse).unwrap();
        let dense = (0..8)
            .map(|i| ByteRange {
                start: i * 8 * mib,
                end_inclusive: (i + 1) * 8 * mib - 1,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            choose_plan(64 * mib, &dense, CostEstimate::default()).unwrap(),
            TransferMode::Full
        );
        assert!(ByteRange {
            start: 0,
            end_inclusive: u64::MAX
        }
        .len()
        .is_err());
        assert!(validate_plan(&TransferMode::Ranges(vec![]), 64 * mib, &sparse).is_err());
    }
    #[test]
    fn drive_pack_calibration_requires_successful_sample_volume() {
        let mut c = RequestCalibration::default();
        for _ in 0..3 {
            c.success(16 * 1024 * 1024, 1., 0.1);
        }
        assert_eq!(c.estimate().header_seconds, 0.25);
        c.success(16 * 1024 * 1024, 1., 0.1);
        assert_eq!(c.estimate().header_seconds, 0.1);
        c.success(1, f64::NAN, 0.1);
        assert_eq!(c.estimate().header_seconds, 0.1);
    }
}
