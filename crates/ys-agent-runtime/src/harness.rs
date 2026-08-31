use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use ys_agent_core::{
    AllowedDataScope, ArtifactKind, ArtifactMetadata, ArtifactRef, ArtifactStore, CoreError,
    CoreResult, CostClass, EventActor, ModelProvider, PendingRunEvent, Principal, PutArtifact,
    QueryBudget, RetentionPolicy, RunEventKind, RunId, RunSnapshot, RunStatus, Sensitivity, StepId,
    ToolExecutionContext, WorkspaceId,
};

use crate::{
    ContextAssembler, ContextAssemblyRequest, ContextManifestArtifactWriter,
    PersistContextIdentity, PromptBuilder, ToolViewSnapshot,
    loop_driver::{HarnessStep, StepAccounting, StepOutcome},
    telemetry::{TelemetryDispatcher, TelemetryEvent},
    tools::{
        ConnectorToolAvailability, GovernedToolContext, QueryPhase, ToolCatalog, ToolPreparation,
        ToolRuntime, ToolViewBuilder, WorkspaceToolPolicy,
    },
    workflow::query::{QueryWorkflow, QueryWorkflowState, WorkflowDirective, WorkflowEffect},
};

pub struct HarnessConfig {
    pub workspace_id: WorkspaceId,
    pub principal: Principal,
    pub query_budget: QueryBudget,
    pub data_scope: AllowedDataScope,
    pub connector_tools: ConnectorToolAvailability,
    pub tool_policy: WorkspaceToolPolicy,
    pub context_token_budget: u32,
    pub schema_ttl: Duration,
}

pub struct Harness {
    store: Arc<dyn ys_agent_core::RuntimeStore>,
    artifacts: Arc<dyn ArtifactStore>,
    model: Arc<dyn ModelProvider>,
    catalog: Arc<ToolCatalog>,
    tool_runtime: Arc<ToolRuntime>,
    context_assembler: Arc<ContextAssembler>,
    prompt_builder: PromptBuilder,
    manifest_writer: ContextManifestArtifactWriter,
    workflow: QueryWorkflow,
    config: HarnessConfig,
    telemetry: Arc<TelemetryDispatcher>,
}

pub struct HarnessDependencies {
    pub store: Arc<dyn ys_agent_core::RuntimeStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub model: Arc<dyn ModelProvider>,
    pub catalog: Arc<ToolCatalog>,
    pub tool_runtime: Arc<ToolRuntime>,
    pub context_assembler: Arc<ContextAssembler>,
    pub telemetry: Arc<TelemetryDispatcher>,
}

struct ToolStepInput {
    current: RunSnapshot,
    state: QueryWorkflowState,
    responded: PendingRunEvent,
    call: ys_agent_core::ToolCall,
    tool: Arc<dyn ys_agent_core::Tool>,
    governed: GovernedToolContext,
    tokens: u32,
    model_telemetry: TelemetryEvent,
}

struct ModelStepInput {
    current: RunSnapshot,
    state: QueryWorkflowState,
    view: crate::tools::ToolView,
    model_call_id: String,
    response: ys_agent_core::ModelResponse,
    tokens: u32,
    telemetry: TelemetryEvent,
}

impl Harness {
    pub fn new(
        dependencies: HarnessDependencies,
        prompt_builder: PromptBuilder,
        config: HarnessConfig,
    ) -> Self {
        let manifest_writer = ContextManifestArtifactWriter::new(dependencies.artifacts.clone());
        Self {
            store: dependencies.store,
            artifacts: dependencies.artifacts,
            model: dependencies.model,
            catalog: dependencies.catalog,
            tool_runtime: dependencies.tool_runtime,
            context_assembler: dependencies.context_assembler,
            prompt_builder,
            manifest_writer,
            workflow: QueryWorkflow::new(),
            config,
            telemetry: dependencies.telemetry,
        }
    }

    fn next_snapshot(
        &self,
        current: &RunSnapshot,
        state: &QueryWorkflowState,
        status: RunStatus,
        pending_wait_metadata: Option<serde_json::Value>,
        primary_artifact_id: Option<ys_agent_core::ArtifactId>,
        step_id: StepId,
    ) -> CoreResult<RunSnapshot> {
        Ok(RunSnapshot {
            run_id: current.run_id,
            task_id: current.task_id,
            workflow: current.workflow,
            status,
            attempt: current.attempt,
            retry_of_run_id: current.retry_of_run_id,
            version: current.version + 1,
            workflow_state: state.to_snapshot()?,
            pending_wait_metadata,
            primary_artifact_id,
            last_completed_step_id: Some(step_id),
        })
    }

    async fn append(
        &self,
        current: &RunSnapshot,
        artifacts: Vec<ArtifactMetadata>,
        events: Vec<PendingRunEvent>,
        next: &RunSnapshot,
    ) -> CoreResult<()> {
        self.store
            .append(&current.run_id, current.version, artifacts, events, next)
            .await
    }

    async fn advance(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        next_phase: QueryPhase,
    ) -> CoreResult<StepOutcome> {
        let step_id = StepId::new();
        state.transition(next_phase)?;
        let next = self.next_snapshot(
            &current,
            &state,
            RunStatus::Running,
            None,
            current.primary_artifact_id,
            step_id,
        )?;
        self.append(
            &current,
            vec![],
            vec![system_event(RunEventKind::StepStarted {
                step_id,
                label: format!("query::{next_phase:?}"),
            })],
            &next,
        )
        .await?;
        Ok(StepOutcome::Continue {
            snapshot: next,
            accounting: StepAccounting::default(),
        })
    }

    async fn wait(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        clarification_id: String,
        question: String,
        reason: String,
    ) -> CoreResult<StepOutcome> {
        let step_id = StepId::new();
        state.pending_clarification = Some(crate::workflow::query::ClarificationNeed {
            id: clarification_id.clone(),
            question: question.clone(),
            reason: reason.clone(),
        });
        let wait_metadata = json!({
            "clarification_id": clarification_id,
            "question": question,
            "reason": reason,
        });
        let next = self.next_snapshot(
            &current,
            &state,
            RunStatus::WaitingForInput,
            Some(wait_metadata),
            current.primary_artifact_id,
            step_id,
        )?;
        self.append(
            &current,
            vec![],
            vec![
                system_event(RunEventKind::StepStarted {
                    step_id,
                    label: "query::clarify".to_owned(),
                }),
                system_event(RunEventKind::ClarificationRequested {
                    clarification_id,
                    question,
                }),
                system_event(RunEventKind::RunWaiting { reason }),
            ],
            &next,
        )
        .await?;
        Ok(StepOutcome::Wait {
            snapshot: next,
            accounting: StepAccounting::default(),
        })
    }
}

