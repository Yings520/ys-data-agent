use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ContextManifest, ToolCall, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantToolCall {
    pub provider_call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_tool_call: Option<AssistantToolCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolSpec>,
    pub context_manifest: ContextManifest,
    pub temperature: Option<f32>,
    #[serde(default)]
    pub tool_choice: ModelToolChoice,
    #[serde(default)]
    pub response_format: ModelResponseFormat,
}

/// Provider-neutral control over whether a model may emit a tool call on this turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelToolChoice {
    #[default]
    Auto,
    Required,
    None,
}

/// Provider-neutral response envelope requested for non-tool output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelResponseFormat {
    #[default]
    Text,
    JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub action: AgentAction,
    pub raw_content: Option<String>,
    pub usage: Option<ModelUsage>,
}

/// Actions a model may propose. Runtime decides execution.
/// v0.2: no dynamic RequestCapability (single executable Workflow).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentAction {
    Respond {
        message: String,
    },
    StartQuery,
    UnsupportedCapability {
        capability: String,
        message: String,
    },
    CallTool {
        call: ToolCall,
    },
    ProposeQueryPlan {
        plan: Value,
    },
    RequestClarification {
        question: String,
    },
    ProposeCompletion {
        summary: String,
        primary_artifact_hint: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub tool_calling: bool,
    pub structured_outputs: bool,
    pub max_context_tokens: u32,
    pub parallel_tool_calls: bool,
    pub streaming: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            tool_calling: false,
            structured_outputs: false,
            max_context_tokens: 128_000,
            parallel_tool_calls: false,
            streaming: false,
        }
    }
}
