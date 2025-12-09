# Extensible Tools System

**Status**: In Progress (Phase 0 Complete)
**Date**: 2025-11-30

This document outlines the architecture and implementation plan for the Extensible Tools System in Colmena. It consolidates previous design documents and progress reports.

## 1. Overview

The goal is to enhance the DAG engine to support:
1.  **Built-in Tools**: Core tools (calculator, string utils, etc.) available without defining DAG nodes.
2.  **Composite Execution**: Combining multiple tool sources (Built-in + DAG Nodes + SubDAGs).
3.  **SubDAG Tools**: Executing entire workflows as single tools.
4.  **Partial Configuration**: Pre-configuring nodes (e.g., HTTP headers) while exposing only specific parameters to the LLM.

## 2. Architecture

We are implementing a **Hybrid Architecture** using a `CompositeToolExecutor` that delegates to specific executors:

```
┌─────────────────────────────────────────────────┐
│            LlmNode / AgentService               │
└─────────────────┬───────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│         CompositeToolExecutor                   │
│  ┌──────────────────────────────────────────┐  │
│  │ 1. BuiltInToolExecutor                    │  │
│  │    - Native Rust functions                │  │
│  │    - calculator, datetime, etc.           │  │
│  └──────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────┐  │
│  │ 2. DagToolExecutor (Existing)             │  │
│  │    - Exposes DAG nodes as tools           │  │
│  │    - Supports partial configuration       │  │
│  └──────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────┐  │
│  │ 3. SubDagToolExecutor (Planned)           │  │
│  │    - Executes external DAG files          │  │
│  └──────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

## 3. Implementation Plan

### Phase 0: Node Schema Improvements (✅ Complete)
- Added `description()` to `ExecutableNode`.
- Implemented `ToolConfiguration` for partial config (fixed values + exposed inputs).
- Updated `DagToolExecutor` to generate dynamic schemas based on configuration.

### Phase 1: Built-in Tools (Next)
- **Goal**: Provide standard tools without DAG overhead.
- **Tasks**:
    1. Create `BuiltInToolExecutor`.
    2. Implement core tools: `calculator`, `get_current_datetime`, `string_length`, `json_parse`.
    3. Integrate with `LlmNode`.

### Phase 2: Composite Executor
- **Goal**: Allow mixing built-in tools and DAG nodes.
- **Tasks**:
    1. Create `CompositeToolExecutor` that holds a list of `Box<dyn ToolExecutor>`.
    2. Implement `execute` (try each executor) and `available_tools` (merge results).
    3. Update `LlmNode` to use the composite executor.

### Phase 3: SubDAG Support
- **Goal**: Modularize complex logic into reusable workflows.
- **Tasks**:
    1. Define `SubDagDefinition` (file path, input schema).
    2. Create `SubDagToolExecutor`.
    3. Implement loading and execution of sub-DAGs with arguments.

## 4. Configuration Reference

### Enabling Tools
Tools are enabled in the `LlmNode` config via `enabled_tools`:

```json
{
  "enabled_tools": ["calculator", "fetch_users", "weather_workflow"]
}
```
- **Specific List**: Only listed tools are available.
- **Wildcard `"*"`**: All available tools are enabled.

### Tool Configuration (Partial Config)
Configure nodes as tools with fixed parameters:

```json
{
  "tool_configurations": {
    "fetch_users": {
      "node_type": "http_request",
      "description": "Fetch user data",
      "fixed_config": {
        "base_url": "https://api.example.com",
        "method": "GET"
      },
      "exposed_inputs": ["query_parameters"]
    }
  }
}
```

## 5. Historical Context (Completed Features)
*See `docs/archive/` for detailed history.*
- **ReAct Loop**: Fully implemented in `AgentService`.
- **Memory**: Tool calls are persisted in SQLite/PostgreSQL.
- **Providers**: OpenAI, Gemini, and Anthropic adapters support tool calling.