fn system_event(kind: RunEventKind) -> PendingRunEvent {
    PendingRunEvent {
        actor: EventActor::System,
        kind,
    }
}

fn artifact_events(artifacts: &[ArtifactMetadata]) -> Vec<PendingRunEvent> {
    artifacts
        .iter()
        .cloned()
        .map(|artifact| system_event(RunEventKind::ArtifactCreated { artifact }))
        .collect()
}

fn artifact_identity(artifact: &ArtifactRef) -> serde_json::Value {
    json!({
        "artifact_id": artifact.id(),
        "kind": artifact.metadata.kind,
        "content_hash": artifact.metadata.content_hash,
    })
}

impl Harness {
    fn runtime_query_state_message(
        &self,
        state: &QueryWorkflowState,
    ) -> CoreResult<ys_agent_core::ModelMessage> {
        let content = serde_json::to_string(&json!({
            "phase": state.phase,
            "source_id": self.config.data_scope.source_id,
            "intent": state.intent,
            "artifacts": {
                "metric_evidence": state.metric_evidence.as_ref().map(artifact_identity),
                "schema_evidence": state.schema_evidence.iter().map(artifact_identity).collect::<Vec<_>>(),
                "freshness_evidence": state.freshness_evidence.as_ref().map(artifact_identity),
                "execution_plan": state.execution_plan.as_ref().map(artifact_identity),
                "preflight": state.preflight.as_ref().map(artifact_identity),
                "query_result": state.query_result.as_ref().map(artifact_identity),
                "verification_report": state.verification_report.as_ref().map(artifact_identity),
            },
            "assumptions": state.assumptions,
            "warnings": state.warnings,
        }))
        .map_err(|error| {
            CoreError::validation("runtime_query_state_serialization_failed", error.to_string())
        })?;
        Ok(ys_agent_core::ModelMessage {
            role: ys_agent_core::ModelRole::User,
            content: format!("RUNTIME_QUERY_STATE_JSON:\n{content}"),
            tool_call_id: None,
            name: None,
            assistant_tool_call: None,
        })
    }

    async fn read_model_safe_json<T>(&self, artifact: &ArtifactRef) -> CoreResult<Option<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        if artifact.metadata.sensitivity > Sensitivity::Internal {
            return Ok(None);
        }
        let bytes = self
            .artifacts
            .get(
                artifact,
                &ys_agent_core::ArtifactAccessContext {
                    workspace_id: self.config.workspace_id,
                    principal_id: self.config.principal.id,
                    purpose: ys_agent_core::ArtifactAccessPurpose::ModelPreview,
                    max_sensitivity: Sensitivity::Internal,
                },
            )
            .await?;
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            CoreError::validation("model_workflow_evidence_invalid", error.to_string())
        })
    }

    fn workflow_evidence_message(
        artifact: &ArtifactRef,
        content: serde_json::Value,
    ) -> CoreResult<ys_agent_core::ModelMessage> {
        let content = serde_json::to_string(&json!({
            "artifact": artifact_identity(artifact),
            "content": content,
        }))
        .map_err(|error| {
            CoreError::validation("workflow_evidence_serialization_failed", error.to_string())
        })?;
        Ok(ys_agent_core::ModelMessage {
            role: ys_agent_core::ModelRole::User,
            content: format!("UNTRUSTED_WORKFLOW_EVIDENCE_JSON:\n{content}"),
            tool_call_id: None,
            name: None,
            assistant_tool_call: None,
        })
    }

    async fn workflow_evidence_messages(
        &self,
        state: &QueryWorkflowState,
    ) -> CoreResult<Vec<ys_agent_core::ModelMessage>> {
        let mut messages = Vec::new();
        if matches!(
            state.phase,
            QueryPhase::Plan | QueryPhase::Verify | QueryPhase::ReadyToComplete
        ) && let Some(artifact) = &state.metric_evidence
            && let Some(content) = self.read_model_safe_json(artifact).await?
        {
            messages.push(Self::workflow_evidence_message(artifact, content)?);
        }
        if matches!(state.phase, QueryPhase::Plan | QueryPhase::ReadyToComplete) {
            for artifact in &state.schema_evidence {
                if let Some(content) = self.read_model_safe_json(artifact).await? {
                    messages.push(Self::workflow_evidence_message(artifact, content)?);
                }
            }
        }
        if matches!(
            state.phase,
            QueryPhase::Verify | QueryPhase::ReadyToComplete
        ) && let Some(artifact) = &state.freshness_evidence
            && let Some(content) = self.read_model_safe_json(artifact).await?
        {
            messages.push(Self::workflow_evidence_message(artifact, content)?);
        }
        if state.phase == QueryPhase::ReadyToComplete {
            if let Some(artifact) = &state.query_result
                && let Some(result) = self
                    .read_model_safe_json::<StoredResultEvidence>(artifact)
                    .await?
            {
                messages.push(Self::workflow_evidence_message(
                    artifact,
                    json!({
                        "source_id": result.compiled.source_id,
                        "source_relations": result.compiled.source_relations,
                        "metric_id": result.compiled.metric_id,
                        "metric_version": result.compiled.metric_version,
                        "semantic_status": result.compiled.semantic_status,
                        "columns": result.result.columns,
                        "row_count": result.result.row_count,
                        "truncated": result.result.truncated,
                        "warning_codes": result.result.warning_codes,
                        "model_preview": result.result.model_preview,
                    }),
                )?);
            }
            if let Some(artifact) = &state.verification_report
                && let Some(report) = self
                    .read_model_safe_json::<crate::workflow::query::VerificationReport>(artifact)
                    .await?
            {
                messages.push(Self::workflow_evidence_message(
                    artifact,
                    serde_json::to_value(report).map_err(|error| {
                        CoreError::validation(
                            "verification_evidence_serialization_failed",
                            error.to_string(),
                        )
                    })?,
                )?);
            }
        }
        Ok(messages)
    }

    async fn clarification_messages(
        &self,
        state: &QueryWorkflowState,
    ) -> CoreResult<Vec<ys_agent_core::ModelMessage>> {
        let mut messages = Vec::new();
        for artifact in &state.clarification_evidence {
            if artifact.metadata.sensitivity == Sensitivity::Restricted {
                continue;
            }
            let bytes = self
                .artifacts
                .get(
                    artifact,
                    &ys_agent_core::ArtifactAccessContext {
                        workspace_id: self.config.workspace_id,
                        principal_id: self.config.principal.id,
                        purpose: ys_agent_core::ArtifactAccessPurpose::RuntimeVerification,
                        max_sensitivity: Sensitivity::Internal,
                    },
                )
                .await?;
            let answer = String::from_utf8(bytes).map_err(|error| {
                CoreError::validation("clarification_utf8_invalid", error.to_string())
            })?;
            messages.push(ys_agent_core::ModelMessage {
                role: ys_agent_core::ModelRole::User,
                content: format!(
                    "UNTRUSTED_CLARIFICATION_EVIDENCE_JSON:\n{}",
                    serde_json::to_string(&json!({
                        "artifact_id": artifact.id(),
                        "answer": answer,
                    }))
                    .map_err(|error| CoreError::validation(
                        "clarification_serialization_failed",
                        error.to_string(),
                    ))?
                ),
                tool_call_id: None,
                name: None,
                assistant_tool_call: None,
            });
        }
        Ok(messages)
    }
}

