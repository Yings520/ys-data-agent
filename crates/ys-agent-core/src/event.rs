use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AgentAction, ArtifactId, ArtifactMetadata, CoreError, CoreResult, EventId, PrincipalId, RunId,
    StepId, TaskId, ToolCall, ToolCallId, ToolFailure, WorkspaceId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventActor {
    System,
    Principal { id: PrincipalId },
    Model { model: String },
    Tool { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: EventId,
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub actor: EventActor,
    pub event: VersionedRunEvent,
}

/// Runtime-proposed event before Store assigns identity / sequence / timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRunEvent {
    pub actor: EventActor,
    pub kind: RunEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedRunEvent {
    pub schema_version: u32,
    pub kind: RunEventKind,
}

impl VersionedRunEvent {
    pub const V1: u32 = 1;
    pub fn v1(kind: RunEventKind) -> Self {
        Self {
            schema_version: Self::V1,
            kind,
        }
    }

    pub fn validate_supported(&self) -> CoreResult<()> {
        if self.schema_version == Self::V1 {
            Ok(())
        } else {
            Err(CoreError::UnsupportedSchemaVersion {
                version: self.schema_version,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    RunStarted,
    StepStarted {
        step_id: StepId,
        label: String,
    },
    ModelRequested {
        model_call_id: String,
        context_manifest_id: ArtifactId,
    },
    ModelResponded {
        model_call_id: String,
        action: AgentAction,
    },
    ToolCallProposed {
        call: ToolCall,
    },
    PolicyEvaluated {
        call_id: ToolCallId,
        decision: PolicyDecision,
    },
    ToolExecutionStarted {
        call_id: ToolCallId,
    },
    ToolExecutionSucceeded {
        call_id: ToolCallId,
        artifacts: Vec<ArtifactId>,
    },
    ToolExecutionFailed {
        call_id: ToolCallId,
        failure: ToolFailure,
    },
    ToolExecutionIndeterminate {
        call_id: ToolCallId,
        failure: ToolFailure,
    },
    ArtifactCreated {
        artifact: ArtifactMetadata,
    },
    ClarificationRequested {
        clarification_id: String,
        question: String,
    },
    ClarificationAnswered {
        clarification_id: String,
        answer_artifact_id: ArtifactId,
    },
    RunWaiting {
        reason: String,
    },
    RunResumed,
    RunCompleted {
        primary_artifact_id: ArtifactId,
    },
    RunFailed {
        code: String,
        message: String,
    },
    RunCancelled {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny { code: String, message: String },
    RequireConfirmation { code: String, message: String },
}
