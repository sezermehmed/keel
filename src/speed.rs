//! Per-peer speeds from two byte counters.
//!
//! The engine reports how many bytes a peer has moved, not how fast.

/// Bytes per second between two samples. `None` when the clock did not move.
pub fn bytes_per_sec(before: u64, before_ms: u64, after: u64, after_ms: u64) -> Option<u64> {
    if after_ms <= before_ms {
        return None;
    }
    if after < before {
        return Some(0);
    }
    let elapsed = after_ms - before_ms;
    Some((after - before).saturating_mul(1000) / elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_mebibyte_in_one_second() {
        let bps = bytes_per_sec(0, 0, 1024 * 1024, 1000).unwrap();
        assert_eq!(crate::wire::format_rate(bps as f64), "1.0 MiB/s");
    }
}
