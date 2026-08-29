use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};
use ys_agent_core::{
    CoreResult, CostClass, EventActor, PendingRunEvent, PolicyDecision, RunEventKind, RunId,
    RunStatus, SideEffect, Tool, ToolCallId, ToolFailure, ToolFailureCategory, ToolOutcome,
    ToolSpec,
};

use super::{ToolView, WorkspaceToolPolicy, catalog::validate_instance};

#[derive(Clone)]
pub struct GovernedToolContext {
    pub execution: ys_agent_core::ToolExecutionContext,
    pub call_id: ToolCallId,
    pub view: ToolView,
    pub policy: WorkspaceToolPolicy,
    pub run_status: RunStatus,
    pub expected_cost_class: CostClass,
    pub connector_cost_unknown: bool,
}

fn preflight(
    implementation_spec: &ToolSpec,
    context: &GovernedToolContext,
    arguments: &Value,
) -> Result<ToolSpec, ToolFailure> {
    validate_instance(&implementation_spec.input_schema, arguments).map_err(|message| {
        failure(
            "invalid_tool_arguments",
            ToolFailureCategory::InvalidArguments,
            message,
            false,
            true,
            context.expected_cost_class,
        )
    })?;

    let visible_spec = context
        .view
        .spec(&implementation_spec.name)
        .ok_or_else(|| {
            failure(
                "tool_not_visible",
                ToolFailureCategory::Policy,
                "tool is not present in the supplied ToolView",
                false,
                false,
                context.expected_cost_class,
            )
        })?;

    if visible_spec.version != implementation_spec.version {
        return Err(failure(
            "tool_version_mismatch",
            ToolFailureCategory::Policy,
            "tool implementation version does not match the ToolView",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    validate_instance(&visible_spec.input_schema, arguments).map_err(|message| {
        failure(
            "invalid_phase_arguments",
            ToolFailureCategory::InvalidArguments,
            message,
            false,
            true,
            context.expected_cost_class,
        )
    })?;

    if context.run_status != RunStatus::Running {
        return Err(failure(
            "run_not_executable",
            ToolFailureCategory::Policy,
            "tools may execute only while the Run is running",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    let has_effective_permission = visible_spec.required_permissions.len() == 1
        && visible_spec.required_permissions[0] == "data_query";
    if !has_effective_permission {
        return Err(failure(
            "missing_data_query_permission",
            ToolFailureCategory::Authorization,
            "principal is not allowed to use DataQuery tools",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    if visible_spec.side_effect != SideEffect::None {
        return Err(failure(
            "write_rejected",
            ToolFailureCategory::Policy,
            "v0.2 rejects every write-capable tool",
            false,
            false,
            context.expected_cost_class,
        ));
    }
    if !context.policy.allows(visible_spec) {
        return Err(failure(
            "workspace_tool_policy_denied",
            ToolFailureCategory::Policy,
            "Workspace policy denies this tool",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    if let Some(source_id) = arguments.get("source_id").and_then(Value::as_str)
        && source_id != context.execution.data_scope.source_id
    {
        return Err(failure(
            "source_acl_denied",
            ToolFailureCategory::Authorization,
            "requested source is outside the exact allowed data scope",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    if let Some(sql) = arguments.get("sql").and_then(Value::as_str)
        && sql.len() > context.execution.query_budget.max_sql_bytes
    {
        return Err(failure(
            "sql_budget_exceeded",
            ToolFailureCategory::Budget,
            "SQL text exceeds the QueryBudget",
            false,
            true,
            context.expected_cost_class,
        ));
    }

    let budget = &context.execution.query_budget;
    if budget.statement_timeout_ms == 0
        || budget.acquire_timeout_ms == 0
        || budget.max_concurrency == 0
        || budget.max_result_bytes == 0
    {
        return Err(failure(
            "invalid_query_budget",
            ToolFailureCategory::Budget,
            "QueryBudget execution limits must be greater than zero",
            false,
            false,
            context.expected_cost_class,
        ));
    }

    Ok(visible_spec.clone())
}

fn failure(
    code: impl Into<String>,
    category: ToolFailureCategory,
    user_message: impl Into<String>,
    retryable: bool,
    parameter_revision_allowed: bool,
    cost_class: CostClass,
) -> ToolFailure {
    ToolFailure {
        code: code.into(),
        category,
        user_message: user_message.into(),
        retryable,
        parameter_revision_allowed,
        remote_query_id: None,
        cost_class,
    }
}

#[async_trait]
pub trait ToolEventSink: Send + Sync {
    async fn emit(&self, event: PendingRunEvent) -> CoreResult<()>;
}

struct NoopToolEventSink;

#[async_trait]
impl ToolEventSink for NoopToolEventSink {
    async fn emit(&self, _event: PendingRunEvent) -> CoreResult<()> {
        Ok(())
    }
}

pub struct ToolRuntime {
    max_same_call_retries: usize,
    run_semaphores: Mutex<HashMap<RunId, Arc<Semaphore>>>,
    events: Arc<dyn ToolEventSink>,
}

impl ToolRuntime {
    pub fn with_max_same_call_retries(max_same_call_retries: usize) -> Self {
        Self::with_event_sink(max_same_call_retries, Arc::new(NoopToolEventSink))
    }

    pub fn with_event_sink(max_same_call_retries: usize, events: Arc<dyn ToolEventSink>) -> Self {
        Self {
            max_same_call_retries,
            run_semaphores: Mutex::new(HashMap::new()),
            events,
        }
    }

    async fn semaphore_for(&self, context: &GovernedToolContext) -> Arc<Semaphore> {
        let mut semaphores = self.run_semaphores.lock().await;
        semaphores
            .entry(context.execution.run_id)
            .or_insert_with(|| {
                Arc::new(Semaphore::new(
                    context.execution.query_budget.max_concurrency,
                ))
            })
            .clone()
    }

    async fn emit_policy_allow(&self, context: &GovernedToolContext) -> CoreResult<()> {
        self.events
            .emit(PendingRunEvent {
                actor: EventActor::System,
                kind: RunEventKind::PolicyEvaluated {
                    call_id: context.call_id,
                    decision: PolicyDecision::Allow,
                },
            })
            .await
    }

    async fn emit_policy_deny(
        &self,
        context: &GovernedToolContext,
        denied: &ToolFailure,
    ) -> CoreResult<()> {
        self.events
            .emit(PendingRunEvent {
                actor: EventActor::System,
                kind: RunEventKind::PolicyEvaluated {
                    call_id: context.call_id,
                    decision: PolicyDecision::Deny {
                        code: denied.code.clone(),
                        message: denied.user_message.clone(),
                    },
                },
            })
            .await
    }

    async fn emit_started(&self, context: &GovernedToolContext, tool_name: &str) -> CoreResult<()> {
        self.events
            .emit(PendingRunEvent {
                actor: EventActor::Tool {
                    name: tool_name.to_owned(),
                },
                kind: RunEventKind::ToolExecutionStarted {
                    call_id: context.call_id,
                },
            })
            .await
    }

    async fn emit_terminal(
        &self,
        context: &GovernedToolContext,
        tool_name: &str,
        outcome: &ToolOutcome,
    ) -> CoreResult<()> {
        let kind = match outcome {
            ToolOutcome::Succeeded { artifacts, .. } => RunEventKind::ToolExecutionSucceeded {
                call_id: context.call_id,
                artifacts: artifacts.iter().map(|artifact| artifact.id).collect(),
            },
            ToolOutcome::Failed { failure } | ToolOutcome::Rejected { failure } => {
                RunEventKind::ToolExecutionFailed {
                    call_id: context.call_id,
                    failure: failure.clone(),
                }
            }
            ToolOutcome::Indeterminate { failure } => RunEventKind::ToolExecutionIndeterminate {
                call_id: context.call_id,
                failure: failure.clone(),
            },
        };

        self.events
            .emit(PendingRunEvent {
                actor: EventActor::Tool {
                    name: tool_name.to_owned(),
                },
                kind,
            })
            .await
    }

    async fn execute_once(
        &self,
        tool: &Arc<dyn Tool>,
        spec: &ToolSpec,
        context: &GovernedToolContext,
        arguments: &Value,
    ) -> ToolOutcome {
        let semaphore = self.semaphore_for(context).await;
        let acquire_timeout =
            Duration::from_millis(context.execution.query_budget.acquire_timeout_ms);
        let permit = match tokio::time::timeout(acquire_timeout, semaphore.acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => {
                return ToolOutcome::Failed {
                    failure: failure(
                        "tool_concurrency_closed",
                        ToolFailureCategory::Internal,
                        "Tool Runtime concurrency gate is unavailable",
                        false,
                        false,
                        context.expected_cost_class,
                    ),
                };
            }
            Err(_) => {
                return ToolOutcome::Failed {
                    failure: failure(
                        "tool_concurrency_timeout",
                        ToolFailureCategory::Budget,
                        "Tool call could not acquire its concurrency budget in time",
                        context.expected_cost_class == CostClass::Low
                            && !context.connector_cost_unknown,
                        false,
                        context.expected_cost_class,
                    ),
                };
            }
        };

        let effective_timeout_ms = spec
            .timeout_ms
            .min(context.execution.query_budget.statement_timeout_ms);
        let result = tokio::time::timeout(
            Duration::from_millis(effective_timeout_ms),
            tool.execute(&context.execution, arguments.clone()),
        )
        .await;
        drop(permit);

        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => ToolOutcome::Failed {
                failure: failure(
                    "tool_internal",
                    ToolFailureCategory::Internal,
                    format!("tool returned Core error code {}", error.code()),
                    false,
                    false,
                    context.expected_cost_class,
                ),
            },
            Err(_) => ToolOutcome::Indeterminate {
                failure: failure(
                    "tool_timeout_indeterminate",
                    ToolFailureCategory::Timeout,
                    "Tool status is unknown after timeout",
                    false,
                    false,
                    if context.connector_cost_unknown {
                        CostClass::Unknown
                    } else {
                        context.expected_cost_class
                    },
                ),
            },
        }
    }

    fn normalize_outcome(
        &self,
        spec: &ToolSpec,
        context: &GovernedToolContext,
        outcome: ToolOutcome,
    ) -> ToolOutcome {
        let (message, output, artifacts) = match outcome {
            ToolOutcome::Succeeded {
                message,
                output,
                artifacts,
            } => (message, output, artifacts),
            other => return other,
        };

        if validate_instance(&spec.output_schema, &output).is_err() {
            return ToolOutcome::Failed {
                failure: failure(
                    "invalid_tool_output",
                    ToolFailureCategory::Internal,
                    "Tool output did not match its declared schema",
                    false,
                    false,
                    context.expected_cost_class,
                ),
            };
        }

        let output_bytes = match serde_json::to_vec(&output) {
            Ok(bytes) => bytes.len(),
            Err(_) => {
                return ToolOutcome::Failed {
                    failure: failure(
                        "tool_output_serialization",
                        ToolFailureCategory::Internal,
                        "Tool output could not be serialized",
                        false,
                        false,
                        context.expected_cost_class,
                    ),
                };
            }
        };
        let effective_output_limit = spec
            .max_output_bytes
            .min(context.execution.query_budget.max_result_bytes);
        if output_bytes > effective_output_limit {
            return ToolOutcome::Failed {
                failure: failure(
                    "tool_output_budget_exceeded",
                    ToolFailureCategory::Budget,
                    "Tool output exceeds the QueryBudget result byte limit",
                    false,
                    false,
                    context.expected_cost_class,
                ),
            };
        }

        ToolOutcome::Succeeded {
            message,
            output,
            artifacts,
        }
    }

    pub fn safe_preview(
        &self,
        outcome: &ToolOutcome,
        spec: &ToolSpec,
        policy: &WorkspaceToolPolicy,
    ) -> Value {
        match outcome {
            ToolOutcome::Succeeded { output, .. }
                if spec.output_sensitivity <= policy.max_preview_sensitivity =>
            {
                output.clone()
            }
            ToolOutcome::Succeeded { .. } => json!({
                "redacted": true,
                "reason": "output sensitivity exceeds preview policy"
            }),
            ToolOutcome::Failed { failure }
            | ToolOutcome::Rejected { failure }
            | ToolOutcome::Indeterminate { failure } => json!({
                "code": failure.code.clone(),
                "message": failure.user_message.clone()
            }),
        }
    }

    pub async fn execute(
        &self,
        tool: Arc<dyn Tool>,
        context: GovernedToolContext,
        arguments: Value,
    ) -> ToolOutcome {
        let implementation_spec = tool.spec();
        let visible_spec = match preflight(&implementation_spec, &context, &arguments) {
            Ok(spec) => spec,
            Err(denied) => {
                if self.emit_policy_deny(&context, &denied).await.is_err() {
                    return audit_failure(false);
                }
                let outcome = ToolOutcome::Rejected { failure: denied };
                if self
                    .emit_terminal(&context, &implementation_spec.name, &outcome)
                    .await
                    .is_err()
                {
                    return audit_failure(false);
                }
                return outcome;
            }
        };

        if self.emit_policy_allow(&context).await.is_err() {
            return audit_failure(false);
        }
        if self
            .emit_started(&context, &implementation_spec.name)
            .await
            .is_err()
        {
            return audit_failure(false);
        }

        let mut retries_used = 0;
        let outcome = loop {
            let outcome = self
                .execute_once(&tool, &visible_spec, &context, &arguments)
                .await;
            let outcome = self.normalize_outcome(&visible_spec, &context, outcome);

            if retries_used < self.max_same_call_retries
                && retry_same_call_allowed(&visible_spec, &context, &outcome)
            {
                retries_used += 1;
                tokio::task::yield_now().await;
                continue;
            }
            break outcome;
        };

        if self
            .emit_terminal(&context, &implementation_spec.name, &outcome)
            .await
            .is_err()
        {
            return audit_failure(true);
        }

        outcome
    }
}

fn retry_same_call_allowed(
    spec: &ToolSpec,
    context: &GovernedToolContext,
    outcome: &ToolOutcome,
) -> bool {
    if spec.side_effect != SideEffect::None
        || !spec.idempotent
        || context.expected_cost_class != CostClass::Low
        || context.connector_cost_unknown
    {
        return false;
    }

    match outcome {
        ToolOutcome::Failed { failure } => {
            failure.retryable
                && !failure.parameter_revision_allowed
                && failure.remote_query_id.is_none()
                && failure.cost_class == CostClass::Low
        }
        ToolOutcome::Succeeded { .. }
        | ToolOutcome::Rejected { .. }
        | ToolOutcome::Indeterminate { .. } => false,
    }
}

fn audit_failure(execution_may_have_happened: bool) -> ToolOutcome {
    let failure = failure(
        "tool_event_persistence_failed",
        ToolFailureCategory::Internal,
        "Tool Runtime could not persist required audit Events",
        false,
        false,
        CostClass::Unknown,
    );

    if execution_may_have_happened {
        ToolOutcome::Indeterminate { failure }
    } else {
        ToolOutcome::Failed { failure }
    }
}
