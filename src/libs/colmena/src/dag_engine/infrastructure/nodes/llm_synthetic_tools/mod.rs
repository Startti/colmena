//! Synthetic tools for the LLM node — tools that don't map to DAG nodes.
//!
//! Currently: `load_skill` (progressive-disclosure skill loader).

pub mod load_skill_tool;

pub use load_skill_tool::{
    build_load_skill_tool_definition, dispatch_load_skill, LOAD_SKILL_TOOL_NAME,
};
