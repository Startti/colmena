mod handlers;
mod stream;

use axum::{
    routing::{get, post},
    Router,
};
use stream::stream_execution;
use tower_http::cors::{CorsLayer, Any};

use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use handlers::{create_execution, AppState};
use platform_shared::config;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Redis Setup
    let redis_url = config::get_redis_url();
    let client = redis::Client::open(redis_url.clone()).expect("Failed to open Redis client");
    
    tracing::info!("Connected to Redis at {}", redis_url);

    let state = Arc::new(AppState {
        redis_client: client,
    });

    // Router Setup
    // Router Setup
    let app = Router::new()
        .route("/api/v1/health", get(|| async { "OK" }))
        .route("/api/v1/executions", post(create_execution))
        .route("/api/v1/executions/:job_id/stream", get(stream_execution))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state);

    // Run Server
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("API Gateway listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
