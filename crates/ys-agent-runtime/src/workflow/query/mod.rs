mod artifact;
mod prompts;
mod state;
mod verifier;

pub use artifact::{
    MetricReference, ParameterKind, QueryArtifact, QueryArtifactInput, RedactedParameter,
    ResultColumn, ResultSchema,
};
pub use prompts::{QUERY_SYSTEM_PROMPT_VERSION, query_system_instructions};
pub use state::{
    ClarificationNeed, QueryWorkflowState, classify_intent, material_ambiguity,
    requires_current_freshness,
};
pub use verifier::{
    FreshnessState, QueryVerifier, VerificationCheck, VerificationInput, VerificationReport,
};

use ys_agent_core::{
    AgentAction, ArtifactKind, ArtifactMetadata, ArtifactRef, CoreError, CoreResult,
    PolicyDecision, QueryExecutionPlan, QueryIntent, QueryPlan, ToolCall, ToolOutcome,
};

use crate::tools::QueryPhase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowDirective {
    Advance(QueryPhase),
    Classify(QueryIntent),
    AskModel,
    Verify,
    Wait {
        clarification_id: String,
        question: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum WorkflowEffect {
    ToolCall(ToolCall),
    PersistPlan(QueryPlan),
    Wait {
        clarification_id: String,
        question: String,
        reason: String,
    },
    ProposeCompletion(String),
    Repair {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QueryWorkflow;

impl QueryWorkflow {
    pub fn new() -> Self {
        Self
    }

    pub fn next(&self, state: &QueryWorkflowState) -> CoreResult<WorkflowDirective> {
        match state.phase {
            QueryPhase::Clarify => {
                if let Some(need) = material_ambiguity(&state.question) {
                    Ok(WorkflowDirective::Wait {
                        clarification_id: need.id,
                        question: need.question,
                        reason: need.reason,
                    })
                } else {
                    Ok(WorkflowDirective::Advance(QueryPhase::ClassifyIntent))
                }
            }
            QueryPhase::ClassifyIntent => Ok(WorkflowDirective::Classify(classify_intent(
                &state.question,
            ))),
            QueryPhase::ResolveContext
            | QueryPhase::Plan
            | QueryPhase::ValidateAndPreflight
            | QueryPhase::Execute
            | QueryPhase::ReadyToComplete => Ok(WorkflowDirective::AskModel),
            QueryPhase::Verify => {
                if state.intent == Some(QueryIntent::GovernedMetric)
                    && state.freshness_evidence.is_none()
                {
                    Ok(WorkflowDirective::AskModel)
                } else {
                    Ok(WorkflowDirective::Verify)
                }
            }
            QueryPhase::Package => Ok(WorkflowDirective::Advance(QueryPhase::ReadyToComplete)),
        }
    }

    pub fn validate_action(
        &self,
        state: &QueryWorkflowState,
        action: &AgentAction,
    ) -> CoreResult<WorkflowEffect> {
        match action {
            AgentAction::CallTool { call } => {
                let allowed = match state.phase {
                    QueryPhase::ResolveContext => {
                        matches!(call.name.as_str(), "resolve_metric" | "inspect_schema")
                    }
                    QueryPhase::ValidateAndPreflight | QueryPhase::Execute => {
                        call.name == "query_data"
                    }
                    QueryPhase::Verify => call.name == "read_freshness",
                    QueryPhase::Clarify
                    | QueryPhase::ClassifyIntent
                    | QueryPhase::Plan
                    | QueryPhase::Package
                    | QueryPhase::ReadyToComplete => false,
                };
                if !allowed {
                    return Err(CoreError::validation(
                        "tool_not_allowed_in_query_phase",
                        format!("Tool {} is not allowed in {:?}", call.name, state.phase),
                    ));
                }
                Ok(WorkflowEffect::ToolCall(call.clone()))
            }
            AgentAction::ProposeQueryPlan { plan } if state.phase == QueryPhase::Plan => {
                let plan: QueryPlan = serde_json::from_value(plan.clone()).map_err(|error| {
                    CoreError::validation("invalid_query_plan", error.to_string())
                })?;
                match (&state.intent, &plan.execution) {
                    (Some(QueryIntent::GovernedMetric), QueryExecutionPlan::Metric { .. }) => {}
                    (
                        Some(QueryIntent::AdHocRead),
                        QueryExecutionPlan::AdHoc {
                            sql,
                            assumption_refs,
                        },
                    ) if !assumption_refs.is_empty() && looks_like_read_query(sql) => {}
                    (Some(QueryIntent::AdHocRead), QueryExecutionPlan::AdHoc { .. }) => {
                        return Ok(WorkflowEffect::Repair {
                            code: "plan_not_read_like".to_owned(),
                            message:
                                "AdHoc plan must start with SELECT or WITH and cite assumptions"
                                    .to_owned(),
                        });
                    }
                    _ => {
                        return Ok(WorkflowEffect::Repair {
                            code: "plan_intent_mismatch".to_owned(),
                            message: "Proposed plan does not match QueryIntent".to_owned(),
                        });
                    }
                }
                Ok(WorkflowEffect::PersistPlan(plan))
            }
            AgentAction::RequestClarification { question } => Ok(WorkflowEffect::Wait {
                clarification_id: format!("model-clarification-{:x}", stable_text_hash(question)),
                question: question.clone(),
                reason: "model_requested_material_clarification".to_owned(),
            }),
            AgentAction::ProposeCompletion { summary, .. } => {
                Ok(WorkflowEffect::ProposeCompletion(summary.clone()))
            }
            AgentAction::ProposeQueryPlan { .. } => Ok(WorkflowEffect::Repair {
                code: "plan_not_allowed_in_phase".to_owned(),
                message: format!("QueryPlan is not allowed in {:?}", state.phase),
            }),
        }
    }

    pub fn apply_tool_outcome(
        &self,
        state: &mut QueryWorkflowState,
        outcome: &ToolOutcome,
        artifacts: &[ArtifactMetadata],
    ) -> CoreResult<()> {
        match outcome {
            ToolOutcome::Succeeded { output, .. } => {
                state.last_tool_output = Some(output.clone());
                match state.phase {
                    QueryPhase::ResolveContext => {
                        let evidence = artifacts
                            .iter()
                            .find(|artifact| artifact.kind == ArtifactKind::ContextEvidence)
                            .cloned()
                            .ok_or_else(|| {
                                CoreError::validation(
                                    "context_evidence_missing",
                                    "Context Tool success needs a ContextEvidence Artifact",
                                )
                            })?;
                        match state.intent {
                            Some(QueryIntent::GovernedMetric) => {
                                state.metric_evidence = Some(ArtifactRef::new(evidence));
                                state.transition(QueryPhase::Plan)
                            }
                            Some(QueryIntent::AdHocRead) => {
                                state.schema_evidence.push(ArtifactRef::new(evidence));
                                state.transition(QueryPhase::Plan)
                            }
                            Some(QueryIntent::Metadata) => {
                                state.schema_evidence.push(ArtifactRef::new(evidence));
                                state.transition(QueryPhase::Verify)
                            }
                            None => Err(CoreError::validation(
                                "query_intent_missing",
                                "ResolveContext needs QueryIntent",
                            )),
                        }
                    }
                    QueryPhase::ValidateAndPreflight => {
                        let preflight = artifact_of_kind(artifacts, ArtifactKind::QueryPreflight)?;
                        state.preflight = Some(ArtifactRef::new(preflight));
                        state.policy_decision = Some(PolicyDecision::Allow);
                        state.transition(QueryPhase::Execute)
                    }
                    QueryPhase::Execute => {
                        let result = artifact_of_kind(artifacts, ArtifactKind::QueryResult)?;
                        state.query_result = Some(ArtifactRef::new(result));
                        state.transition(QueryPhase::Verify)
                    }
                    QueryPhase::Verify => {
                        let evidence = artifact_of_kind(artifacts, ArtifactKind::ContextEvidence)?;
                        state.freshness_evidence = Some(ArtifactRef::new(evidence));
                        Ok(())
                    }
                    QueryPhase::Clarify
                    | QueryPhase::ClassifyIntent
                    | QueryPhase::Plan
                    | QueryPhase::Package
                    | QueryPhase::ReadyToComplete => Err(CoreError::validation(
                        "unexpected_tool_success",
                        "Current phase cannot accept a Tool outcome",
                    )),
                }
            }
            ToolOutcome::Rejected { failure } | ToolOutcome::Failed { failure }
                if failure.parameter_revision_allowed
                    && matches!(
                        state.phase,
                        QueryPhase::ValidateAndPreflight | QueryPhase::Execute
                    ) =>
            {
                state.return_to_plan(format!("{}:{}", failure.code, failure.user_message))
            }
            ToolOutcome::Rejected { failure } | ToolOutcome::Failed { failure } => {
                Err(CoreError::validation(
                    "query_tool_terminal_failure",
                    format!("{}:{}", failure.code, failure.user_message),
                ))
            }
            ToolOutcome::Indeterminate { .. } => Err(CoreError::validation(
                "query_tool_indeterminate",
                "Indeterminate Tool execution requires recovery",
            )),
        }
    }
}

fn stable_text_hash(value: &str) -> u64 {
    value.bytes().fold(1469598103934665603u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(1099511628211)
    })
}

fn looks_like_read_query(sql: &str) -> bool {
    let sql = sql.trim_start().to_ascii_lowercase();
    sql.starts_with("select ") || sql.starts_with("with ")
}

fn artifact_of_kind(
    artifacts: &[ArtifactMetadata],
    kind: ArtifactKind,
) -> CoreResult<ArtifactMetadata> {
    artifacts
        .iter()
        .find(|artifact| artifact.kind == kind)
        .cloned()
        .ok_or_else(|| {
            CoreError::validation(
                "tool_artifact_missing",
                format!("Tool outcome is missing {kind:?} metadata"),
            )
        })
}
