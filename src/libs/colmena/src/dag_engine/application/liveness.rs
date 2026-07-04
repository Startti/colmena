//! Liveness configuration for the DAG execution loop: how often to emit a
//! `Progress` heartbeat while a node is silent, and after how much silence
//! an in-flight node is considered hung and aborted.
//! See SPEC_STREAM_MIDRUN_LIVENESS.md (ADP repo, apps/service/ia/platform/).

use std::time::Duration;

pub const DEFAULT_HEARTBEAT_SECS: u64 = 20;
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessSettings {
    /// Silence after which a `Progress` heartbeat is emitted. `None` = no heartbeat.
    pub heartbeat_interval: Option<Duration>,
    /// Silence after which the in-flight node is aborted with an error. `None` = never.
    pub idle_timeout: Option<Duration>,
}

impl Default for LivenessSettings {
    fn default() -> Self {
        Self::normalized(DEFAULT_HEARTBEAT_SECS, DEFAULT_IDLE_TIMEOUT_SECS)
    }
}

impl LivenessSettings {
    pub fn disabled() -> Self {
        Self { heartbeat_interval: None, idle_timeout: None }
    }

    /// Reads `COLMENA_HEARTBEAT_INTERVAL_SECS` / `COLMENA_IDLE_TIMEOUT_SECS`.
    /// Unset or unparsable → defaults. `0` → that knob disabled.
    pub fn from_env() -> Self {
        Self::normalized(
            parse_secs("COLMENA_HEARTBEAT_INTERVAL_SECS", DEFAULT_HEARTBEAT_SECS),
            parse_secs("COLMENA_IDLE_TIMEOUT_SECS", DEFAULT_IDLE_TIMEOUT_SECS),
        )
    }

    /// `0` disables a knob. When both are enabled the heartbeat must beat
    /// before the idle watchdog fires, so `heartbeat >= idle` is clamped to
    /// `idle / 3` (min 1s).
    pub fn normalized(heartbeat_secs: u64, idle_secs: u64) -> Self {
        let idle_timeout = (idle_secs > 0).then(|| Duration::from_secs(idle_secs));
        let mut heartbeat_interval =
            (heartbeat_secs > 0).then(|| Duration::from_secs(heartbeat_secs));
        if let (Some(hb), Some(idle)) = (heartbeat_interval, idle_timeout) {
            if hb >= idle {
                let clamped = (idle_secs / 3).max(1);
                eprintln!(
                    "⚠️ COLMENA_HEARTBEAT_INTERVAL_SECS ({heartbeat_secs}) >= COLMENA_IDLE_TIMEOUT_SECS ({idle_secs}); clamping heartbeat to {clamped}s"
                );
                heartbeat_interval = Some(Duration::from_secs(clamped));
            }
        }
        Self { heartbeat_interval, idle_timeout }
    }
}

fn parse_secs(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("⚠️ {name}='{v}' is not a valid number of seconds; using default {default}");
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_20s_heartbeat_300s_idle() {
        let s = LivenessSettings::default();
        assert_eq!(s.heartbeat_interval, Some(Duration::from_secs(20)));
        assert_eq!(s.idle_timeout, Some(Duration::from_secs(300)));
    }

    #[test]
    fn zero_disables_each_knob_independently() {
        let s = LivenessSettings::normalized(0, 300);
        assert_eq!(s.heartbeat_interval, None);
        assert_eq!(s.idle_timeout, Some(Duration::from_secs(300)));

        let s = LivenessSettings::normalized(20, 0);
        assert_eq!(s.heartbeat_interval, Some(Duration::from_secs(20)));
        assert_eq!(s.idle_timeout, None);

        assert_eq!(LivenessSettings::normalized(0, 0), LivenessSettings::disabled());
    }

    #[test]
    fn heartbeat_gte_idle_is_clamped_to_a_third() {
        // 90s heartbeat vs 60s idle → clamped to 20s (60/3).
        let s = LivenessSettings::normalized(90, 60);
        assert_eq!(s.heartbeat_interval, Some(Duration::from_secs(20)));
        assert_eq!(s.idle_timeout, Some(Duration::from_secs(60)));
        // Tiny idle still yields a >= 1s heartbeat.
        let s = LivenessSettings::normalized(5, 2);
        assert_eq!(s.heartbeat_interval, Some(Duration::from_secs(1)));
    }
}