impl Harness {
    async fn model_step(
        &self,
        current: RunSnapshot,
        state: QueryWorkflowState,
    ) -> CoreResult<StepOutcome> {
        let step_id = StepId::new();
        let view = ToolViewBuilder::new(self.catalog.as_ref())
            .for_workflow(current.workflow)
            .for_query_phase(state.phase)
            .for_principal(&self.config.principal)
            .with_connector_tools(self.config.connector_tools.clone())
            .for_run_status(current.status)
            .build()?;
        let view_snapshot = ToolViewSnapshot::new(view.content_hash(), view.model_tools())?;
        let assembled = self
            .context_assembler
            .assemble(
                &ContextAssemblyRequest {
                    task_goal: state.question.clone(),
                    query: state.question.clone(),
                    token_budget: self.config.context_token_budget,
                    schema_ttl: self.config.schema_ttl,
                    requires_schema: matches!(
                        state.phase,
                        QueryPhase::ResolveContext | QueryPhase::Plan
                    ),
                    requires_freshness: state.phase == QueryPhase::Verify,
                    recent_task_summary: None,
                    now: Utc::now(),
                },
                &view_snapshot,
            )
            .await?;
        let model_call_id = format!("model-{step_id}");
        let prepared_manifest = self
            .manifest_writer
            .persist(
                &assembled.manifest,
                PersistContextIdentity {
                    workspace_id: self.config.workspace_id,
                    task_id: current.task_id,
                    run_id: current.run_id,
                },
                model_call_id.clone(),
                PromptBuilder::VERSION,
            )
            .await?;
        let mut request = self.prompt_builder.build(
            &state.question,
            state.phase,
            &assembled.manifest,
            &view_snapshot,
        )?;
        request
            .messages
            .push(self.runtime_query_state_message(&state)?);
        request
            .messages
            .extend(self.clarification_messages(&state).await?);
        request
            .messages
            .extend(self.workflow_evidence_messages(&state).await?);

        let started = self.next_snapshot(
            &current,
            &state,
            RunStatus::Running,
            None,
            current.primary_artifact_id,
            step_id,
        )?;
        self.append(
            &current,
            vec![prepared_manifest.metadata],
            vec![
                system_event(RunEventKind::StepStarted {
                    step_id,
                    label: format!("query::{:?}", state.phase),
                }),
                prepared_manifest.artifact_created,
                prepared_manifest.model_requested,
            ],
            &started,
        )
        .await?;

        let model_started_at = Instant::now();
        let response = self.model.complete(request).await?;
        let tokens = response
            .usage
            .as_ref()
            .map(|usage| usage.total_tokens)
            .unwrap_or(0);
        let model_telemetry = TelemetryEvent::ModelUsage {
            run_id: started.run_id,
            model_call_id: model_call_id.clone(),
            prompt_tokens: response
                .usage
                .as_ref()
                .map(|usage| u64::from(usage.prompt_tokens)),
            completion_tokens: response
                .usage
                .as_ref()
                .map(|usage| u64::from(usage.completion_tokens)),
            milliseconds: elapsed_milliseconds(model_started_at),
        };
        self.apply_model_response(ModelStepInput {
            current: started,
            state,
            view,
            model_call_id,
            response,
            tokens,
            telemetry: model_telemetry,
        })
        .await
    }

