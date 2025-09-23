use crate::llm::domain::{LlmError, LlmRepository};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::MockLlmRepository;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_health_check_healthy() {
        let mut mock_repo = MockLlmRepository::new();

        mock_repo
            .expect_health_check()
            .times(1)
            .returning(|| Ok(()));

        let use_case = LlmHealthCheckUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute().await.unwrap();

        assert!(result.is_healthy());
        assert!(result.reason().is_none());
    }

    #[tokio::test]
    async fn test_health_check_unhealthy() {
        let mut mock_repo = MockLlmRepository::new();
        let error_message = "Connection failed".to_string();

        mock_repo.expect_health_check().times(1).returning(move || {
            Err(LlmError::NetworkError {
                message: error_message.clone(),
            })
        });

        let use_case = LlmHealthCheckUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute().await.unwrap();

        assert!(!result.is_healthy());
        assert!(result.reason().is_some());
        assert!(result.reason().unwrap().contains("Connection failed"));
    }
}
