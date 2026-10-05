//! Integration test: the `COLMENA_LARGE_TABULAR` switch is read once, when
//! `EngineConfig::from_env` builds the config, and is off by default.
//!
//! Own test binary (own process) because it mutates process environment and
//! `from_env` installs the process-wide Python executor. Sets a dummy
//! `DATABASE_URL`: `from_env` only checks that it is set, it never connects.

use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::tabular_prepare::ports::PrepareRequest;
use colmena::tabular_prepare::sqlite_registry::SqlitePreparationRegistry;
use colmena::tabular_prepare::{EnsureOutcome, TabularPrepare};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use std::time::Duration;

async fn registry() -> Arc<SqlitePreparationRegistry> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("migrations/sqlite")
        .run(&pool)
        .await
        .unwrap();
    Arc::new(SqlitePreparationRegistry::from_pool(Arc::new(pool)))
}

fn request() -> PrepareRequest {
    PrepareRequest {
        source_key: "chat-attachments/u/s/big.csv".to_string(),
        mime_type: "text/csv".to_string(),
        filename: "big.csv".to_string(),
        size_bytes: 60_000_000,
    }
}

/// What a tool would observe with no wait: `NotEnabled` when the switch is
/// off; with it on and nothing prepared, the request is made and the answer is
/// `StillPreparing`.
async fn outcome(prepare: &TabularPrepare) -> EnsureOutcome {
    prepare
        .ensure_prepared(&request(), Duration::ZERO)
        .await
        .unwrap()
}

fn is_enabled(out: &EnsureOutcome) -> bool {
    matches!(out, EnsureOutcome::StillPreparing { progress: None })
}

#[tokio::test]
async fn the_switch_is_off_by_default_and_read_once_into_the_engine_config() {
    std::env::set_var("DATABASE_URL", "postgres://unused:unused@localhost/unused");
    std::env::remove_var("COLMENA_LARGE_TABULAR");
    let off = EngineConfig::from_env().await.unwrap();
    let off_prepare = TabularPrepare::new(off.prepare.clone(), registry().await);
    assert_eq!(
        outcome(&off_prepare).await,
        EnsureOutcome::NotEnabled,
        "unset means off: behaviour unchanged"
    );

    std::env::set_var("COLMENA_LARGE_TABULAR", "on");
    let on = EngineConfig::from_env().await.unwrap();
    let on_prepare = TabularPrepare::new(on.prepare.clone(), registry().await);
    assert!(
        is_enabled(&outcome(&on_prepare).await),
        "on enables the feature"
    );

    // Read once: flipping the environment afterwards changes neither the
    // config already built nor what the lifecycle does with it.
    std::env::set_var("COLMENA_LARGE_TABULAR", "off");
    assert!(
        is_enabled(&outcome(&on_prepare).await),
        "an enabled config stays enabled"
    );
    std::env::set_var("COLMENA_LARGE_TABULAR", "on");
    assert_eq!(
        outcome(&off_prepare).await,
        EnsureOutcome::NotEnabled,
        "a disabled config stays disabled"
    );

    // A config built after the change does see the new value.
    std::env::set_var("COLMENA_LARGE_TABULAR", "off");
    let rebuilt = EngineConfig::from_env().await.unwrap();
    assert!(!rebuilt.prepare.large_tabular);

    // Switch on with the placeholder trigger: the engine refuses to start
    // (before touching any database) instead of waiting forever per call.
    std::env::set_var("COLMENA_LARGE_TABULAR", "on");
    let unwired = EngineConfig::from_env().await.unwrap();
    match ColmenaEngine::new(unwired).await {
        Ok(_) => panic!("an unwired runner with the switch on must stop start-up"),
        Err(e) => assert!(e.to_string().contains("COLMENA_LARGE_TABULAR"), "{e}"),
    }
}
