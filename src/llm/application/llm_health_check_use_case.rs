use crate::llm::domain::{LlmRepository, LlmError};
use std::sync::Arc;

pub struct LlmHealthCheckUseCase {
    repository: Arc<dyn LlmRepository>,
}

impl LlmHealthCheckUseCase {
    pub fn new(repository: Arc<dyn LlmRepository>) -> Self {
        Self { repository }
    }

    pub async fn execute(&self) -> Result<HealthStatus, LlmError> {
        match self.repository.health_check().await {
            Ok(_) => Ok(HealthStatus::Healthy),
            Err(e) => Ok(HealthStatus::Unhealthy {
                reason: e.to_string(),
            }),
        }
    }

    pub fn provider_name(&self) -> &'static str {
        self.repository.provider_name()
    }
}

#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy,
    Unhealthy { reason: String },
}

impl HealthStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            HealthStatus::Healthy => None,
            HealthStatus::Unhealthy { reason } => Some(reason),
        }
    }
}