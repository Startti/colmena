use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use redis::AsyncCommands;
use std::{convert::Infallible, sync::Arc};
use tokio_stream::StreamExt;
use crate::handlers::AppState;

pub async fn stream_execution(
    Path(job_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    tracing::info!("Client connected to stream for job: {}", job_id);
    let redis_client = state.redis_client.clone();
    let channel_name = format!("events:{}", job_id);

    let stream = async_stream::stream! {
        let mut conn = match redis_client.get_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to connect to Redis: {}", e);
                return;
            }
        };

        // Start reading from the beginning ("0") to catch up on missed events
        let mut last_id = "0".to_string();
        let opts = redis::streams::StreamReadOptions::default()
            .block(5000); // Block for 5s awaiting new items

        loop {
            // XREAD BLOCK 5000 STREAMS key ID
            let result: redis::RedisResult<redis::streams::StreamReadReply> = conn.xread_options(
                &[&channel_name], 
                &[&last_id], 
                &opts
            ).await;

            match result {
                Ok(reply) => {
                    for stream_key in reply.keys {
                        for element in stream_key.ids {
                            last_id = element.id; // Update last_id to progress
                            
                            if let Some(redis::Value::Data(bytes)) = element.map.get("data") {
                                if let Ok(payload) = String::from_utf8(bytes.clone()) {
                                    yield Ok(Event::default().data(payload.clone()));

                                    // Check for completion signals in the payload
                                    if payload.contains("\"graph_finish\"") || payload.contains("\"error\"") {
                                        return; // End the stream
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // Timeouts are normal with block, but other errors aren't
                    // If it's just a timeout (nil), we continue loop. 
                    // Redis crate error handling for xread timeout can vary, usually it returns empty success or specific error.
                    tracing::debug!("Stream read error/timeout: {}", e);
                    // Check if connection is still valid or other fatal errors
                }
            }
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
