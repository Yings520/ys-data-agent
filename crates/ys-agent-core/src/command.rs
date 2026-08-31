use serde::{Deserialize, Serialize};

use crate::{CommandId, RunId, SessionId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandResultKind {
    SessionCreated,
    TaskCreated,
    RunStarted,
    RunResumed,
    ClarificationAnswered,
    RunCancelled,
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
}
