# src/libs/colmena/src/dag_engine/application/liveness.rs

**Layer:** application  
**Purpose:** Liveness configuration for DAG execution loop: heartbeat intervals for progress notifications and idle timeouts for aborting hung nodes.

## Symbols

- `DEFAULT_HEARTBEAT_SECS` (pub const) — default heartbeat interval in seconds (20)
- `DEFAULT_IDLE_TIMEOUT_SECS` (pub const) — default idle timeout in seconds (300)
- `LivenessSettings` (pub struct) — holds optional heartbeat_interval and idle_timeout Duration fields
- `LivenessSettings::default` (impl method) — creates default liveness settings via normalized() with 20s heartbeat and 300s idle
- `LivenessSettings::disabled` (pub impl method) — creates liveness settings with both heartbeat and idle_timeout disabled (None)
- `LivenessSettings::from_env` (pub impl method) — reads COLMENA_HEARTBEAT_INTERVAL_SECS / COLMENA_IDLE_TIMEOUT_SECS env vars and normalizes
- `LivenessSettings::normalized` (pub impl method) — validates settings; if heartbeat >= idle, clamps heartbeat to idle/3 (min 1s) and emits warning
- `parse_secs` (fn, private) — parses named env var as u64 seconds, emits warning on parse failure, returns default on unset
- `tests` (mod, cfg test) — test module
- `defaults_are_20s_heartbeat_300s_idle` (test) — verifies default() yields 20s heartbeat and 300s idle
- `zero_disables_each_knob_independently` (test) — verifies 0 value disables heartbeat or idle independently, and disabled() matches (0,0)
- `heartbeat_gte_idle_is_clamped_to_a_third` (test) — verifies heartbeat >= idle clamps to idle/3, with min 1s floor

## File-level notes

- **Clean, focused module** — single responsibility (liveness configuration) with straightforward logic
- **Well-tested** — three test cases cover defaults, per-knob disabling, and clamping logic
- **Direct stderr for config warnings** — uses `eprintln!()` in `normalized()` and `parse_secs()` for immediate feedback at initialization time (acceptable for pre-logging-setup phase)
- **External spec reference** — module docstring points to SPEC_STREAM_MIDRUN_LIVENESS.md in ADP repo (apps/service/ia/platform/) for rationale
- **No unfinished code or dead symbols** — all public APIs are reasonable, all private helpers are used
