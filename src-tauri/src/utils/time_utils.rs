use std::time::Instant;

pub fn seconds_remaining(start: Instant, done: u64, total: Option<u64>) -> Option<f64> {
    let total = total?;
    if done == 0 || done >= total {
        return Some(0.0);
    }

    let elapsed = start.elapsed().as_secs_f64();
    let speed = done as f64 / elapsed.max(0.001);
    Some((total - done) as f64 / speed)
}
