use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ArtifactId, CoreError, CoreResult, RunId, StepId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowKind {
    Query,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Queued,
    Running,
    WaitingForInput,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub workflow: WorkflowKind,
    pub status: RunStatus,
    pub attempt: u32,
    pub retry_of_run_id: Option<RunId>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Run {
    pub fn new(task_id: TaskId, workflow: WorkflowKind) -> Self {
        let now = Utc::now();
        Self {
            id: RunId::new(),
            task_id,
            workflow,
            status: RunStatus::Queued,
            attempt: 1,
            retry_of_run_id: None,
            version: 1,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
        }
    }

    pub fn start(&mut self) -> CoreResult<()> {
        self.transition_to(RunStatus::Running)
    }

    pub fn wait_for_input(&mut self, _wait_key: impl Into<String>) -> CoreResult<()> {
        self.transition_to(RunStatus::WaitingForInput)
    }

    pub fn resume(&mut self) -> CoreResult<()> {
        self.transition_to(RunStatus::Running)
    }

    pub fn succeed(&mut self) -> CoreResult<()> {
        self.transition_to(RunStatus::Succeeded)
    }

    pub fn fail(&mut self) -> CoreResult<()> {
        self.transition_to(RunStatus::Failed)
    }

    pub fn cancel(&mut self) -> CoreResult<()> {
        self.transition_to(RunStatus::Cancelled)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Cancelled
        )
    }

    pub fn retry_from(previous: &Run) -> CoreResult<Self> {
        if previous.status != RunStatus::Failed {
            return Err(CoreError::invalid_transition(
                "run",
                format!("{:?}", previous.status),
                "Retry",
            ));
        }
        let now = Utc::now();
        Ok(Self {
            id: RunId::new(),
            task_id: previous.task_id,
            workflow: previous.workflow,
            status: RunStatus::Running,
            attempt: previous.attempt + 1,
            retry_of_run_id: Some(previous.id),
            version: 1,
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: None,
        })
    }

    pub fn snapshot(
        &self,
        workflow_state: Value,
        pending_wait_metadata: Option<Value>,
        primary_artifact_id: Option<ArtifactId>,
        last_completed_step_id: Option<StepId>,
    ) -> RunSnapshot {
        RunSnapshot {
            run_id: self.id,
            task_id: self.task_id,
            workflow: self.workflow,
            status: self.status,
            version: self.version,
            workflow_state,
            pending_wait_metadata,
            primary_artifact_id,
            last_completed_step_id,
        }
    }

    fn transition_to(&mut self, next: RunStatus) -> CoreResult<()> {
        if !Self::can_transition(self.status, next) {
            return Err(CoreError::invalid_transition(
                "run",
                format!("{:?}", self.status),
                format!("{:?}", next),
            ));
        }
        let now = Utc::now();
        self.status = next;
        self.updated_at = now;
        self.version += 1;

        if next == RunStatus::Running && self.started_at.is_none() {
            self.started_at = Some(now);
        }

        if self.is_terminal() {
            self.finished_at = Some(now);
        }

        Ok(())
    }

    fn can_transition(current: RunStatus, next: RunStatus) -> bool {
        // *
        // *Queued
        //    │
        //    │ start
        //    ▼
        // Running
        //    │
        //    ├──────────────▶ Succeeded
        //    │
        //    ├──────────────▶ Failed
        //    │
        //    ├──────────────▶ Cancelled
        //    │
        //    │ wait_for_input
        //    ▼
        // WaitingForInput
        //    │
        //    │ resume
        //    ▼
        // Running

        matches!(
            (current, next),
            (RunStatus::Queued, RunStatus::Running)
                | (RunStatus::Queued, RunStatus::Cancelled)
                | (RunStatus::Running, RunStatus::WaitingForInput)
                | (RunStatus::Running, RunStatus::Succeeded)
                | (RunStatus::Running, RunStatus::Failed)
                | (RunStatus::Running, RunStatus::Cancelled)
                | (RunStatus::WaitingForInput, RunStatus::Running)
                | (RunStatus::WaitingForInput, RunStatus::Cancelled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub workflow: WorkflowKind,
    pub status: RunStatus,
    pub version: u64,

    pub workflow_state: Value,

    pub pending_wait_metadata: Option<Value>,

    pub primary_artifact_id: Option<ArtifactId>,

    pub last_completed_step_id: Option<StepId>,
}
