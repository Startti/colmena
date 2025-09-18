use colmena::llm::domain::LlmProvider;
use colmena::shared::infrastructure::{ServiceContainerFactory, ConfigResolver};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🐝 Colmena - AI Agent Orchestration Library");

    // Load environment variables
    ConfigResolver::load_env()?;

    // Test basic functionality
    println!("Testing LLM providers...");

    for provider in [LlmProvider::OpenAi, LlmProvider::Gemini, LlmProvider::Anthropic] {
        let container = ServiceContainerFactory::create_for_provider(provider.clone());
        let health_status = container.llm_health_check.execute().await;

        match health_status {
            Ok(status) => {
                if status.is_healthy() {
                    println!("✅ {} is healthy", provider);
                } else {
                    println!("❌ {} is unhealthy: {:?}", provider, status.reason());
                }
            }
            Err(e) => {
                println!("⚠️  {} health check failed: {}", provider, e);
            }
        }
    }

    println!("🎉 Colmena initialization complete!");
    Ok(())
}