    async fn apply_model_response(&self, input: ModelStepInput) -> CoreResult<StepOutcome> {
        let ModelStepInput {
            current,
            mut state,
            view,
            model_call_id,
            response,
            tokens,
            telemetry: model_telemetry,
        } = input;
        let effect = self.workflow.validate_action(&state, &response.action)?;
        let responded = system_event(RunEventKind::ModelResponded {
            model_call_id,
            action: response.action,
        });

        match effect {
            WorkflowEffect::ToolCall(call) => {
                let tool = self
                    .catalog
                    .get(&call.name)
                    .ok_or_else(|| CoreError::NotFound {
                        entity: "tool",
                        id: call.name.clone(),
                    })?;
                let governed = GovernedToolContext {
                    execution: ToolExecutionContext {
                        call_id: call.id,
                        workspace_id: self.config.workspace_id,
                        task_id: current.task_id,
                        run_id: current.run_id,
                        principal: self.config.principal.clone(),
                        query_budget: self.config.query_budget.clone(),
                        data_scope: self.config.data_scope.clone(),
                        confirmation_granted: false,
                    },
                    call_id: call.id,
                    view,
                    policy: self.config.tool_policy.clone(),
                    run_status: current.status,
                    expected_cost_class: match state.phase {
                        QueryPhase::Execute => ys_agent_core::CostClass::High,
                        QueryPhase::ValidateAndPreflight
                        | QueryPhase::ResolveContext
                        | QueryPhase::Verify => ys_agent_core::CostClass::Low,
                        QueryPhase::Clarify
                        | QueryPhase::ClassifyIntent
                        | QueryPhase::Plan
                        | QueryPhase::Package
                        | QueryPhase::ReadyToComplete => ys_agent_core::CostClass::Unknown,
                    },
                    connector_cost_unknown: false,
                };
                self.run_tool(ToolStepInput {
                    current,
                    state,
                    responded,
                    call,
                    tool,
                    governed,
                    tokens,
                    model_telemetry,
                })
                .await
            }
            WorkflowEffect::PersistPlan(plan) => {
                self.persist_plan(current, state, responded, plan, tokens, model_telemetry)
                    .await
            }
            WorkflowEffect::Wait {
                clarification_id,
                question,
                reason,
            } => {
                let acknowledged = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], vec![responded], &acknowledged)
                    .await?;
                self.telemetry.emit_after_commit(model_telemetry).await;
                self.wait(acknowledged, state, clarification_id, question, reason)
                    .await
            }
            WorkflowEffect::ProposeCompletion(summary) => {
                self.complete(current, state, responded, summary, tokens, model_telemetry)
                    .await
            }
            WorkflowEffect::Repair { code, message } => {
                let warning = format!("{code}:{message}");
                if state.phase == QueryPhase::Plan {
                    state.warnings.push(warning);
                    state.last_tool_output = None;
                } else {
                    state.return_to_plan(warning)?;
                }
                let next = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], vec![responded], &next)
                    .await?;
                self.telemetry.emit_after_commit(model_telemetry).await;
                Ok(StepOutcome::Continue {
                    snapshot: next,
                    accounting: StepAccounting {
                        model_calls: 1,
                        tool_calls: 0,
                        tokens,
                    },
                })
            }
        }
    }

    async fn run_tool(&self, input: ToolStepInput) -> CoreResult<StepOutcome> {
        let ToolStepInput {
            current,
            mut state,
            responded,
            call,
            tool,
            governed,
            tokens,
            model_telemetry,
        } = input;
        match self
            .tool_runtime
            .prepare(tool, governed, call.arguments.clone())
        {
            ToolPreparation::Rejected { outcome, events } => {
                let mut all_events = vec![
                    responded,
                    system_event(RunEventKind::ToolCallProposed { call }),
                ];
                all_events.extend(events);
                self.workflow
                    .apply_tool_outcome(&mut state, &outcome, &[])?;
                let next = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], all_events, &next).await?;
                self.telemetry.emit_after_commit(model_telemetry).await;
                Ok(StepOutcome::Continue {
                    snapshot: next,
                    accounting: StepAccounting {
                        model_calls: 1,
                        tool_calls: 0,
                        tokens,
                    },
                })
            }
            ToolPreparation::Ready {
                prepared,
                before_io,
            } => {
                let tool_call_id = call.id;
                let tool_name = call.name.clone();
                let mut started_events = vec![
                    responded,
                    system_event(RunEventKind::ToolCallProposed { call }),
                ];
                started_events.extend(before_io);
                let started = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], started_events, &started)
                    .await?;
                self.telemetry.emit_after_commit(model_telemetry).await;

                let tool_started_at = Instant::now();
                let outcome = self.tool_runtime.execute_prepared(&prepared).await;
                let mut metadata = outcome.artifact_metadata().to_vec();
                if let Some(evidence) = self
                    .prepare_context_evidence(&started, &state, &outcome)
                    .await?
                {
                    metadata.push(evidence);
                }
                let terminal = self.tool_runtime.terminal_event(&prepared, &outcome);
                self.workflow
                    .apply_tool_outcome(&mut state, &outcome, &metadata)?;
                let mut events = artifact_events(&metadata);
                events.push(terminal);
                let next = self.next_snapshot(
                    &started,
                    &state,
                    RunStatus::Running,
                    None,
                    started.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&started, metadata, events, &next).await?;
                self.telemetry
                    .emit_after_commit(TelemetryEvent::ToolLatency {
                        run_id: started.run_id,
                        tool_call_id,
                        tool_name,
                        milliseconds: elapsed_milliseconds(tool_started_at),
                        outcome: tool_outcome_code(&outcome).to_owned(),
                    })
                    .await;
                Ok(StepOutcome::Continue {
                    snapshot: next,
                    accounting: StepAccounting {
                        model_calls: 1,
                        tool_calls: 1,
                        tokens,
                    },
                })
            }
        }
    }

    async fn recovered_tool_step(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        call: ys_agent_core::ToolCall,
    ) -> CoreResult<StepOutcome> {
        state.pending_recovery_call = None;
        let recovered_cost = state
            .pending_recovery_cost_class
            .take()
            .unwrap_or(CostClass::Unknown);
        let view = ToolViewBuilder::new(self.catalog.as_ref())
            .for_workflow(current.workflow)
            .for_query_phase(state.phase)
            .for_principal(&self.config.principal)
            .with_connector_tools(self.config.connector_tools.clone())
            .for_run_status(RunStatus::Running)
            .build()?;
        let tool = self
            .catalog
            .get(&call.name)
            .ok_or_else(|| CoreError::NotFound {
                entity: "tool",
                id: call.name.clone(),
            })?;
        let governed = GovernedToolContext {
            execution: ToolExecutionContext {
                call_id: call.id,
                workspace_id: self.config.workspace_id,
                task_id: current.task_id,
                run_id: current.run_id,
                principal: self.config.principal.clone(),
                query_budget: self.config.query_budget.clone(),
                data_scope: self.config.data_scope.clone(),
                confirmation_granted: state.recovery_confirmation_granted,
            },
            call_id: call.id,
            view,
            policy: self.config.tool_policy.clone(),
            run_status: RunStatus::Running,
            expected_cost_class: recovered_cost,
            connector_cost_unknown: recovered_cost == CostClass::Unknown,
        };
        state.recovery_confirmation_granted = false;
        self.run_recovered_tool(current, state, call, tool, governed)
            .await
    }

    async fn run_recovered_tool(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        call: ys_agent_core::ToolCall,
        tool: Arc<dyn ys_agent_core::Tool>,
        governed: GovernedToolContext,
    ) -> CoreResult<StepOutcome> {
        match self
            .tool_runtime
            .prepare(tool, governed, call.arguments.clone())
        {
            ToolPreparation::Rejected { outcome, events } => {
                let mut all_events = vec![system_event(RunEventKind::ToolCallProposed { call })];
                all_events.extend(events);
                self.workflow
                    .apply_tool_outcome(&mut state, &outcome, &[])?;
                let next = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], all_events, &next).await?;
                Ok(StepOutcome::Continue {
                    snapshot: next,
                    accounting: StepAccounting::default(),
                })
            }
            ToolPreparation::Ready {
                prepared,
                before_io,
            } => {
                let tool_call_id = call.id;
                let tool_name = call.name.clone();
                let mut started_events = vec![system_event(RunEventKind::ToolCallProposed {
                    call: call.clone(),
                })];
                started_events.extend(before_io);
                let started = self.next_snapshot(
                    &current,
                    &state,
                    RunStatus::Running,
                    None,
                    current.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&current, vec![], started_events, &started)
                    .await?;

                let tool_started_at = Instant::now();
                let outcome = self.tool_runtime.execute_prepared(&prepared).await;
                let mut metadata = outcome.artifact_metadata().to_vec();
                if let Some(evidence) = self
                    .prepare_context_evidence(&started, &state, &outcome)
                    .await?
                {
                    metadata.push(evidence);
                }
                let terminal = self.tool_runtime.terminal_event(&prepared, &outcome);
                self.workflow
                    .apply_tool_outcome(&mut state, &outcome, &metadata)?;
                let mut terminal_events = artifact_events(&metadata);
                terminal_events.push(terminal);
                let next = self.next_snapshot(
                    &started,
                    &state,
                    RunStatus::Running,
                    None,
                    started.primary_artifact_id,
                    StepId::new(),
                )?;
                self.append(&started, metadata, terminal_events, &next)
                    .await?;
                self.telemetry
                    .emit_after_commit(TelemetryEvent::ToolLatency {
                        run_id: started.run_id,
                        tool_call_id,
                        tool_name,
                        milliseconds: elapsed_milliseconds(tool_started_at),
                        outcome: tool_outcome_code(&outcome).to_owned(),
                    })
                    .await;
                Ok(StepOutcome::Continue {
                    snapshot: next,
                    accounting: StepAccounting {
                        model_calls: 0,
                        tool_calls: 1,
                        tokens: 0,
                    },
                })
            }
        }
    }

    async fn prepare_context_evidence(
        &self,
        current: &RunSnapshot,
        state: &QueryWorkflowState,
        outcome: &ys_agent_core::ToolOutcome,
    ) -> CoreResult<Option<ArtifactMetadata>> {
        if !matches!(state.phase, QueryPhase::ResolveContext | QueryPhase::Verify) {
            return Ok(None);
        }
        let Some(output) = outcome.success_json() else {
            return Ok(None);
        };
        let bytes = serde_json::to_vec(output).map_err(|error| {
            CoreError::validation("tool_evidence_serialization_failed", error.to_string())
        })?;
        self.artifacts
            .put(PutArtifact {
                workspace_id: self.config.workspace_id,
                task_id: current.task_id,
                run_id: current.run_id,
                kind: ArtifactKind::ContextEvidence,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            })
            .await
            .map(Some)
    }

    async fn classify(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        intent: ys_agent_core::QueryIntent,
    ) -> CoreResult<StepOutcome> {
        let step_id = StepId::new();
        state.intent = Some(intent);
        state.transition(QueryPhase::ResolveContext)?;
        let next = self.next_snapshot(
            &current,
            &state,
            RunStatus::Running,
            None,
            current.primary_artifact_id,
            step_id,
        )?;
        self.append(
            &current,
            vec![],
            vec![system_event(RunEventKind::StepStarted {
                step_id,
                label: format!("query::intent::{intent:?}"),
            })],
            &next,
        )
        .await?;
        Ok(StepOutcome::Continue {
            snapshot: next,
            accounting: StepAccounting::default(),
        })
    }

    async fn persist_plan(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
        responded: PendingRunEvent,
        plan: ys_agent_core::QueryPlan,
        tokens: u32,
        model_telemetry: TelemetryEvent,
    ) -> CoreResult<StepOutcome> {
        let bytes = serde_json::to_vec(&plan).map_err(|error| {
            CoreError::validation("query_plan_serialization_failed", error.to_string())
        })?;
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: self.config.workspace_id,
                task_id: current.task_id,
                run_id: current.run_id,
                kind: ArtifactKind::QueryPlan,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            })
            .await?;
        state.execution_plan = Some(ArtifactRef::new(metadata.clone()));
        state.transition(QueryPhase::ValidateAndPreflight)?;
        let next = self.next_snapshot(
            &current,
            &state,
            RunStatus::Running,
            None,
            current.primary_artifact_id,
            StepId::new(),
        )?;
        self.append(
            &current,
            vec![metadata.clone()],
            vec![
                responded,
                system_event(RunEventKind::ArtifactCreated { artifact: metadata }),
            ],
            &next,
        )
        .await?;
        self.telemetry.emit_after_commit(model_telemetry).await;
        Ok(StepOutcome::Continue {
            snapshot: next,
            accounting: StepAccounting {
                model_calls: 1,
                tool_calls: 0,
                tokens,
            },
        })
    }
}

