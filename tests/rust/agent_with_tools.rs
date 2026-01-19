/*
use async_trait::async_trait;
use colmena::llm::application::AgentService;
use colmena::llm::domain::{
    LlmConfig, LlmError, LlmProvider, ParameterProperty, ProviderKind, ThreadId, ToolCall,
    ToolDefinition, ToolExecutor, ToolParameters, ToolResult,
};
use colmena::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use colmena::llm::infrastructure::LlmProviderFactory;
use std::collections::HashMap;
use std::sync::Arc;

/// Simple calculator tool executor for demonstration
struct CalculatorToolExecutor;

#[async_trait]
impl ToolExecutor for CalculatorToolExecutor {
    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
        let args: HashMap<String, serde_json::Value> =
            serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                LlmError::InvalidToolCall {
                    reason: format!("Failed to parse args: {}", e),
                }
            })?;

        let result = match tool_call.function.name.as_str() {
            "add" => {
                let a = args.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                (a + b).to_string()
            }
            "multiply" => {
                let a = args.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                (a * b).to_string()
            }
            _ => {
                return Err(LlmError::ToolNotFound {
                    name: tool_call.function.name.clone(),
                })
            }
        };

        Ok(ToolResult {
            tool_call_id: tool_call.id.clone(),
            success: true,
            output: result,
            error: None,
        })
    }

    async fn available_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut props = HashMap::new();
                        props.insert(
                            "a".to_string(),
                            ParameterProperty {
                                property_type: "number".to_string(),
                                description: "First number".to_string(),
                                enum_values: None,
                            },
                        );
                        props.insert(
                            "b".to_string(),
                            ParameterProperty {
                                property_type: "number".to_string(),
                                description: "Second number".to_string(),
                                enum_values: None,
                            },
                        );
                        props
                    },
                    required: vec!["a".to_string(), "b".to_string()],
                },
            },
            ToolDefinition {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                parameters: ToolParameters {
                    schema_type: "object".to_string(),
                    properties: {
                        let mut props = HashMap::new();
                        props.insert(
                            "a".to_string(),
                            ParameterProperty {
                                property_type: "number".to_string(),
                                description: "First number".to_string(),
                                enum_values: None,
                            },
                        );
                        props.insert(
                            "b".to_string(),
                            ParameterProperty {
                                property_type: "number".to_string(),
                                description: "Second number".to_string(),
                                enum_values: None,
                            },
                        );
                        props
                    },
                    required: vec!["a".to_string(), "b".to_string()],
                },
            },
        ]
    }
}

#[tokio::test]
async fn test_agent_execution() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    println!("🤖 AgentService Tool Calling Example\n");

    // 1. Setup LLM provider (using OpenAI)
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY environment variable must be set");

    let provider = LlmProvider::new(
        ProviderKind::OpenAi,
        api_key,
        Some("gpt-4o-mini".to_string()),
    )?;

    let config = LlmConfig::new(provider).with_temperature(0.7)?;

    // 2. Create repositories
    let llm_repo = LlmProviderFactory::create(ProviderKind::OpenAi);
    let conversation_repo = Arc::new(InMemoryConversationRepository::new());

    // 3. Create AgentService
    let agent_service = AgentService::new(llm_repo, conversation_repo);

    // 4. Setup tools
    let tool_executor = CalculatorToolExecutor;
    let tools = tool_executor.available_tools().await;

    println!("📋 Available tools:");
    for tool in &tools {
        println!("  - {}: {}", tool.name, tool.description);
    }
    println!();

    // 5. Run agent with a math problem
    let thread_id = ThreadId("example_thread".to_string());
    let prompt = "What is 15 + 27? And then multiply the result by 3.".to_string();

    println!("💬 User: {}", prompt);
    println!("\n🔄 Running agent with tool calling...\n");

    let params = colmena::llm::application::AgentRunParams {
        thread_id: &thread_id,
        prompt,
        config,
        tools,
        tool_executor: &tool_executor,
        max_iterations: Some(10),
        on_token: None,
    };

    match agent_service.run(params).await {
        Ok(response) => {
            println!("✅ Agent response:");
            println!("   {}", response.content());
            println!("\n📊 Token usage:");
            if let Some(usage) = response.usage() {
                println!("   Total tokens: {}", usage.total_tokens);
                println!("   Prompt tokens: {}", usage.prompt_tokens);
                println!("   Completion tokens: {}", usage.completion_tokens);
            }
        }
        Err(e) => {
            eprintln!("❌ Error: {}", e);
            return Err(e.into());
        }
    }

    println!("\n✨ Example completed successfully!");

    Ok(())
}
*/
