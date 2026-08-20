//! Payload-logging policy: the crate-internal contract that gates raw
//! user/LLM-controlled content (Python source, SQL text, rendered plans)
//! away from the default log stream.
//!
//! See `docs/developer_guide/50_logging_and_observability.md` for the full
//! target namespace taxonomy and per-environment `RUST_LOG` /
//! `COLMENA_LOG_PAYLOADS` matrix.
//!
//! Two **independent** gates must both be open before any payload record is
//! emitted:
//! 1. The `EnvFilter` directive enables the specific `colmena::payload::*`
//!    target (checked by `tracing` itself, before the callback runs).
//! 2. [`payload_logging_enabled`] returns `true` (checked by
//!    [`payload_trace!`] before calling into `tracing::trace!`).
//!
//! Neither gate alone is sufficient — see the four-axis behavioral test in
//! `python_node.rs`.

use std::sync::OnceLock;

/// Event target for `python_script` node metadata (safe fields only — no
/// raw Python source). Added here as PR 1 lands the first migrated site;
/// `colmena::sql` / `colmena::orchestrator` join in a later slice once
/// their sites migrate, to avoid an unused `pub(crate) const` under this
/// crate's `#![deny(warnings)]` lint policy.
pub(crate) const T_PYTHON_NODE: &str = "colmena::python_node";

/// Payload target carrying the raw Python source body of a `python_script`
/// node execution. Gated by both an `EnvFilter` directive AND
/// `COLMENA_LOG_PAYLOADS` — see [`payload_trace`] and the module doc above.
pub(crate) const P_PYTHON_CODE: &str = "colmena::payload::python_code";

/// Name of the environment variable that opens gate #2. Hoisted to a const so
/// the docs-sync test can bind it to the operator-facing guide: a typo here
/// would silently disable the only mechanism operators are told to use, and
/// no behavioral test can catch it (the `OnceLock` below is a process global
/// that tests cannot re-resolve).
pub(crate) const ENV_PAYLOAD_FLAG: &str = "COLMENA_LOG_PAYLOADS";

static PAYLOAD: OnceLock<bool> = OnceLock::new();

/// Pure resolution of gate #2 from a raw environment value: absent, empty or
/// unparseable all mean closed. Split out of [`payload_logging_enabled`] so
/// the production composition is unit-testable without touching process state.
pub(crate) fn resolve_payload_flag(raw: Option<&str>) -> bool {
    raw.and_then(crate::dag_engine::engine::parse_bool_str)
        .unwrap_or(false)
}

/// Gate #2 of the double-gate payload contract: returns `true` only when
/// `COLMENA_LOG_PAYLOADS` resolves to a truthy value, read lazily and
/// cached for the lifetime of the process.
///
/// Deliberately NOT an `AtomicBool` set once at CLI startup (contrast
/// `dag_engine::verbose`): the ADP worker embeds colmena as a library and
/// never calls colmena's `main`, so a startup-set flag would leave
/// payloads permanently unreachable there. Lazily resolving from the
/// environment on first read works identically whether the host is the
/// `dag_engine` binary or an embedding application.
#[inline]
pub(crate) fn payload_logging_enabled() -> bool {
    #[cfg(test)]
    if let Some(v) = test_override::get() {
        return v;
    }
    *PAYLOAD.get_or_init(|| resolve_payload_flag(std::env::var(ENV_PAYLOAD_FLAG).ok().as_deref()))
}

/// Emit a raw payload record on `tracing::trace!`, but only when
/// [`payload_logging_enabled`] returns `true`. This is the ONLY sanctioned
/// way to emit payload content in this crate: welding the guard check into
/// the macro makes it structurally impossible for a call site to forget it
/// (contrast a plain `if payload_logging_enabled() { tracing::trace!(...) }`
/// written out by hand at every site, which a future maintainer could copy
/// incorrectly or omit).
///
/// New payload kinds (e.g. `sql_query`, `planner_plan`) get their own match
/// arm when their call sites land, alongside their target constants.
macro_rules! payload_trace {
    (python_code, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_PYTHON_CODE, $($t)*);
        }
    };
}
pub(crate) use payload_trace;

/// Test-only thread-local override for the payload guard.
///
/// Scoped identically to `tracing::subscriber::set_default` (also
/// thread-local), so the four-axis behavioral test in `python_node.rs` can
/// run each axis on its own OS thread with zero cross-test interference and
/// no `#[serial]` — unlike the process-global `OnceLock`, a thread-local can
/// be flipped per test without racing `cargo test`'s parallel execution.
#[cfg(test)]
pub(crate) mod test_override {
    use std::cell::Cell;