#[async_trait]
impl HarnessStep for Harness {
    async fn step(&self, run_id: &RunId) -> CoreResult<StepOutcome> {
        let current = self.store.load_run(run_id).await?;
        if current.status != RunStatus::Running {
            return Ok(StepOutcome::Terminal {
                snapshot: current,
                accounting: StepAccounting::default(),
            });
        }
        let state = QueryWorkflowState::from_snapshot(current.workflow_state.clone())?;
        if let Some(call) = state.pending_recovery_call.clone() {
            return self.recovered_tool_step(current, state, call).await;
        }
        match self.workflow.next(&state)? {
            WorkflowDirective::Advance(phase) => self.advance(current, state, phase).await,
            WorkflowDirective::Classify(intent) => self.classify(current, state, intent).await,
            WorkflowDirective::AskModel => self.model_step(current, state).await,
            WorkflowDirective::Verify => self.verify(current, state).await,
            WorkflowDirective::Wait {
                clarification_id,
                question,
                reason,
            } => {
                self.wait(current, state, clarification_id, question, reason)
                    .await
            }
        }
    }

    async fn emit_terminal_run_latency(&self, run_id: &RunId, elapsed: Duration) {
        self.telemetry
            .emit_after_commit(TelemetryEvent::RunLatency {
                run_id: *run_id,
                milliseconds: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            })
            .await;
    }

