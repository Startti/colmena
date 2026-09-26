//! Integration test: `EngineConfig::from_env` installs the process-wide
//! Python executor before it does anything else that touches storage, so a
//! bad `COLMENA_PYTHON_EXECUTOR` value stops startup with a message that
//! names the offending variable and value — it never falls back to
//! `inprocess`.
//!
//! This runs as its own integration-test binary (own process), not as a unit
//! test inside `engine.rs`: `python_exec`'s installed executor is cached in a
//! process-wide `OnceCell`, so within one process only the first call to
//! `install_from_env` (from any test) actually builds it — every later call,
//! even with a different environment, replays that first result. A unit test
//! sharing the process with the rest of the crate's test suite could pass or
//! fail depending on what ran before it. A fresh process guarantees the
//! `OnceCell` is empty here.
//!
//! Sets a dummy `DATABASE_URL` because `EngineConfig::from_env` reads it
//! before installing the executor (a plain `std::env::var` check, not a
//! connection attempt) — this test asserts on the executor error, not on
//! `DATABASE_URL` being unset, and it must not touch a database either way.

use colmena::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use colmena::dag_engine::engine::EngineConfig;
use colmena::dag_engine::infrastructure::python_exec;

#[tokio::test]
async fn a_bad_executor_value_stops_startup_before_the_database() {
    std::env::set_var("DATABASE_URL", "postgres://unused:unused@localhost/unused");
    std::env::set_var("COLMENA_PYTHON_EXECUTOR", "nope");

    let msg = match EngineConfig::from_env().await {
        Ok(_) => panic!("an invalid COLMENA_PYTHON_EXECUTOR value must fail startup"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("COLMENA_PYTHON_EXECUTOR=\"nope\""),
        "error message must name the offending variable and value, got: {msg}"
    );

    // The misconfiguration is cached: a later `run` call fails the same way
    // without ever invoking Python.
    let req = PythonRunRequest {
        code: String::new(),
        mode: "none".to_string(),
        timeout: None,
        inputs: serde_json::Map::new(),
    };
    match python_exec::run(req).await {
        Err(PythonRunError::Internal(m)) => {
            assert!(
                m.starts_with("PythonExecutorError:"),
                "expected the cached PythonExecutorError prefix, got: {m}"
            );
            assert!(
                m.contains("COLMENA_PYTHON_EXECUTOR"),
                "expected the offending variable name, got: {m}"
            );
        }
        other => panic!("expected Err(PythonRunError::Internal(_)), got: {other:?}"),
    }
}