    thread_local! { static V: Cell<Option<bool>> = const { Cell::new(None) }; }

    pub(super) fn get() -> Option<bool> {
        V.with(|c| c.get())
    }

    /// RAII guard: resets the override to `None` on drop, even on panic,
    /// so a failing assertion never leaks the override into later tests.
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            V.with(|c| c.set(None));
        }
    }

    /// Set the thread-local override for the lifetime of the returned
    /// guard.
    pub(crate) fn set(v: bool) -> Guard {
        V.with(|c| c.set(Some(v)));
        Guard
    }
}

#[cfg(test)]
mod tests {
    use crate::dag_engine::engine::parse_bool_str;

    // ── Pure-parser unit tests ──────────────────────────────────────────
    // These exercise `parse_bool_str` directly — no env read, no `OnceLock`,
    // no process-global state — so they are safe to run in parallel with
    // every other test in the suite (no `#[serial]` needed).

    #[test]
    fn recognizes_truthy_values() {
        for val in ["true", "TRUE", "1", "yes", "on", "  True  "] {
            assert_eq!(
                parse_bool_str(val),
                Some(true),
                "value '{val}' should parse as true"
            );
        }
    }

    #[test]
    fn recognizes_falsy_values() {
        for val in ["false", "FALSE", "0", "no", "off"] {
            assert_eq!(
                parse_bool_str(val),
                Some(false),
                "value '{val}' should parse as false"
            );
        }
    }

    #[test]
    fn returns_none_for_garbage() {
        for val in ["maybe", "", "2", "yesplease"] {
            assert_eq!(parse_bool_str(val), None, "value '{val}' should be None");
        }
    }

    // ── Guard behavior ──────────────────────────────────────────────────

    #[test]
    fn guard_reads_thread_local_override_false() {
        // Asserts the override path returns false. It deliberately does
        // NOT assert the process-env default path: `PAYLOAD` is a
        // process-global `OnceLock` shared by every test in the binary,
        // so a developer shell exporting COLMENA_LOG_PAYLOADS=1 would
        // flip it and make any such assertion flaky. The safe-by-default
        // posture is proven behaviorally instead, by axis (a) of the
        // four-axis test in `python_node.rs`.
        let _guard = super::test_override::set(false);
        assert!(!super::payload_logging_enabled());
    }

    #[test]
    fn guard_reads_thread_local_override_true() {
        let _guard = super::test_override::set(true);
        assert!(super::payload_logging_enabled());
    }

    #[test]
    fn guard_override_resets_on_drop() {
        {
            let _guard = super::test_override::set(true);
            assert!(super::payload_logging_enabled());
        }
        assert!(super::test_override::get().is_none());
    }

    // ── Production resolution path ──────────────────────────────────────
    // `payload_logging_enabled` short-circuits on the test override, so the
    // real env-driven composition is never reached by a behavioral test.
    // These cover it directly.

    #[test]
    fn payload_flag_closed_when_env_absent_or_unparseable() {
        assert!(!super::resolve_payload_flag(None));
        assert!(!super::resolve_payload_flag(Some("")));
        assert!(!super::resolve_payload_flag(Some("maybe")));
    }

    #[test]
    fn payload_flag_opens_only_on_truthy_values() {
        for v in ["1", "true", "TRUE", "yes", "on"] {
            assert!(
                super::resolve_payload_flag(Some(v)),
                "'{v}' should open the payload gate"
            );
        }
        for v in ["0", "false", "no", "off"] {
            assert!(
                !super::resolve_payload_flag(Some(v)),
                "'{v}' should keep the payload gate closed"
            );
        }
    }

    // ── Docs-sync ────────────────────────────────────────────────────────
    // Guards against a silent rename: if a target literal changes here
    // without updating the guide, this test fails instead of the ADP
    // contract silently drifting.

    #[test]
    fn target_consts_appear_verbatim_in_the_guide() {
        let guide =
            include_str!("../../../../../docs/developer_guide/50_logging_and_observability.md");
        for target in [
            super::T_PYTHON_NODE,
            super::P_PYTHON_CODE,
            super::ENV_PAYLOAD_FLAG,
        ] {
            assert!(
                guide.contains(target),
                "target '{target}' must appear verbatim in the guide doc"
            );
        }
    }
}
