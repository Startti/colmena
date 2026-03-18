/*
// Example: Testing OpenAI Tool Calling
//
// This example demonstrates how to use the OpenAI adapter with tool calling.
// Run with: cargo run --example openai_tool_test
//
// Make sure to set your OPENAI_API_KEY environment variable first:
// export OPENAI_API_KEY="your-api-key-here"

use colmena::llm::domain::LlmRepository;
use colmena::llm::domain::{
    LlmConfig, LlmMessage, LlmProvider, LlmRequest, ParameterProperty, ProviderKind,
    ToolDefinition, ToolParameters,
};
use colmena::llm::infrastructure::OpenAiAdapter;
use std::collections::HashMap;

#[tokio::test]
async fn test_openai_tools() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    println!("🧪 Testing OpenAI Function Calling");
    println!("==================================\n");

    // Get API key from environment
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY environment variable not set");

    // Create LLM provider and config
    let provider = LlmProvider::new(
        ProviderKind::OpenAi,
        api_key,
        Some("gpt-4o-mini".to_string()),
    )?;

    let config = LlmConfig::new(provider);

    // Define a simple "add" tool
    let mut properties = HashMap::new();
    properties.insert(
        "a".to_string(),
        ParameterProperty::new("number".to_string(), "First number to add".to_string()),
    );
    properties.insert(
        "b".to_string(),
        ParameterProperty::new("number".to_string(), "Second number to add".to_string()),
    );

    let add_tool = ToolDefinition {
        name: "add".to_string(),
        description: "Add two numbers together and return the result".to_string(),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["a".to_string(), "b".to_string()],
        },
    };

    // Define a "multiply" tool
    let mut multiply_properties = HashMap::new();
    multiply_properties.insert(
        "a".to_string(),
        ParameterProperty::new("number".to_string(), "First number to multiply".to_string()),
    );
    multiply_properties.insert(
        "b".to_string(),
        ParameterProperty::new(
            "number".to_string(),
            "Second number to multiply".to_string(),
        ),
    );

    let multiply_tool = ToolDefinition {
        name: "multiply".to_string(),
        description: "Multiply two numbers together and return the result".to_string(),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties: multiply_properties,
            required: vec!["a".to_string(), "b".to_string()],
        },
    };

    // Create messages
    let messages = vec![LlmMessage::user(
        "What is (15 + 27) * 3? Use the add and multiply functions to calculate it step by step."
            .to_string(),
    )?];

    // Create request with tools
    let request =
        LlmRequest::new(messages, config, false)?.with_tools(vec![add_tool, multiply_tool]);

    println!("📤 Sending request to OpenAI with 'add' and 'multiply' tools...");
    println!("Prompt: What is (15 + 27) * 3? Use the add and multiply functions to calculate it step by step.\n");

    // Call OpenAI API
    let adapter = OpenAiAdapter::new();
    let response = adapter.call(request).await?;

    // Display results
    println!("📥 Response received:");
    println!("Content: {}", response.content());

    if let Some(tool_calls) = response.tool_calls() {
        println!("\n🔧 Tool Calls:");
        for (i, call) in tool_calls.iter().enumerate() {
            println!("  {}. Function: {}", i + 1, call.function.name);
            println!("     Arguments: {}", call.function.arguments);
            println!("     Call ID: {}", call.id);
        }
    } else {
        println!("\n❌ No tool calls in response");
    }

    if let Some(usage) = response.usage() {
        println!("\n📊 Token Usage:");
        println!("  Prompt tokens: {}", usage.prompt_tokens);
        println!("  Completion tokens: {}", usage.completion_tokens);
        println!("  Total tokens: {}", usage.total_tokens);
    }

    if let Some(finish_reason) = response.finish_reason() {
        println!("\n🏁 Finish Reason: {}", finish_reason);
    }

    println!("\n✅ Test completed successfully!");

    Ok(())
}
*/
