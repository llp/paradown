use crate::config::RetryConfig;
use std::time::Duration;

pub(crate) fn next_retry_delay(config: &RetryConfig, retry_count: u32) -> Option<Duration> {
    if retry_count >= config.max_retries {
        return None;
    }

    Some(compute_retry_delay(config, retry_count.saturating_add(1)))
}

fn compute_retry_delay(config: &RetryConfig, attempt: u32) -> Duration {
    let initial_delay = config.initial_delay.max(1);
    let max_delay = config.max_delay.max(initial_delay);
    let backoff_factor = config.backoff_factor.max(1.0);
    let exponent = attempt.saturating_sub(1).min(32);
    let calculated = (initial_delay as f64) * backoff_factor.powi(exponent as i32);
    let seconds = calculated.ceil() as u64;

    Duration::from_secs(seconds.clamp(initial_delay, max_delay))
}

#[cfg(test)]
mod tests {
    use super::next_retry_delay;
    use crate::config::RetryConfig;
    use std::time::Duration;

    #[test]
    fn uses_initial_delay_for_first_retry() {
        let cfg = RetryConfig {
            max_retries: 3,
            initial_delay: 2,
            max_delay: 30,
            backoff_factor: 2.0,
        };

        assert_eq!(next_retry_delay(&cfg, 0), Some(Duration::from_secs(2)));
    }

    #[test]
    fn caps_retry_delay_at_max_delay() {
        let cfg = RetryConfig {
            max_retries: 5,
            initial_delay: 2,
            max_delay: 5,
            backoff_factor: 3.0,
        };

        assert_eq!(next_retry_delay(&cfg, 3), Some(Duration::from_secs(5)));
    }

    #[test]
    fn returns_none_after_retry_budget_is_exhausted() {
        let cfg = RetryConfig {
            max_retries: 2,
            initial_delay: 1,
            max_delay: 30,
            backoff_factor: 2.0,
        };

        assert_eq!(next_retry_delay(&cfg, 2), None);
    }
}
