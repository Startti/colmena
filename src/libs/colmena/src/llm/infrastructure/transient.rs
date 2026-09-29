//! Transient provider answers: which HTTP statuses say nothing about the
//! request or the key, and how long to wait before asking again.

use std::time::Duration;

/// 408, 429, 500, 502, 503, 504 and 529 (Anthropic's "overloaded"). Any other status,
/// a 5xx included, is not transient.
pub fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// Exponential backoff with full jitter: a random wait in
/// `[0, min(cap, base * 2^(attempt - 1))]`. `attempt` counts from 1.
pub fn backoff_with_jitter(attempt: u32, base: Duration, cap: Duration) -> Duration {
    backoff_with(attempt, base, cap, jitter_fraction())
}

fn backoff_with(attempt: u32, base: Duration, cap: Duration, fraction: f64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    let ceiling = base.saturating_mul(1u32 << exponent).min(cap);
    ceiling.mul_f64(fraction.clamp(0.0, 1.0))
}

/// A uniform fraction in `[0, 1)` from 53 of the random low bits of a v4 UUID
/// (the crate already depends on `uuid`; no new dependency for one number).
fn jitter_fraction() -> f64 {
    let bits = (uuid::Uuid::new_v4().as_u128() & ((1u128 << 53) - 1)) as u64;
    bits as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_statuses() {
        for s in [408, 429, 500, 502, 503, 504, 529] {
            assert!(is_transient_status(s), "{s}");
        }
        for s in [200, 400, 401, 403, 404, 422, 501] {
            assert!(!is_transient_status(s), "{s}");
        }
    }

    #[test]
    fn the_ceiling_doubles_per_attempt_and_stops_at_the_cap() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_millis(350);
        assert_eq!(backoff_with(1, base, cap, 1.0), Duration::from_millis(100));
        assert_eq!(backoff_with(2, base, cap, 1.0), Duration::from_millis(200));
        assert_eq!(backoff_with(3, base, cap, 1.0), Duration::from_millis(350));
        assert_eq!(backoff_with(40, base, cap, 1.0), Duration::from_millis(350));
    }

    #[test]
    fn the_wait_is_a_fraction_of_the_ceiling() {
        let base = Duration::from_millis(100);
        let cap = Duration::from_secs(1);
        assert_eq!(backoff_with(1, base, cap, 0.0), Duration::ZERO);
        assert_eq!(backoff_with(1, base, cap, 0.5), Duration::from_millis(50));
        assert_eq!(backoff_with(1, base, cap, 7.0), Duration::from_millis(100));
    }

    #[test]
    fn jitter_is_spread_over_the_unit_interval() {
        let samples: Vec<f64> = (0..200).map(|_| jitter_fraction()).collect();
        assert!(samples.iter().all(|f| (0.0..1.0).contains(f)));
        assert!(samples.iter().any(|f| *f < 0.5) && samples.iter().any(|f| *f >= 0.5));
        assert!(
            backoff_with_jitter(1, Duration::from_millis(100), Duration::from_secs(1))
                <= Duration::from_millis(100)
        );
    }
}
