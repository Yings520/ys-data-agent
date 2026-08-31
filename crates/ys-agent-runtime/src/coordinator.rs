use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ys_agent_core::{ArtifactId, CoreError, CoreResult, Session, Task, TaskId, TaskStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FutureWorkflow {
    Analysis,
    BuildChange,
    Operate,
    MlDataPrep,
}

impl FutureWorkflow {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Analysis => "Analysis",
            Self::BuildChange => "Build/Change",
            Self::Operate => "Operate",
            Self::MlDataPrep => "ML Data Prep",
        }
    }

    pub fn from_capability(capability: &str) -> Option<Self> {
        match capability.trim().to_ascii_lowercase().as_str() {
            "analysis" => Some(Self::Analysis),
            "build_change" | "build/change" | "build-change" => Some(Self::BuildChange),
            "operate" => Some(Self::Operate),
            "ml_data_prep" | "ml-data-prep" | "ml" => Some(Self::MlDataPrep),
            _ => None,
        }
    }

    pub fn capability_name(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::BuildChange => "build_change",
            Self::Operate => "operate",
            Self::MlDataPrep => "ml_data_prep",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinationDecision {
    FrontDoor {
        input: String,
    },
    ContinueCurrentTask {
        task_id: TaskId,
    },
    CreateNewTask {
        goal: String,
    },
    RequestClarification {
        question: String,
    },
    UnsupportedCapability {
        workflow: FutureWorkflow,
        message: String,
        safe_evidence_refs: Vec<ArtifactId>,
    },
}

#[async_trait]
pub trait Coordinator: Send + Sync {
    async fn route(
        &self,
        session: &Session,
        focused_task: Option<&Task>,
        input: &str,
    ) -> CoreResult<CoordinationDecision>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuleBasedCoordinator;

#[async_trait]
impl Coordinator for RuleBasedCoordinator {
    async fn route(
        &self,
        session: &Session,
        focused_task: Option<&Task>,
        input: &str,
    ) -> CoreResult<CoordinationDecision> {
        let input = input.trim();
        if input.is_empty() {
            return Err(CoreError::validation(
                "empty_message",
                "a message must contain non-whitespace text",
            ));
        }

        if let Some(task) = focused_task
            && task.workspace_id != session.workspace_id
        {
            return Err(CoreError::validation(
                "cross_workspace_task",
                "the focused task belongs to another workspace",
            ));
        }

        let lower = input.to_ascii_lowercase();

        // Safety comes first: an unsupported request must never create a Query Run.
        if let Some(workflow) = unsupported_workflow(&lower) {
            return Ok(CoordinationDecision::UnsupportedCapability {
                workflow,
                message: format!(
                    "{} is not executable in v0.2; no Run was created.",
                    workflow.display_name()
                ),
                safe_evidence_refs: Vec::new(),
            });
        }

        if lower.starts_with("/task new") {
            let goal = input["/task new".len()..].trim();
            return Ok(CoordinationDecision::CreateNewTask {
                goal: if goal.is_empty() { input } else { goal }.to_owned(),
            });
        }

        if let Some(task) = focused_task {
            if is_ambiguous_follow_up(&lower) {
                return Ok(CoordinationDecision::RequestClarification {
                    question: "What should change in the current query?".to_owned(),
                });
            }
            if is_active(task.status) && is_short_contextual_follow_up(&lower) {
                return Ok(CoordinationDecision::ContinueCurrentTask { task_id: task.id });
            }
        }

        Ok(CoordinationDecision::FrontDoor {
            input: input.to_owned(),
        })
    }
}

fn is_active(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Open | TaskStatus::InProgress | TaskStatus::Waiting
    )
}

fn is_ambiguous_follow_up(input: &str) -> bool {
    matches!(input, "change it" | "do the other one" | "something else")
}

fn is_short_contextual_follow_up(input: &str) -> bool {
    if input.split_whitespace().count() > 12 {
        return false;
    }

    let markers = [
        "same ",
        "instead",
        "what about",
        "break it down",
        " by ",
        "only ",
        "include ",
        "exclude ",
        "also ",
        "those ",
    ];
    markers.iter().any(|marker| input.contains(marker))
}

fn unsupported_workflow(input: &str) -> Option<FutureWorkflow> {
    let build_change = [
        "change the dbt",
        "edit the dbt",
        "update the dbt",
        "modify the dbt",
        "create a dbt model",
        "update the model",
        "write code",
    ];
    if build_change.iter().any(|phrase| input.contains(phrase)) {
        return Some(FutureWorkflow::BuildChange);
    }

    let operate = [
        "deploy",
        "rerun the job",
        "restart the pipeline",
        "materialize",
        "orchestrate",
    ];
    if operate.iter().any(|phrase| input.contains(phrase)) {
        return Some(FutureWorkflow::Operate);
    }

    let ml = [
        "train a model",
        "training dataset",
        "feature engineering",
        "prepare ml",
    ];
    if ml.iter().any(|phrase| input.contains(phrase)) {
        return Some(FutureWorkflow::MlDataPrep);
    }

    let analysis = ["why ", "root cause", "explain the drop", "analyze "];
    analysis
        .iter()
        .any(|phrase| input.contains(phrase))
        .then_some(FutureWorkflow::Analysis)
}

#[cfg(test)]
mod tests {
    use super::{FutureWorkflow, unsupported_workflow};

    #[test]
    fn chat_is_not_classified_by_greeting_keywords() {
        assert_eq!(unsupported_workflow("你好，介绍一下你自己"), None);
        assert_eq!(unsupported_workflow("hello there, who are you?"), None);
        assert_eq!(unsupported_workflow("你今天怎么样"), None);
    }
}
