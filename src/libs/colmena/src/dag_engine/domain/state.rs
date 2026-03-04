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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagTask {
    pub id: String,
    pub run_id: String,
    pub task_name: String,
    pub assigned_to: String,
    pub completed: bool,
    pub result: Option<Value>,
    /// Execution phase (1-based). Tasks in lower phases run first.
    pub phase: i32,
    /// If true, this task is dispatched in the same turn as other parallel tasks in the same phase.
    pub parallel: bool,
}

/// A summary produced by the ReactorNode at the end of a phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagPhaseSummary {
    pub run_id: String,
    pub phase: i32,
    pub summary: String,
}

#[async_trait]
pub trait DagTaskMemoryRepository: Send + Sync {
    async fn add_task(&self, task: &DagTask) -> Result<(), DagError>;
    async fn update_task_result(&self, task_id: &str, result: Value) -> Result<(), DagError>;
    async fn get_tasks_for_run(&self, run_id: &str) -> Result<Vec<DagTask>, DagError>;
    async fn get_first_uncompleted_task(&self, run_id: &str) -> Result<Option<DagTask>, DagError>;
    async fn delete_task(&self, task_id: &str) -> Result<(), DagError>;
    async fn clear_tasks_for_run(&self, run_id: &str) -> Result<(), DagError>;

    // --- Phase-aware routing ---
    /// Returns the lowest phase number that still has incomplete tasks, or None if all done.
    async fn get_current_phase(&self, run_id: &str) -> Result<Option<i32>, DagError>;
    /// Returns all incomplete tasks for a specific phase.
    async fn get_uncompleted_tasks_for_phase(&self, run_id: &str, phase: i32) -> Result<Vec<DagTask>, DagError>;

    // --- Phase summaries (written by ReactorNode, read by final_reactor) ---
    async fn save_phase_summary(&self, run_id: &str, phase: i32, summary: &str) -> Result<(), DagError>;
    async fn get_phase_summaries(&self, run_id: &str) -> Result<Vec<DagPhaseSummary>, DagError>;
}

