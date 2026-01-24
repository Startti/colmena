pub mod config;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JobRequest {
    pub job_id: String,
    pub dag_json: Value,
    pub inputs: Value,
    pub created_at: i64,
}