    async fn fail_terminal(
        &self,
        run_id: &RunId,
        code: &'static str,
        message: String,
    ) -> CoreResult<RunSnapshot> {
        let current = self.store.load_run(run_id).await?;
        let state = QueryWorkflowState::from_snapshot(current.workflow_state.clone())?;
        let next = self.next_snapshot(
            &current,
            &state,
            RunStatus::Failed,
            None,
            current.primary_artifact_id,
            StepId::new(),
        )?;
        self.append(
            &current,
            vec![],
            vec![system_event(RunEventKind::RunFailed {
                code: code.to_owned(),
                message,
            })],
            &next,
        )
        .await?;
        Ok(next)
    }
}

#[derive(serde::Deserialize)]
struct StoredCompiledQuery {
    source_id: ys_agent_core::SourceId,
    sql: String,
    parameters: Vec<ys_agent_core::QueryParameter>,
    source_relations: Vec<String>,
    metric_id: Option<String>,
    metric_version: Option<String>,
    semantic_status: ys_agent_core::SemanticStatus,
}

#[derive(serde::Deserialize)]
struct StoredResultEvidence {
    compiled: StoredCompiledQuery,
    result: ys_agent_core::QueryResult,
}

#[derive(serde::Deserialize)]
struct StoredPreflightEvidence {
    budget_hash: String,
    scope_hash: String,
}

#[derive(serde::Deserialize)]
struct FreshnessToolEvidence {
    source_id: ys_agent_core::SourceId,
    relation: String,
    observed_at: chrono::DateTime<Utc>,
    latest_data_at: Option<chrono::DateTime<Utc>>,
    age_seconds: Option<u64>,
    is_fresh: Option<bool>,
}

#[derive(serde::Deserialize)]
struct StoredSchemaEvidence {
    relations: Vec<StoredObservedRelation>,
}

#[derive(serde::Deserialize)]
struct StoredObservedRelation {
    name: String,
}

impl Harness {
    async fn read_json<T>(&self, artifact: &ArtifactRef) -> CoreResult<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let bytes = self
            .artifacts
            .get(
                artifact,
                &ys_agent_core::ArtifactAccessContext {
                    workspace_id: self.config.workspace_id,
                    principal_id: self.config.principal.id,
                    purpose: ys_agent_core::ArtifactAccessPurpose::RuntimeVerification,
                    max_sensitivity: Sensitivity::Restricted,
                },
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|error| {
            CoreError::validation("verification_artifact_invalid", error.to_string())
        })
    }
}

