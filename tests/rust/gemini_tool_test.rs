/*
// Example: Testing Gemini Tool Calling
//
// This example demonstrates how to use the Gemini adapter with tool calling.
// Run with: cargo run --example gemini_tool_test
//
// Make sure to set your GEMINI_API_KEY environment variable first:
// export GEMINI_API_KEY="your-api-key-here"

use colmena::llm::domain::LlmRepository;
use colmena::llm::domain::{
    LlmConfig, LlmMessage, LlmProvider, LlmRequest, ParameterProperty, ProviderKind,
    ToolDefinition, ToolParameters,
};
use colmena::llm::infrastructure::GeminiAdapter;
use std::collections::HashMap;

#[tokio::test]
async fn test_gemini_tools() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    println!("🧪 Testing Gemini Function Calling");
    println!("==================================\n");

    // Get API key from environment
    let api_key =
        std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable not set");

    // Create LLM provider and config
    let provider = LlmProvider::new(
        ProviderKind::Gemini,
        api_key,
        Some("gemini-2.5-flash".to_string()),
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

    // Create messages
    let messages = vec![LlmMessage::user(
        "What is 15 + 27? Use the add function to calculate it.".to_string(),
    )?];

    // Create request with tools
    let request = LlmRequest::new(messages, config, false)?.with_tools(vec![add_tool]);

    println!("📤 Sending request to Gemini with 'add' tool...");
    println!("Prompt: What is 15 + 27? Use the add function to calculate it.\n");

    // Call Gemini API
    let adapter = GeminiAdapter::new();
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

    println!("\n✅ Test completed successfully!");

    Ok(())
}
*/
