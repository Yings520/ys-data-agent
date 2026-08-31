use serde::{Deserialize, Serialize};

use crate::{ArtifactId, CommandId, RunId, SessionId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandResultKind {
    SessionCreated,
    TaskCreated,
    RunStarted,
    RunResumed,
    ClarificationAnswered,
    RunCancelled,
    ArtifactExported,
    ConversationResponded,
    UnsupportedCapability,
    NoopReplay,
}

/// Returned for every command so identical retries can reuse the original result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub command_id: CommandId,
    pub command_fingerprint: String,
    pub result_kind: CommandResultKind,
    pub session_id: Option<SessionId>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub artifact_id: Option<ArtifactId>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_receipt_without_an_artifact_id_remains_readable() {
        let serialized = serde_json::json!({
            "command_id": CommandId::new(),
            "command_fingerprint": "resume_task",
            "result_kind": "run_resumed",
            "session_id": null,
            "task_id": null,
            "run_id": null,
        });

        let receipt: CommandReceipt =
            serde_json::from_value(serialized).expect("legacy command receipt");

        assert_eq!(receipt.result_kind, CommandResultKind::RunResumed);
        assert_eq!(receipt.artifact_id, None);
        assert_eq!(receipt.message, None);
        assert_eq!(receipt.capability, None);
    }
}