fn verification_hash<T: Serialize>(value: &T) -> CoreResult<String> {
    use sha2::{Digest, Sha256};

    let bytes = serde_json::to_vec(value)
        .map_err(|error| CoreError::validation("verification_hash_failed", error.to_string()))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn elapsed_milliseconds(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn tool_outcome_code(outcome: &ys_agent_core::ToolOutcome) -> &'static str {
    match outcome {
        ys_agent_core::ToolOutcome::Succeeded { .. } => "succeeded",
        ys_agent_core::ToolOutcome::Rejected { .. } => "rejected",
        ys_agent_core::ToolOutcome::Failed { .. } => "failed",
        ys_agent_core::ToolOutcome::Indeterminate { .. } => "indeterminate",
    }
}

impl Harness {
    async fn verification_input(
        &self,
        state: &QueryWorkflowState,
    ) -> CoreResult<crate::workflow::query::VerificationInput> {
        use crate::workflow::query::FreshnessState;
        use ys_agent_core::{Capability, CellValue, QueryExecutionPlan, SemanticStatus, TimeRange};

        let intent = state.intent.ok_or_else(|| {
            CoreError::validation("query_intent_missing", "Verification needs QueryIntent")
        })?;
        let plan = match &state.execution_plan {
            Some(artifact) => Some(self.read_json::<ys_agent_core::QueryPlan>(artifact).await?),
            None => None,
        };
        let result = match &state.query_result {
            Some(artifact) => Some(self.read_json::<StoredResultEvidence>(artifact).await?),
            None => None,
        };
        let preflight = match &state.preflight {
            Some(artifact) => Some(self.read_json::<StoredPreflightEvidence>(artifact).await?),
            None => None,
        };
        let freshness = match &state.freshness_evidence {
            Some(artifact) => Some(self.read_json::<FreshnessToolEvidence>(artifact).await?),
            None => None,
        };
        let metric: Option<ys_agent_core::MetricDefinition> = match &state.metric_evidence {
            Some(artifact) => Some(self.read_json(artifact).await?),
            None => None,
        };

        let requested_time_range = plan.as_ref().and_then(|plan| match &plan.execution {
            QueryExecutionPlan::Metric { start, end, .. } => Some(TimeRange {
                start: *start,
                end: *end,
            }),
            QueryExecutionPlan::AdHoc { .. } => None,
        });
        let compiled_time_range =
            result
                .as_ref()
                .and_then(|result| match result.compiled.parameters.as_slice() {
                    [
                        ys_agent_core::QueryParameter::Timestamp(start),
                        ys_agent_core::QueryParameter::Timestamp(end),
                    ] => Some(TimeRange {
                        start: *start,
                        end: *end,
                    }),
                    _ => None,
                });
        let requested_metric = metric
            .as_ref()
            .map(|metric| (metric.id.clone(), metric.version.clone()));
        let compiled_metric = result.as_ref().and_then(|result| {
            Some((
                result.compiled.metric_id.clone()?,
                result.compiled.metric_version.clone()?,
            ))
        });
        let assumption_refs = plan
            .as_ref()
            .and_then(|plan| match &plan.execution {
                QueryExecutionPlan::AdHoc {
                    assumption_refs, ..
                } => Some(assumption_refs.clone()),
                QueryExecutionPlan::Metric { .. } => None,
            })
            .unwrap_or_default();
        let result_empty = result
            .as_ref()
            .is_some_and(|result| result.result.row_count == 0);
        let result_all_null = result.as_ref().is_some_and(|result| {
            !result.result.rows.is_empty()
                && result
                    .result
                    .rows
                    .iter()
                    .flatten()
                    .all(|cell| matches!(cell, CellValue::Null))
        });
        let semantic_status = result
            .as_ref()
            .map(|result| result.compiled.semantic_status)
            .unwrap_or(SemanticStatus::Observed);
        let freshness_state = match freshness.as_ref().and_then(|value| value.is_fresh) {
            Some(true) => FreshnessState::Fresh,
            Some(false) => FreshnessState::Stale,
            None if intent == ys_agent_core::QueryIntent::Metadata => FreshnessState::NotRequired,
            None => FreshnessState::Unknown,
        };
        let relation_matches = match (&metric, &result) {
            (Some(metric), Some(result)) => {
                result.compiled.source_relations == vec![metric.source_relation.clone()]
            }
            _ => intent != ys_agent_core::QueryIntent::GovernedMetric,
        };
        let result_schema_matches = result.as_ref().is_none_or(|result| {
            result
                .result
                .rows
                .iter()
                .all(|row| row.len() == result.result.columns.len())
        });
        let artifact_metadata_complete = state
            .schema_evidence
            .iter()
            .chain(state.metric_evidence.iter())
            .chain(state.freshness_evidence.iter())
            .chain(state.execution_plan.iter())
            .chain(state.preflight.iter())
            .chain(state.query_result.iter())
            .all(|artifact| {
                artifact.metadata.sensitivity != Sensitivity::Restricted
                    || (artifact.metadata.retention_policy.is_some()
                        && artifact.metadata.expires_at.is_some())
            });
        let expected_scope_hash = verification_hash(&self.config.data_scope)?;
        let expected_budget_hash = verification_hash(&self.config.query_budget)?;
        let preflight_scope_matches = preflight
            .as_ref()
            .is_some_and(|value| value.scope_hash == expected_scope_hash);
        let preflight_budget_matches = preflight
            .as_ref()
            .is_some_and(|value| value.budget_hash == expected_budget_hash);

        Ok(crate::workflow::query::VerificationInput {
            intent,
            policy_decision: state.policy_decision.clone().or_else(|| {
                (intent == ys_agent_core::QueryIntent::Metadata)
                    .then_some(ys_agent_core::PolicyDecision::Allow)
            }),
            data_query_permission_present: self
                .config
                .principal
                .has_capability(Capability::DataQuery),
            source_scope_matches: result.as_ref().is_none_or(|result| {
                result.compiled.source_id.as_str() == self.config.data_scope.source_id
            }),
            field_scope_matches: preflight_scope_matches
                || intent == ys_agent_core::QueryIntent::Metadata,
            query_budget_passed: preflight_budget_matches
                || intent == ys_agent_core::QueryIntent::Metadata,
            result_policy_passed: result.is_some()
                || intent == ys_agent_core::QueryIntent::Metadata,
            claims_reference_authorized_evidence: state.query_result.is_some()
                || !state.schema_evidence.is_empty(),
            artifact_metadata_complete,
            executed_result: state.query_result.clone(),
            requested_time_range,
            compiled_time_range,
            requested_metric,
            compiled_metric,
            relation_matches,
            result_schema_matches,
            freshness_evidence: state.freshness_evidence.clone(),
            freshness_state,
            current_data_required: crate::workflow::query::requires_current_freshness(
                &state.question,
            ),
            assumption_refs,
            ast_policy_passed: state.preflight.is_some(),
            semantic_status,
            observed_metadata: state.schema_evidence.clone(),
            invented_metric: false,
            invented_sql_result: false,
            invented_business_conclusion: false,
            result_truncated: result
                .as_ref()
                .is_some_and(|result| result.result.truncated),
            result_empty,
            result_all_null,
            unconfirmed_assumptions: !state.assumptions.is_empty(),
            sensitive_columns_redacted: result.as_ref().is_some_and(|result| {
                result
                    .result
                    .warning_codes
                    .iter()
                    .any(|code| code.contains("restricted"))
            }),
        })
    }

    async fn verify(
        &self,
        current: RunSnapshot,
        mut state: QueryWorkflowState,
    ) -> CoreResult<StepOutcome> {
        let report =
            crate::workflow::query::QueryVerifier.verify(self.verification_input(&state).await?);
        let bytes = serde_json::to_vec(&report).map_err(|error| {
            CoreError::validation("verification_serialization_failed", error.to_string())
        })?;
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: self.config.workspace_id,
                task_id: current.task_id,
                run_id: current.run_id,
                kind: ArtifactKind::VerificationReport,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            })
            .await?;
        state.verification_report = Some(ArtifactRef::new(metadata.clone()));
        state.warnings.extend(report.warnings.clone());
        if report.hard_failures.is_empty() {
            state.transition(QueryPhase::Package)?;
        }
        let status = if report.hard_failures.is_empty() {
            RunStatus::Running
        } else {
            RunStatus::Failed
        };
        let next = self.next_snapshot(
            &current,
            &state,
            status,
            None,
            current.primary_artifact_id,
            StepId::new(),
        )?;
        let mut events = vec![system_event(RunEventKind::ArtifactCreated {
            artifact: metadata.clone(),
        })];
        if !report.hard_failures.is_empty() {
            events.push(system_event(RunEventKind::RunFailed {
                code: "verification_failed".to_owned(),
                message: report.hard_failures.join(","),
            }));
        }
        self.append(&current, vec![metadata], events, &next).await?;
        Ok(if status == RunStatus::Failed {
            StepOutcome::Terminal {
                snapshot: next,
                accounting: StepAccounting::default(),
            }
        } else {
            StepOutcome::Continue {
                snapshot: next,
                accounting: StepAccounting::default(),
            }
        })
    }
}

