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

impl std::fmt::Display for DagRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DagRunStatus::Running => "RUNNING",
            DagRunStatus::Suspended => "SUSPENDED",
            DagRunStatus::Completed => "COMPLETED",
            DagRunStatus::Failed => "FAILED",
        };
        write!(f, "{}", s)
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
    pub session_id: String,

    /// Chat / conversation handle. NULL for legacy runs that never opted in.
    #[serde(default)]
    pub agent_session_id: Option<String>,

    /// session_id of the immediate parent run when this row is a subgraph.
    /// NULL for root runs.
    #[serde(default)]
    pub parent_session_id: Option<String>,

    pub graph_json: Value,
    pub all_outputs: HashMap<String, Value>,
    pub status: DagRunStatus,

    /// Global shared state acting as a persistent whiteboard for all nodes
    #[serde(default)]
    pub global_shared_state: Value,

    /// The current execution queue. When suspending, this captures what is left to run.
    #[serde(default)]
    pub active_queue: std::collections::VecDeque<String>,

    /// Sequence of executed nodes as (CallerId, TargetId)
    #[serde(default)]
    pub execution_history: Vec<(String, String)>,

    /// Total execution count per node
    #[serde(default)]
    pub global_calls: HashMap<String, u32>,

    /// Caller-specific execution count matrix: caller_id -> target_id -> count
    #[serde(default)]
    pub caller_specific_calls: HashMap<String, HashMap<String, u32>>,
}

#[async_trait]
pub trait DagStateRepository: Send + Sync {
    async fn get_by_id(&self, session_id: &str) -> Result<Option<DagRunState>, DagError>;
    async fn save(&self, state: &DagRunState) -> Result<(), DagError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagTask {
    pub id: String,
    pub session_id: String,
    pub task_name: String,
    pub assigned_to: String,
    pub completed: bool,
    pub result: Option<Value>,
    /// Execution phase (1-based). Tasks in lower phases run first.
    pub phase: i32,
    /// If true, this task is dispatched in the same turn as other parallel tasks in the same phase.
    pub parallel: bool,
    /// Optional context describing why this task exists and what the user's intent is.
    /// Set by the Planner or Reactor; used to enrich agent prompts with semantic purpose.
    #[serde(default)]
    pub context: Option<String>,
    /// If true, this task was added by the phase_reactor as a prerequisite that must
    /// complete before the next phase starts. Bridge tasks execute in the same phase
    /// as the one that spawned them; their results are saved as a bridge summary
    /// visible to the next phase.
    #[serde(default)]
    pub is_bridge: bool,
}

/// A summary produced by the ReactorNode at the end of a phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagPhaseSummary {
    pub session_id: String,
    pub phase: i32,
    pub summary: String,
}

#[async_trait]
pub trait DagTaskMemoryRepository: Send + Sync {
    async fn add_task(&self, task: &DagTask) -> Result<(), DagError>;
    async fn update_task_result(&self, task_id: &str, result: Value) -> Result<(), DagError>;
    async fn get_tasks_for_run(&self, session_id: &str) -> Result<Vec<DagTask>, DagError>;
    async fn get_first_uncompleted_task(
        &self,
        session_id: &str,
    ) -> Result<Option<DagTask>, DagError>;
    async fn delete_task(&self, task_id: &str) -> Result<(), DagError>;
    async fn clear_tasks_for_run(&self, session_id: &str) -> Result<(), DagError>;

    // --- Phase-aware routing ---
    /// Returns the lowest phase number that still has incomplete tasks, or None if all done.
    async fn get_current_phase(&self, session_id: &str) -> Result<Option<i32>, DagError>;
    /// Returns all incomplete tasks for a specific phase.
    async fn get_uncompleted_tasks_for_phase(
        &self,
        session_id: &str,
        phase: i32,
    ) -> Result<Vec<DagTask>, DagError>;

    // --- Phase summaries (written by ReactorNode, read by final_reactor) ---
    async fn save_phase_summary(
        &self,
        session_id: &str,
        phase: i32,
        summary: &str,
    ) -> Result<(), DagError>;
    async fn get_phase_summaries(&self, session_id: &str)
        -> Result<Vec<DagPhaseSummary>, DagError>;
}
