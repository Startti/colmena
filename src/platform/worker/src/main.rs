use redis::AsyncCommands;
use tracing::{info, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use platform_shared::{config, JobRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Worker starting...");

    // Redis Setup
    let redis_url = config::get_redis_url();
    let client = redis::Client::open(redis_url.clone())?;
    
    // Connection 1: Blocking Consumer
    let mut con_consumer = client.get_async_connection().await?;
    
    // Connection 2: Publisher (Multiplexed is better for sharing/cloning if needed, but simple async works here)
    let mut con_publisher = client.get_async_connection().await?;

    info!("Worker connected to Redis at {}", redis_url);
    info!("Waiting for jobs on 'job_queue'...");

    loop {
        // BRPOP: Blocking pop from right (FIFO relative to LPUSH). Timeout 0 = block indefinitely.
        let result: Option<(String, String)> = con_consumer.brpop("job_queue", 0.0).await.ok();

        if let Some((_list, job_json)) = result {
             match serde_json::from_str::<JobRequest>(&job_json) {
                Ok(job) => {
                    info!("Received Job: {}", job.job_id);
                    // CORE-06 execution will go here
                    if let Err(e) = process_job(&job, &mut con_publisher).await {
                         error!("Job {} failed: {}", job.job_id, e);
                    } else {
                         info!("Job {} completed", job.job_id);
                    }
                },
                Err(e) => {
                    error!("Failed to deserialize job: {} | Payload: {}", e, job_json);
                }
             }
        }
    }
}

async fn process_job(job: &JobRequest, redis_con: &mut redis::aio::Connection) -> Result<(), Box<dyn std::error::Error>> {
    use colmena_dag_engine::dag_engine::{
        application::run_use_case::DagRunUseCase,
        infrastructure::registry::HashMapNodeRegistry,
        domain::graph::Graph,
    };
    use colmena_dag_engine::llm::infrastructure::persistence::repository_factory::ConversationRepositoryFactory;
    use std::sync::Arc;
    use futures::StreamExt; // Importante para .next()

    info!("Initializing execution for Job: {}", job.job_id);

    // 1. Deserialize Graph
    let graph: Graph = serde_json::from_value(job.dag_json.clone())
        .map_err(|e| format!("Invalid DAG JSON: {}", e))?;

    // 2. Setup Registry & UseCase
    let repo_factory = Arc::new(ConversationRepositoryFactory::new());
    let registry = HashMapNodeRegistry::new(repo_factory);
    let use_case = DagRunUseCase::new(registry);

    // 3. Execute Stream
    info!("Executing DAG in stream mode...");
    let stream = use_case.execute_stream(graph);
    tokio::pin!(stream);
    
    // Redis Channel
    let channel_name = format!("events:{}", job.job_id);

    while let Some(event_result) = stream.next().await {
        match event_result {
            Ok(event) => {
                // Serialize event to JSON
                if let Ok(json_event) = serde_json::to_string(&event) {
                    // Use XADD instead of PUBLISH to persist events in a stream
                    // Key: events:{job_id}, ID: *, Field: "data", Value: json_event
                    let _: () = redis_con.xadd(
                        &channel_name,
                        "*", 
                        &[("data", json_event)]
                    ).await?;
                }
                
                // If the event is GraphFinish, we can log specific success
                if let colmena_dag_engine::dag_engine::domain::events::DagExecutionEvent::GraphFinish { output } = &event {
                     info!("Job {} finished. Final Output: {:?}", job.job_id, output);
                }
            },
            Err(e) => {
                 error!("Error in DAG stream for job {}: {}", job.job_id, e);
                 // Optionally publish error event
                 // return Err(...) // Should we stop? Yes.
                 return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())));
            }
        }
    }

    Ok(())
}
