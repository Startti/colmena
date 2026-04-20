//! Validated configuration for the `PgPoolRegistry`.

use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_entries: usize,
    pub max_conn_per_url: u32,
    pub min_conn_per_url: u32,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub acquire_timeout: Duration,
}

impl PoolConfig {
    pub fn defaults() -> Self {
        Self {
            max_entries: 100,
            max_conn_per_url: 2,
            min_conn_per_url: 0,
            idle_timeout: Duration::from_secs(30),
            max_lifetime: Duration::from_secs(600),
            acquire_timeout: Duration::from_secs(10),
        }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        fn parse<T: std::str::FromStr>(var: &'static str, default: T) -> Result<T, ConfigError> {
            match std::env::var(var) {
                Ok(s) => s.trim().parse::<T>().map_err(|_| ConfigError::Invalid {
                    var,
                    reason: format!("could not parse value '{}'", s),
                }),
                Err(_) => Ok(default),
            }
        }

        let max_entries: usize = parse("COLMENA_POOL_MAX_ENTRIES", 100)?;
        let max_conn_per_url: u32 = parse("COLMENA_POOL_MAX_CONN_PER_URL", 2)?;
        let min_conn_per_url: u32 = parse("COLMENA_POOL_MIN_CONN_PER_URL", 0)?;
        let idle_timeout_sec: u64 = parse("COLMENA_POOL_IDLE_TIMEOUT_SEC", 30)?;
        let max_lifetime_sec: u64 = parse("COLMENA_POOL_MAX_LIFETIME_SEC", 600)?;
        let acquire_timeout_sec: u64 = parse("COLMENA_POOL_ACQUIRE_TIMEOUT_SEC", 10)?;

        if !(1..=10_000).contains(&max_entries) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_ENTRIES",
                reason: format!("{} out of range 1..=10000", max_entries),
            });
        }
        if !(1..=50).contains(&max_conn_per_url) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_CONN_PER_URL",
                reason: format!("{} out of range 1..=50", max_conn_per_url),
            });
        }
        if min_conn_per_url > max_conn_per_url {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MIN_CONN_PER_URL",
                reason: format!(
                    "{} cannot exceed max_conn_per_url={}",
                    min_conn_per_url, max_conn_per_url
                ),
            });
        }
        if !(10..=3600).contains(&idle_timeout_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_IDLE_TIMEOUT_SEC",
                reason: format!("{} out of range 10..=3600", idle_timeout_sec),
            });
        }
        if !(60..=86_400).contains(&max_lifetime_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_LIFETIME_SEC",
                reason: format!("{} out of range 60..=86400", max_lifetime_sec),
            });
        }
        if !(1..=60).contains(&acquire_timeout_sec) {
            return Err(ConfigError::Invalid {
                var: "COLMENA_POOL_ACQUIRE_TIMEOUT_SEC",
                reason: format!("{} out of range 1..=60", acquire_timeout_sec),
            });
        }

        Ok(Self {
            max_entries,
            max_conn_per_url,
            min_conn_per_url,
            idle_timeout: Duration::from_secs(idle_timeout_sec),
            max_lifetime: Duration::from_secs(max_lifetime_sec),
            acquire_timeout: Duration::from_secs(acquire_timeout_sec),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn defaults_match_spec() {
        let c = PoolConfig::defaults();
        assert_eq!(c.max_entries, 100);
        assert_eq!(c.max_conn_per_url, 2);
        assert_eq!(c.min_conn_per_url, 0);
        assert_eq!(c.idle_timeout, Duration::from_secs(30));
        assert_eq!(c.max_lifetime, Duration::from_secs(600));
        assert_eq!(c.acquire_timeout, Duration::from_secs(10));
    }

    #[test]
    #[serial]
    fn from_env_uses_defaults_when_unset() {
        // Safety: these tests run serially via `#[serial]`-ish discipline — we
        // clear every known var so accidental leaks from other tests don't leak in.
        for var in [
            "COLMENA_POOL_MAX_ENTRIES",
            "COLMENA_POOL_MAX_CONN_PER_URL",
            "COLMENA_POOL_MIN_CONN_PER_URL",
            "COLMENA_POOL_IDLE_TIMEOUT_SEC",
            "COLMENA_POOL_MAX_LIFETIME_SEC",
            "COLMENA_POOL_ACQUIRE_TIMEOUT_SEC",
        ] {
            std::env::remove_var(var);
        }
        let c = PoolConfig::from_env().unwrap();
        assert_eq!(c.max_entries, 100);
    }

    #[test]
    #[serial]
    fn from_env_rejects_out_of_range() {
        std::env::set_var("COLMENA_POOL_MAX_CONN_PER_URL", "999");
        let err = PoolConfig::from_env().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "COLMENA_POOL_MAX_CONN_PER_URL",
                ..
            }
        ));
        std::env::remove_var("COLMENA_POOL_MAX_CONN_PER_URL");
    }

    #[test]
    #[serial]
    fn from_env_rejects_min_greater_than_max() {
        std::env::set_var("COLMENA_POOL_MIN_CONN_PER_URL", "5");
        std::env::set_var("COLMENA_POOL_MAX_CONN_PER_URL", "3");
        let err = PoolConfig::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }));
        std::env::remove_var("COLMENA_POOL_MIN_CONN_PER_URL");
        std::env::remove_var("COLMENA_POOL_MAX_CONN_PER_URL");
    }
}
