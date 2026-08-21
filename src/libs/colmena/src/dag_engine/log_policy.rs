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

/// Event target for `sql_query` node metadata (safe fields only — no raw
/// SQL text).
pub(crate) const T_SQL: &str = "colmena::sql";

/// Event target for orchestrator metadata (planner/critic cycle — safe
/// fields only, never the rendered plan text).
pub(crate) const T_ORCHESTRATOR: &str = "colmena::orchestrator";

/// Event target for `extraction` node metadata (safe fields only — never
/// the raw parsed output, which is LLM-generated).
pub(crate) const T_EXTRACTION: &str = "colmena::extraction";

/// Event target for `reactor` node metadata (prompt/context sizes — never the
/// bodies themselves).
pub(crate) const T_REACTOR: &str = "colmena::reactor";

/// Event target for `llm_call` node metadata (prompt/response sizes — never
/// the bodies themselves).
pub(crate) const T_LLM: &str = "colmena::llm";

/// Payload target carrying the raw Python source body of a `python_script`
/// node execution. Gated by both an `EnvFilter` directive AND
/// `COLMENA_LOG_PAYLOADS` — see [`payload_trace`] and the module doc above.
pub(crate) const P_PYTHON_CODE: &str = "colmena::payload::python_code";

/// Payload target carrying the raw SQL text of a `sql_query` node
/// execution. Same double-gate as [`P_PYTHON_CODE`].
pub(crate) const P_SQL_QUERY: &str = "colmena::payload::sql_query";

/// Payload target carrying the orchestrator's rendered phase plan (the
/// `[agent]: task → ctx` lines) — LLM-authored `task`/`context` text. Same
/// double-gate as [`P_PYTHON_CODE`]/[`P_SQL_QUERY`]. A third payload target
/// beyond the original two-target proposal: without it, a plain `debug!`
/// on the orchestrator event target would still leak this text under
/// `colmena::payload=off`, making that filter directive's promise false.
pub(crate) const P_PLANNER_PLAN: &str = "colmena::payload::planner_plan";

/// Payload target carrying orchestrator agent/reactor I/O: the inputs and
/// raw result JSON exchanged with the internal phase reactor, the
/// orchestrator node's own resolved inputs (`--verbose`), and the raw
/// result an agent subgraph returns (including the LLM-authored
/// `task.task_name` alongside it). Same double-gate as [`P_PYTHON_CODE`].
/// One target for all four sites rather than four narrower ones: they are
/// all "what the orchestrator sent to / received from another node",
/// which is the natural unit an operator would want to enable or silence
/// together.
pub(crate) const P_AGENT_IO: &str = "colmena::payload::agent_io";

/// Payload target carrying the raw parsed output of an `extraction` node
/// execution (LLM-generated structured data). Same double-gate as
/// [`P_PYTHON_CODE`].
pub(crate) const P_EXTRACTION_RESULT: &str = "colmena::payload::extraction_result";