impl Harness {
    async fn complete(
        &self,
        current: RunSnapshot,
        state: QueryWorkflowState,
        responded: PendingRunEvent,
        summary: String,
        tokens: u32,
        model_telemetry: TelemetryEvent,
    ) -> CoreResult<StepOutcome> {
        if state.phase != QueryPhase::ReadyToComplete || state.verification_report.is_none() {
            let failed = self.next_snapshot(
                &current,
                &state,
                RunStatus::Failed,
                None,
                current.primary_artifact_id,
                StepId::new(),
            )?;
            self.append(
                &current,
                vec![],
                vec![
                    responded,
                    system_event(RunEventKind::RunFailed {
                        code: "completion_gate_failed".to_owned(),
                        message: "Completion was proposed before verification".to_owned(),
                    }),
                ],
                &failed,
            )
            .await?;
            self.telemetry.emit_after_commit(model_telemetry).await;
            return Ok(StepOutcome::Terminal {
                snapshot: failed,
                accounting: StepAccounting {
                    model_calls: 1,
                    tool_calls: 0,
                    tokens,
                },
            });
        }

        let verification: crate::workflow::query::VerificationReport = self
            .read_json(state.verification_report.as_ref().expect("checked above"))
            .await?;
        let plan = match &state.execution_plan {
            Some(artifact) => Some(self.read_json::<ys_agent_core::QueryPlan>(artifact).await?),
            None => None,
        };
        let result = match &state.query_result {
            Some(artifact) => Some(self.read_json::<StoredResultEvidence>(artifact).await?),
            None => None,
        };
        let metric = match &state.metric_evidence {
            Some(artifact) => Some(
                self.read_json::<ys_agent_core::MetricDefinition>(artifact)
                    .await?,
            ),
            None => None,
        };
        let freshness_tool = match &state.freshness_evidence {
            Some(artifact) => Some(self.read_json::<FreshnessToolEvidence>(artifact).await?),
            None => None,
        };
        let freshness = freshness_tool.map(|value| ys_agent_core::FreshnessObservation {
            source_id: value.source_id,
            relation: value.relation,
            observed_at: value.observed_at,
            data_as_of: value.latest_data_at,
            lag_seconds: value.age_seconds,
        });
        let mut observed_relations = Vec::new();
        for evidence in &state.schema_evidence {
            let schema: StoredSchemaEvidence = self.read_json(evidence).await?;
            observed_relations.extend(schema.relations.into_iter().map(|relation| relation.name));
        }
        observed_relations.sort();
        observed_relations.dedup();
        let intent = state.intent.expect("verified state has intent");
        let source_id = plan
            .as_ref()
            .map(|plan| plan.source_id.clone())
            .or_else(|| freshness.as_ref().map(|value| value.source_id.clone()))
            .unwrap_or_else(|| {
                ys_agent_core::SourceId::new(self.config.data_scope.source_id.clone())
            });
        let source_relations = result
            .as_ref()
            .map(|result| result.compiled.source_relations.clone())
            .unwrap_or(observed_relations);
        let time_range = plan.as_ref().and_then(|plan| match &plan.execution {
            ys_agent_core::QueryExecutionPlan::Metric { start, end, .. } => {
                Some(ys_agent_core::TimeRange {
                    start: *start,
                    end: *end,
                })
            }
            ys_agent_core::QueryExecutionPlan::AdHoc { .. } => None,
        });
        let result_schema = crate::workflow::query::ResultSchema {
            columns: result
                .as_ref()
                .map(|result| {
                    result
                        .result
                        .columns
                        .iter()
                        .cloned()
                        .map(|name| crate::workflow::query::ResultColumn {
                            name,
                            data_type: None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        let artifact = crate::workflow::query::QueryArtifact::package(
            crate::workflow::query::QueryArtifactInput {
                question: state.question.clone(),
                intent,
                answer_summary: summary,
                metric: metric
                    .as_ref()
                    .map(|metric| crate::workflow::query::MetricReference {
                        id: metric.id.clone(),
                        version: metric.version.clone(),
                    }),
                semantic_status: result
                    .as_ref()
                    .map(|result| result.compiled.semantic_status)
                    .unwrap_or(ys_agent_core::SemanticStatus::Observed),
                source_id,
                source_relations,
                time_range,
                executed_sql: result.as_ref().map(|result| result.compiled.sql.clone()),
                parameters: result
                    .as_ref()
                    .map(|result| result.compiled.parameters.clone())
                    .unwrap_or_default(),
                result_schema,
                result_artifact: state.query_result.clone(),
                freshness,
                verification,
                assumptions: state.assumptions.clone(),
                sensitivity: Sensitivity::Internal,
                retention_policy: RetentionPolicy::Session,
                expires_at: None,
                generated_at: Utc::now(),
            },
        )?;
        let bytes = serde_json::to_vec(&artifact).map_err(|error| {
            CoreError::validation("query_artifact_serialization_failed", error.to_string())
        })?;
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: self.config.workspace_id,
                task_id: current.task_id,
                run_id: current.run_id,
                kind: ArtifactKind::Query,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: artifact.sensitivity,
                owner: None,
                retention_policy: Some(artifact.retention_policy.clone()),
                expires_at: artifact.expires_at,
                producer_step_id: None,
            })
            .await?;
        let completed = self.next_snapshot(
            &current,
            &state,
            RunStatus::Succeeded,
            None,
            Some(metadata.id),
            StepId::new(),
        )?;
        self.append(
            &current,
            vec![metadata.clone()],
            vec![
                responded,
                system_event(RunEventKind::ArtifactCreated {
                    artifact: metadata.clone(),
                }),
                system_event(RunEventKind::RunCompleted {
                    primary_artifact_id: metadata.id,
                }),
            ],
            &completed,
        )
        .await?;
        self.telemetry.emit_after_commit(model_telemetry).await;
        Ok(StepOutcome::Terminal {
            snapshot: completed,
            accounting: StepAccounting {
                model_calls: 1,
                tool_calls: 0,
                tokens,
            },
        })
    }
}
