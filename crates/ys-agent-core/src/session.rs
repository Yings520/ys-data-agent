use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult, PrincipalId, SessionId, TaskId, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub workspace_id: WorkspaceId,
    pub principal_id: PrincipalId,
    pub focused_task_id: Option<TaskId>,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl Session {
    pub fn new(workspace_id: WorkspaceId, principal_id: PrincipalId) -> Self {
        Self {
            id: SessionId::new(),
            workspace_id,
            principal_id,
            focused_task_id: None,
            created_at: Utc::now(),
            closed_at: None,
        }
    }

    pub fn focus_task(&mut self, task_id: TaskId) -> CoreResult<()> {
        if self.closed_at.is_some() {
            return Err(CoreError::invalid_transition(
                "session", "Closed", "Forcused",
            ));
        }
        self.focused_task_id = Some(task_id);
        Ok(())
    }

    pub fn clear_focused_task(&mut self) -> CoreResult<()> {
        if self.closed_at.is_some() {
            return Err(CoreError::invalid_transition(
                "session",
                "Closed",
                "Unforcused",
            ));
        }
        self.focused_task_id = None;
        Ok(())
    }

    pub fn close(&mut self) -> CoreResult<()> {
        if self.closed_at.is_some() {
            return Err(CoreError::invalid_transition("session", "Closed", "Closed"));
        }
        self.closed_at = Some(Utc::now());
        self.focused_task_id = None;
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.closed_at.is_some()
    }
}