/// Payload target carrying raw LLM request/response bodies — system messages,
/// prompts, and completions from `llm_call` and `reactor`. The most sensitive
/// content in the system: gated by both an `EnvFilter` directive AND
/// `COLMENA_LOG_PAYLOADS`.
pub(crate) const P_LLM_IO: &str = "colmena::payload::llm_io";

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
    (sql_query, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_SQL_QUERY, $($t)*);
        }
    };
    (planner_plan, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_PLANNER_PLAN, $($t)*);
        }
    };
    (agent_io, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_AGENT_IO, $($t)*);
        }
    };
    (extraction_result, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_EXTRACTION_RESULT, $($t)*);
        }
    };
    (llm_io, $($t:tt)*) => {
        if $crate::dag_engine::log_policy::payload_logging_enabled() {
            tracing::trace!(target: $crate::dag_engine::log_policy::P_LLM_IO, $($t)*);
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
            super::T_SQL,
            super::T_ORCHESTRATOR,
            super::T_EXTRACTION,
            super::T_REACTOR,
            super::T_LLM,
            super::P_PYTHON_CODE,
            super::P_SQL_QUERY,
            super::P_PLANNER_PLAN,
            super::P_AGENT_IO,
            super::P_EXTRACTION_RESULT,
            super::P_LLM_IO,
            super::ENV_PAYLOAD_FLAG,
        ] {
            assert!(
                guide.contains(target),
                "target '{target}' must appear verbatim in the guide doc"
            );
        }
    }

    // ── Double-gate coverage for the payload kinds added after PR 1 ─────
    // `python_code` has the four-axis behavioral proof in `python_node.rs`,
    // which drives a real node. The other four kinds cannot be proved that
    // way in CI — `sql_query` would need a live database, `planner_plan`
    // and `agent_io` a full orchestrator run, `extraction_result` a live
    // LLM call — so they are proved here at the macro level: same four
    // axes, same double gate, exercised through `payload_trace!`.
    //
    // Honest limit: this proves the MACHINERY for these kinds, not that
    // their call sites route raw content through it. The regression fence
    // catches a reintroduced `println!`; nothing automatically catches raw
    // content added as a field on a safe `debug!` event.

    use std::io::Write;
    use std::sync::{Arc, Mutex as StdMutex};
    use tracing_subscriber::{fmt, EnvFilter};

    #[derive(Clone, Default)]
    struct BufWriter(Arc<StdMutex<Vec<u8>>>);
    impl<'a> fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufHandle;
        fn make_writer(&'a self) -> Self::Writer {
            BufHandle(self.0.clone())
        }
    }
    struct BufHandle(Arc<StdMutex<Vec<u8>>>);
    impl Write for BufHandle {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    const CANARY: &str = "CANARY_4b7d21e0_payload_marker";

    /// Emit both post-PR-1 payload kinds under `filter`, with the guard
    /// forced to `guard`, and return everything the subscriber captured.
    /// Both gates are thread-local here (`set_default` and `test_override`),
    /// so the axes run in parallel with no `#[serial]`.
    fn capture(filter: &str, guard: bool) -> String {
        let buf = BufWriter::default();
        let subscriber = fmt::Subscriber::builder()
            .with_writer(buf.clone())
            .with_env_filter(EnvFilter::new(filter))
            .finish();
        let _override = super::test_override::set(guard);
        let _tracing = tracing::subscriber::set_default(subscriber);
        super::payload_trace!(sql_query, query = %CANARY);
        super::payload_trace!(planner_plan, plan = %CANARY);
        super::payload_trace!(agent_io, io = %CANARY);
        super::payload_trace!(extraction_result, parsed = %CANARY);
        super::payload_trace!(llm_io, prompt = %CANARY);
        drop(_tracing);
        let captured = buf.0.lock().unwrap().clone();
        String::from_utf8(captured).unwrap()
    }

    #[test]
    fn new_kinds_absent_at_default_posture() {
        let out = capture("info", false);
        assert!(
            !out.contains(CANARY),
            "payload leaked at default posture: {out}"
        );
    }

    // Load-bearing: a reflexive `RUST_LOG=trace` must not open the gate on
    // its own. This is the axis that fails if the guard check is deleted.
    #[test]
    fn new_kinds_absent_under_bare_trace_without_guard() {
        let out = capture("colmena=trace", false);
        assert!(
            !out.contains(CANARY),
            "trace filter alone must not expose payload: {out}"
        );
    }

    #[test]
    fn new_kinds_absent_when_directive_blocks_the_target() {
        let out = capture("colmena=trace,colmena::payload=off", true);
        assert!(
            !out.contains(CANARY),
            "guard alone must not expose payload when the directive blocks the target: {out}"
        );
    }

    #[test]
    fn new_kinds_present_when_both_gates_open() {
        let out = capture("colmena::payload=trace", true);
        assert!(
            out.contains(CANARY),
            "both gates open, payload should be visible: {out}"
        );
        assert!(
            out.contains(super::P_SQL_QUERY),
            "captured record must carry the sql_query target: {out}"
        );
        assert!(
            out.contains(super::P_PLANNER_PLAN),
            "captured record must carry the planner_plan target: {out}"
        );
        assert!(
            out.contains(super::P_AGENT_IO),
            "captured record must carry the agent_io target: {out}"
        );
        assert!(
            out.contains(super::P_EXTRACTION_RESULT),
            "captured record must carry the extraction_result target: {out}"
        );
        assert!(
            out.contains(super::P_LLM_IO),
            "captured record must carry the llm_io target: {out}"
        );
    }

    // ── Regression fence (finding #30) ──────────────────────────────────
    // Honest framing: this proves regression-resistance — a future edit
    // can't silently reintroduce a raw print of user/LLM content — NOT
    // security. It is a source-text scan, not a runtime guarantee; see the
    // four-axis behavioral test in `python_node.rs` for the property that
    // actually matters.

    #[test]
    fn no_raw_print_macros_outside_cfg_test_in_migrated_node_files() {
        let files: &[(&str, &str)] = &[
            (
                "python_node.rs",
                include_str!("infrastructure/nodes/python_node.rs"),
            ),
            ("sql.rs", include_str!("infrastructure/nodes/sql.rs")),
            (
                "orchestrator.rs",
                include_str!("infrastructure/nodes/orchestrator.rs"),
            ),
            (
                "extraction.rs",
                include_str!("infrastructure/nodes/extraction.rs"),
            ),
            (
                "reactor.rs",
                include_str!("infrastructure/nodes/reactor.rs"),
            ),
            ("llm.rs", include_str!("infrastructure/nodes/llm.rs")),
        ];
        for (name, content) in files {
            // Truncate at the first `#[cfg(test)]` marker — everything from
            // there on is test code, out of scope for this fence. Files
            // with no test module (e.g. `orchestrator.rs`) scan in full.
            let production_code = match content.find("#[cfg(test)]") {
                Some(idx) => &content[..idx],
                None => content,
            };
            for macro_name in ["println!", "eprintln!", "print!"] {
                assert!(
                    !production_code.contains(macro_name),
                    "{name} contains a raw `{macro_name}` outside #[cfg(test)] — \
                     route it through `tracing::debug!`/`warn!` (event) or \
                     `payload_trace!` (payload) instead. See \
                     docs/developer_guide/50_logging_and_observability.md"
                );
            }
        }
    }

    // ── Narrower fence: no JSON-dump class inside `colmena_log!` ────────
    // Complements the fence above. That one catches ANY raw print macro;
    // this one specifically catches the shape this PR fixed — a
    // `colmena_log!` call whose arguments render a
    // `serde_json::to_string_pretty` value — reintroduced through a
    // legitimate-looking `tracing::debug!`-adjacent call that a future
    // edit adds back as a plain `colmena_log!` (which the print-macro
    // fence above would also catch, but this makes the specific failure
    // mode explicit and gives a more targeted message).
    //
    // Honest limit: this is a source-text scan for one textual pattern,
    // not a data-flow analysis. It catches the JSON-dump class this PR
    // fixed. It does NOT catch every way raw content could reach
    // `colmena_log!` — a bare `{:?}` on a raw value, or a `String` built
    // elsewhere (e.g. `let dump = format!("{:#?}", x); colmena_log!("{}",
    // dump)`) both slip through undetected. A blanket `{:?}` check was
    // considered and rejected: this crate legitimately debug-formats many
    // safe values (session ids, enums, `Option`s) in `colmena_log!` calls,
    // so that check would be too noisy to act on.
    #[test]
    fn no_pretty_json_dump_inside_colmena_log_in_migrated_node_files() {
        let files: &[(&str, &str)] = &[
            (
                "orchestrator.rs",
                include_str!("infrastructure/nodes/orchestrator.rs"),
            ),
            (
                "extraction.rs",
                include_str!("infrastructure/nodes/extraction.rs"),
            ),
            (
                "reactor.rs",
                include_str!("infrastructure/nodes/reactor.rs"),
            ),
            ("llm.rs", include_str!("infrastructure/nodes/llm.rs")),
        ];
        for (name, content) in files {
            let production_code = match content.find("#[cfg(test)]") {
                Some(idx) => &content[..idx],
                None => content,
            };
            for call in colmena_log_invocations(production_code) {
                assert!(
                    !call.contains("serde_json::to_string_pretty"),
                    "{name} routes a `serde_json::to_string_pretty` dump through \
                     `colmena_log!`: {call}\nSplit it into a safe `tracing::debug!` \
                     event (metadata only) plus a `payload_trace!` body (the raw \
                     value) instead. See \
                     docs/developer_guide/50_logging_and_observability.md"
                );
            }
        }
    }

    /// Extract the full text of every `colmena_log!(...)` invocation in
    /// `source`, balancing parentheses so a multi-line call is captured
    /// whole. A naive text scan, not a real parser — sufficient for a
    /// regression fence over this crate's call sites, none of which put a
    /// literal `(` or `)` inside a string argument.
    fn colmena_log_invocations(source: &str) -> Vec<&str> {
        let marker = "colmena_log!(";
        let bytes = source.as_bytes();
        let mut out = Vec::new();
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(marker) {
            let start = search_from + rel;
            let mut depth: i32 = 1; // the opening '(' in the marker itself
            let mut end = start + marker.len();
            while end < bytes.len() && depth > 0 {
                match bytes[end] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                end += 1;
            }
            out.push(&source[start..end]);
            search_from = end;
        }
        out
    }
}
