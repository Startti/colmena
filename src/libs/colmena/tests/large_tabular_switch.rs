//! Integration test: the `COLMENA_LARGE_TABULAR` switch is read once, when
//! `EngineConfig::from_env` builds the config, and is off by default.
//!
//! Own test binary (own process) because it mutates process environment and
//! `from_env` installs the process-wide Python executor. Sets a dummy
//! `DATABASE_URL`: `from_env` only checks that it is set, it never connects.

use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};

#[tokio::test]
async fn the_switch_is_off_by_default_and_read_once_into_the_engine_config() {
    std::env::set_var("DATABASE_URL", "postgres://unused:unused@localhost/unused");
    std::env::remove_var("COLMENA_LARGE_TABULAR");
    let off = EngineConfig::from_env().await.unwrap();
    assert!(!off.prepare.large_tabular, "unset means off");

    std::env::set_var("COLMENA_LARGE_TABULAR", "on");
    let on = EngineConfig::from_env().await.unwrap();
    assert!(on.prepare.large_tabular, "on enables the feature");

    // Read once: the value lives in the built config; a later change of the
    // environment does not reach it (the lifecycle that consumes it arrives in
    // a later slice, which asserts the effect, not just the field).
    std::env::set_var("COLMENA_LARGE_TABULAR", "off");
    assert!(on.prepare.large_tabular);
    let off_again = EngineConfig::from_env().await.unwrap();
    assert!(!off_again.prepare.large_tabular);

    // Switch on with the placeholder trigger: the engine refuses to start
    // (before touching any database) instead of waiting forever per call.
    std::env::set_var("COLMENA_LARGE_TABULAR", "on");
    let unwired = EngineConfig::from_env().await.unwrap();
    match ColmenaEngine::new(unwired).await {
        Ok(_) => panic!("an unwired runner with the switch on must stop start-up"),
        Err(e) => assert!(e.to_string().contains("COLMENA_LARGE_TABULAR"), "{e}"),
    }
}
