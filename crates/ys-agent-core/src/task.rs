use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult, PrincipalId, TaskId, WorkspaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Open,
    InProgress,
    Waiting,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub goal: String,
    pub acceptance_criteria: Vec<String>,
    pub status: TaskStatus,
    pub created_by: PrincipalId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn new(
        workspace_id: WorkspaceId,
        created_by: PrincipalId,
        goal: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: TaskId::new(),
            workspace_id,
            goal: goal.into(),
            acceptance_criteria: Vec::new(),
            status: TaskStatus::Open,
            created_by,
            created_at: now,
            updated_at: now,
            closed_at: None,
        }
    }

    pub fn with_acceptance_criteria<I, S>(mut self, acceptance_criteria: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.acceptance_criteria = acceptance_criteria.into_iter().map(Into::into).collect();
        self
    }

    pub fn start(&mut self) -> CoreResult<()> {
        self.transition_to(TaskStatus::InProgress)
    }

    pub fn resume(&mut self) -> CoreResult<()> {
        self.transition_to(TaskStatus::InProgress)
    }

    pub fn complete(&mut self) -> CoreResult<()> {
        self.transition_to(TaskStatus::Completed)
    }

    pub fn cancel(&mut self) -> CoreResult<()> {
        self.transition_to(TaskStatus::Cancelled)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.status, TaskStatus::Completed | TaskStatus::Cancelled)
    }

    fn transition_to(&mut self, next: TaskStatus) -> CoreResult<()> {
        if !Self::can_transition(self.status, next) {
            return Err(CoreError::invalid_transition(
                "task",
                format!("{:?}", self.status),
                format!("{next:?}"),
            ));
        }
        self.status = next;
        self.updated_at = Utc::now();

        if self.is_terminal() {
            self.closed_at = Some(self.updated_at);
        }

        Ok(())
    }

    fn can_transition(current: TaskStatus, next: TaskStatus) -> bool {
        /*
        *
                       ┌─────────────┐
                       │    Open     │
                       └──────┬──────┘
                              │ start
                              ▼
                        ┌─────────────┐
                   ┌───▶│ InProgress  │──────▶ Completed
                   │    └──────┬──────┘
                   │           │
             resume│           │ wait
                   │           ▼
                   │    ┌─────────────┐
                   └────│   Waiting   │
                        └─────────────┘

               Open ───────────────▶ Cancelled
               InProgress ─────────▶ Cancelled
               Waiting ────────────▶ Cancelled
        */
        matches!(
            (current, next),
            (TaskStatus::Open, TaskStatus::InProgress)
                | (TaskStatus::Open, TaskStatus::Cancelled)
                | (TaskStatus::InProgress, TaskStatus::Completed)
                | (TaskStatus::InProgress, TaskStatus::Cancelled)
                | (TaskStatus::InProgress, TaskStatus::Waiting)
                | (TaskStatus::Waiting, TaskStatus::InProgress)
                | (TaskStatus::Waiting, TaskStatus::Cancelled)
        )
    }
}
