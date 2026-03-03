use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::dag_engine::domain::error::DagError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DagRunStatus {
    Running,
    Suspended,
    Completed,
    Failed,
}

impl ToString for DagRunStatus {
    fn to_string(&self) -> String {
        match self {
            DagRunStatus::Running => "RUNNING".to_string(),
            DagRunStatus::Suspended => "SUSPENDED".to_string(),
            DagRunStatus::Completed => "COMPLETED".to_string(),
            DagRunStatus::Failed => "FAILED".to_string(),
        }
    }
}

impl std::str::FromStr for DagRunStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RUNNING" => Ok(DagRunStatus::Running),
            "SUSPENDED" => Ok(DagRunStatus::Suspended),
            "COMPLETED" => Ok(DagRunStatus::Completed),
            "FAILED" => Ok(DagRunStatus::Failed),
            _ => Err(format!("Unknown status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRunState {
    pub run_id: String,
    pub graph_json: Value,
    pub all_outputs: HashMap<String, Value>,
    pub status: DagRunStatus,
}

#[async_trait]
pub trait DagStateRepository: Send + Sync {
    async fn get_by_id(&self, run_id: &str) -> Result<Option<DagRunState>, DagError>;
    async fn save(&self, state: &DagRunState) -> Result<(), DagError>;
}
