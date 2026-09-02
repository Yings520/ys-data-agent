use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use ys_agent_core::{
    CoreError, CoreResult, CostClass, EventEnvelope, RunEventKind, RunId, RunSnapshot, RunStatus,
    RuntimeStore, ToolCall, ToolCallId, ToolFailure,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryRequest {
    pub explicit_resume: bool,
    pub high_cost_retry_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryDecision {
    StartSameRun,
    ContinueSameRun,
    WaitForInput,
    RetryModelWithNewCallId,
    MarkToolIndeterminate {
        call: ToolCall,
        cost_class: CostClass,
    },
    WaitForExplicitResume {
        call: ToolCall,
    },
    RetryLowCostWithNewCall {
        previous: ToolCall,
    },
    ConfirmHighCostRetry {
        previous: ToolCall,
        cost_class: CostClass,
    },
    RetryConfirmedHighCostWithNewCall {
        previous: ToolCall,
        cost_class: CostClass,
    },
    ReconcileRemoteQuery {
        previous: ToolCall,
        remote_query_id: String,
    },
    ReturnTerminal(RunStatus),
    CreateRetryRun,
}

pub struct RecoveryManager {
    store: Arc<dyn RuntimeStore>,
}

impl RecoveryManager {
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self { store }
    }
}

#[derive(Debug, Clone)]
struct StartedTool {
    call: ToolCall,
    cost_class: CostClass,
}

#[derive(Debug, Default)]
struct ExternalFacts {
    latest_model_call: Option<(String, bool)>,
    proposed_tools: HashMap<ToolCallId, ToolCall>,
    started_tools: HashMap<ToolCallId, CostClass>,
    terminal_tools: HashSet<ToolCallId>,
    latest_indeterminate: Option<(ToolCallId, ToolFailure)>,
}

impl ExternalFacts {
    fn apply(&mut self, event: &EventEnvelope) -> CoreResult<()> {
        event.event.validate_supported()?;
        match &event.event.kind {
            RunEventKind::ModelRequested { model_call_id, .. } => {
                self.latest_model_call = Some((model_call_id.clone(), false));
            }
            RunEventKind::ModelResponded { model_call_id, .. } => {
                let Some((requested_id, responded)) = self.latest_model_call.as_mut() else {
                    return Err(CoreError::validation(
                        "model_response_without_request",
                        "ModelResponded has no preceding ModelRequested Event",
                    ));
                };
                if requested_id != model_call_id {
                    return Err(CoreError::validation(
                        "model_response_id_mismatch",
                        "ModelResponded does not match the latest request",
                    ));
                }
                *responded = true;
            }
            RunEventKind::ToolCallProposed { call } => {
                if self.proposed_tools.insert(call.id, call.clone()).is_some() {
                    return Err(CoreError::validation(
                        "duplicate_tool_call_proposal",
                        "ToolCallProposed repeats a ToolCall ID",
                    ));
                }
            }
            RunEventKind::ToolExecutionStarted {
                call_id,
                cost_class,
            } => {
                if self.started_tools.insert(*call_id, *cost_class).is_some() {
                    return Err(CoreError::validation(
                        "duplicate_tool_execution_started",
                        "ToolExecutionStarted repeats a ToolCall ID",
                    ));
                }
            }
            RunEventKind::ToolExecutionSucceeded { call_id, .. }
            | RunEventKind::ToolExecutionFailed { call_id, .. } => {
                if !self.terminal_tools.insert(*call_id) {
                    return Err(CoreError::validation(
                        "duplicate_tool_terminal_event",
                        "Tool call has more than one terminal Event",
                    ));
                }
            }
            RunEventKind::ToolExecutionIndeterminate { call_id, failure } => {
                if !self.terminal_tools.insert(*call_id) {
                    return Err(CoreError::validation(
                        "duplicate_tool_terminal_event",
                        "Tool call has more than one terminal Event",
                    ));
                }
                self.latest_indeterminate = Some((*call_id, failure.clone()));
            }
            RunEventKind::RunResumed => {
                self.latest_indeterminate = None;
            }
            RunEventKind::ProviderBound { .. }
            | RunEventKind::RunStarted
            | RunEventKind::StepStarted { .. }
            | RunEventKind::PolicyEvaluated { .. }
            | RunEventKind::ArtifactCreated { .. }
            | RunEventKind::ClarificationRequested { .. }
            | RunEventKind::ClarificationAnswered { .. }
            | RunEventKind::RunWaiting { .. }
            | RunEventKind::RunCompleted { .. }
            | RunEventKind::RunFailed { .. }
            | RunEventKind::RunCancelled { .. }
            | RunEventKind::RunStateProjected { .. } => {}
        }
        Ok(())
    }

    fn pending_model(&self) -> bool {
        self.latest_model_call
            .as_ref()
            .is_some_and(|(_, responded)| !responded)
    }

    fn validate(&self) -> CoreResult<()> {
        for call_id in self.started_tools.keys() {
            if !self.proposed_tools.contains_key(call_id) {
                return Err(CoreError::validation(
                    "started_tool_without_proposal",
                    "ToolExecutionStarted has no ToolCallProposed Event",
                ));
            }
        }
        for call_id in &self.terminal_tools {
            if !self.started_tools.contains_key(call_id) {
                return Err(CoreError::validation(
                    "terminal_tool_without_start",
                    "Tool terminal Event has no ToolExecutionStarted Event",
                ));
            }
        }
        Ok(())
    }

    fn started_without_terminal(&self) -> CoreResult<Option<StartedTool>> {
        let pending = self
            .started_tools
            .iter()
            .filter(|(call_id, _)| !self.terminal_tools.contains(call_id))
            .collect::<Vec<_>>();
        if pending.len() > 1 {
            return Err(CoreError::validation(
                "multiple_pending_tool_calls",
                "v0.2 permits at most one unfinished Tool call per Run step",
            ));
        }
        let Some((call_id, cost_class)) = pending.first() else {
            return Ok(None);
        };
        let call = self.proposed_tools.get(call_id).cloned().ok_or_else(|| {
            CoreError::validation(
                "started_tool_without_proposal",
                "ToolExecutionStarted has no ToolCallProposed Event",
            )
        })?;
        Ok(Some(StartedTool {
            call,
            cost_class: **cost_class,
        }))
    }

    fn latest_indeterminate(&self) -> Option<(ToolCall, ToolFailure)> {
        let (call_id, failure) = self.latest_indeterminate.as_ref()?;
        self.proposed_tools
            .get(call_id)
            .cloned()
            .map(|call| (call, failure.clone()))
    }
}

impl RecoveryManager {
    pub async fn assess(
        &self,
        run_id: &RunId,
        request: RecoveryRequest,
    ) -> CoreResult<RecoveryDecision> {
        let snapshot = self.reconstruct(run_id).await?;
        let events = self.store.load_events(run_id, 0).await?;
        validate_event_sequence(run_id, &events)?;
        let mut facts = ExternalFacts::default();
        for event in &events {
            facts
                .apply(event)
                .map_err(|error| CoreError::CorruptRunHistory {
                    run_id: run_id.to_string(),
                    reason: error.to_string(),
                })?;
        }
        facts
            .validate()
            .map_err(|error| CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: error.to_string(),
            })?;

        match snapshot.status {
            RunStatus::Queued => return Ok(RecoveryDecision::StartSameRun),
            RunStatus::WaitingForInput => return Ok(RecoveryDecision::WaitForInput),
            RunStatus::Succeeded | RunStatus::Cancelled => {
                return Ok(RecoveryDecision::ReturnTerminal(snapshot.status));
            }
            RunStatus::Failed => return Ok(RecoveryDecision::CreateRetryRun),
            RunStatus::Running => {}
        }

        if let Some(started) =
            facts
                .started_without_terminal()
                .map_err(|error| CoreError::CorruptRunHistory {
                    run_id: run_id.to_string(),
                    reason: error.to_string(),
                })?
        {
            return Ok(RecoveryDecision::MarkToolIndeterminate {
                call: started.call,
                cost_class: started.cost_class,
            });
        }
        if let Some((previous, failure)) = facts.latest_indeterminate() {
            if let Some(remote_query_id) = failure.remote_query_id {
                return Ok(RecoveryDecision::ReconcileRemoteQuery {
                    previous,
                    remote_query_id,
                });
            }
            return match failure.cost_class {
                CostClass::Low if request.explicit_resume => {
                    Ok(RecoveryDecision::RetryLowCostWithNewCall { previous })
                }
                CostClass::Low => Ok(RecoveryDecision::WaitForExplicitResume { call: previous }),
                CostClass::High | CostClass::Unknown if request.high_cost_retry_confirmed => {
                    Ok(RecoveryDecision::RetryConfirmedHighCostWithNewCall {
                        previous,
                        cost_class: failure.cost_class,
                    })
                }
                CostClass::High | CostClass::Unknown => {
                    Ok(RecoveryDecision::ConfirmHighCostRetry {
                        previous,
                        cost_class: failure.cost_class,
                    })
                }
            };
        }
        if facts.pending_model() {
            return Ok(RecoveryDecision::RetryModelWithNewCallId);
        }
        Ok(RecoveryDecision::ContinueSameRun)
    }

    pub async fn reconstruct(&self, run_id: &RunId) -> CoreResult<RunSnapshot> {
        let events = self.store.load_events(run_id, 0).await?;
        validate_event_sequence(run_id, &events)?;
        if events.is_empty() {
            return Err(CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: "Run has no Events".to_owned(),
            });
        }

        let expected_workspace_id = events[0].workspace_id;
        let expected_task_id = events[0].task_id;
        let mut projected: Option<RunSnapshot> = None;
        let mut last_projection_version: Option<u64> = None;
        for event in &events {
            if event.workspace_id != expected_workspace_id || event.task_id != expected_task_id {
                return Err(CoreError::CorruptRunHistory {
                    run_id: run_id.to_string(),
                    reason: "Event envelope identity changed inside one Run".to_owned(),
                });
            }
            let RunEventKind::RunStateProjected { snapshot } = &event.event.kind else {
                continue;
            };
            if snapshot.run_id != *run_id || snapshot.task_id != expected_task_id {
                return Err(CoreError::CorruptRunHistory {
                    run_id: run_id.to_string(),
                    reason: "Projection identity does not match Event envelope".to_owned(),
                });
            }
            if let Some(previous) = last_projection_version {
                if snapshot.version != previous + 1 {
                    return Err(CoreError::CorruptRunHistory {
                        run_id: run_id.to_string(),
                        reason: format!(
                            "Snapshot version jumped from {previous} to {}",
                            snapshot.version
                        ),
                    });
                }
            } else if snapshot.version != 1 {
                return Err(CoreError::CorruptRunHistory {
                    run_id: run_id.to_string(),
                    reason: format!(
                        "first projected Snapshot has version {}, expected 1",
                        snapshot.version
                    ),
                });
            }
            last_projection_version = Some(snapshot.version);
            projected = Some((**snapshot).clone());
        }
        let projected = projected.ok_or_else(|| CoreError::CorruptRunHistory {
            run_id: run_id.to_string(),
            reason: "Run history has no Snapshot projection".to_owned(),
        })?;
        if !matches!(
            events.last().map(|event| &event.event.kind),
            Some(RunEventKind::RunStateProjected { .. })
        ) {
            return Err(CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: "Final committed mutation does not end with a projection".to_owned(),
            });
        }

        match self.store.load_run(run_id).await {
            Ok(cached) if cached == projected => {}
            Ok(_) | Err(_) => self.store.replace_snapshot_cache(&projected).await?,
        }
        Ok(projected)
    }
}

#[derive(Debug, Clone)]
pub struct AppliedRecovery {
    pub snapshot: RunSnapshot,
    pub schedule: bool,
}

impl RecoveryManager {
    async fn append_state(
        &self,
        current: &RunSnapshot,
        state: &crate::workflow::query::QueryWorkflowState,
        status: RunStatus,
        pending_wait_metadata: Option<serde_json::Value>,
        events: Vec<ys_agent_core::PendingRunEvent>,
    ) -> CoreResult<RunSnapshot> {
        let next = RunSnapshot {
            run_id: current.run_id,
            task_id: current.task_id,
            workflow: current.workflow,
            status,
            attempt: current.attempt,
            retry_of_run_id: current.retry_of_run_id,
            version: current.version + 1,
            workflow_state: state.to_snapshot()?,
            pending_wait_metadata,
            primary_artifact_id: current.primary_artifact_id,
            last_completed_step_id: current.last_completed_step_id,
        };
        self.store
            .append(&current.run_id, current.version, vec![], events, &next)
            .await?;
        Ok(next)
    }

    pub async fn apply(
        &self,
        run_id: &RunId,
        request: RecoveryRequest,
    ) -> CoreResult<AppliedRecovery> {
        let current = self.reconstruct(run_id).await?;
        let mut state = crate::workflow::query::QueryWorkflowState::from_snapshot(
            current.workflow_state.clone(),
        )?;
        match self.assess(run_id, request).await? {
            RecoveryDecision::StartSameRun
            | RecoveryDecision::ContinueSameRun
            | RecoveryDecision::RetryModelWithNewCallId => Ok(AppliedRecovery {
                snapshot: current,
                schedule: true,
            }),
            RecoveryDecision::WaitForInput
            | RecoveryDecision::WaitForExplicitResume { .. }
            | RecoveryDecision::ReturnTerminal(_) => Ok(AppliedRecovery {
                snapshot: current,
                schedule: false,
            }),
            RecoveryDecision::CreateRetryRun => Err(CoreError::validation(
                "retry_run_required",
                "AgentService must create a new Run for a failed attempt",
            )),
            RecoveryDecision::MarkToolIndeterminate { call, cost_class } => {
                let failure = ToolFailure {
                    code: "interrupted_tool_execution".to_owned(),
                    category: ys_agent_core::ToolFailureCategory::Transport,
                    user_message: "Process stopped after Tool execution started".to_owned(),
                    retryable: false,
                    parameter_revision_allowed: false,
                    remote_query_id: None,
                    cost_class,
                };
                let indeterminate = recovery_event(RunEventKind::ToolExecutionIndeterminate {
                    call_id: call.id,
                    failure,
                });
                if !request.explicit_resume {
                    let next = self
                        .append_state(
                            &current,
                            &state,
                            RunStatus::Running,
                            None,
                            vec![indeterminate],
                        )
                        .await?;
                    return Ok(AppliedRecovery {
                        snapshot: next,
                        schedule: false,
                    });
                }

                match cost_class {
                    CostClass::Low => {
                        state.pending_recovery_call = Some(new_call_from(&call));
                        state.pending_recovery_cost_class = Some(CostClass::Low);
                        let next = self
                            .append_state(
                                &current,
                                &state,
                                RunStatus::Running,
                                None,
                                vec![indeterminate, recovery_event(RunEventKind::RunResumed)],
                            )
                            .await?;
                        Ok(AppliedRecovery {
                            snapshot: next,
                            schedule: true,
                        })
                    }
                    CostClass::High | CostClass::Unknown if request.high_cost_retry_confirmed => {
                        state.pending_recovery_call = Some(new_call_from(&call));
                        state.pending_recovery_cost_class = Some(cost_class);
                        state.recovery_confirmation_granted = true;
                        let next = self
                            .append_state(
                                &current,
                                &state,
                                RunStatus::Running,
                                None,
                                vec![indeterminate, recovery_event(RunEventKind::RunResumed)],
                            )
                            .await?;
                        Ok(AppliedRecovery {
                            snapshot: next,
                            schedule: true,
                        })
                    }
                    CostClass::High | CostClass::Unknown => {
                        let clarification_id = format!("confirm-high-cost-retry-{}", call.id);
                        state.pending_recovery_call = Some(call);
                        state.pending_recovery_cost_class = Some(cost_class);
                        state.pending_clarification =
                            Some(crate::workflow::query::ClarificationNeed {
                                id: clarification_id.clone(),
                                question: "Retry the interrupted high or unknown-cost read?"
                                    .to_owned(),
                                reason: "confirm_high_cost_retry".to_owned(),
                            });
                        let metadata = serde_json::json!({
                            "clarification_id": clarification_id.clone(),
                            "question": "Retry the interrupted high or unknown-cost read?",
                            "reason": "confirm_high_cost_retry",
                            "answer_sensitivity": "internal",
                        });
                        let next = self
                            .append_state(
                                &current,
                                &state,
                                RunStatus::WaitingForInput,
                                Some(metadata),
                                vec![
                                    indeterminate,
                                    recovery_event(RunEventKind::ClarificationRequested {
                                        clarification_id,
                                        question:
                                            "Retry the interrupted high or unknown-cost read?"
                                                .to_owned(),
                                    }),
                                    recovery_event(RunEventKind::RunWaiting {
                                        reason: "confirm_high_cost_retry".to_owned(),
                                    }),
                                ],
                            )
                            .await?;
                        Ok(AppliedRecovery {
                            snapshot: next,
                            schedule: false,
                        })
                    }
                }
            }
            RecoveryDecision::RetryLowCostWithNewCall { previous } => {
                state.pending_recovery_call = Some(new_call_from(&previous));
                state.pending_recovery_cost_class = Some(CostClass::Low);
                state.recovery_confirmation_granted = false;
                let next = self
                    .append_state(
                        &current,
                        &state,
                        RunStatus::Running,
                        None,
                        vec![recovery_event(RunEventKind::RunResumed)],
                    )
                    .await?;
                Ok(AppliedRecovery {
                    snapshot: next,
                    schedule: true,
                })
            }
            RecoveryDecision::RetryConfirmedHighCostWithNewCall {
                previous,
                cost_class,
            } => {
                state.pending_recovery_call = Some(new_call_from(&previous));
                state.pending_recovery_cost_class = Some(cost_class);
                state.recovery_confirmation_granted = true;
                let next = self
                    .append_state(
                        &current,
                        &state,
                        RunStatus::Running,
                        None,
                        vec![recovery_event(RunEventKind::RunResumed)],
                    )
                    .await?;
                Ok(AppliedRecovery {
                    snapshot: next,
                    schedule: true,
                })
            }
            RecoveryDecision::ConfirmHighCostRetry {
                previous,
                cost_class,
            } => {
                let clarification_id = format!("confirm-high-cost-retry-{}", previous.id);
                state.pending_recovery_call = Some(previous);
                state.pending_recovery_cost_class = Some(cost_class);
                state.pending_clarification = Some(crate::workflow::query::ClarificationNeed {
                    id: clarification_id.clone(),
                    question: "Retry the interrupted high or unknown-cost read?".to_owned(),
                    reason: "confirm_high_cost_retry".to_owned(),
                });
                let metadata = serde_json::json!({
                    "clarification_id": clarification_id.clone(),
                    "question": "Retry the interrupted high or unknown-cost read?",
                    "reason": "confirm_high_cost_retry",
                    "answer_sensitivity": "internal",
                });
                let next = self
                    .append_state(
                        &current,
                        &state,
                        RunStatus::WaitingForInput,
                        Some(metadata),
                        vec![
                            recovery_event(RunEventKind::ClarificationRequested {
                                clarification_id,
                                question: "Retry the interrupted high or unknown-cost read?"
                                    .to_owned(),
                            }),
                            recovery_event(RunEventKind::RunWaiting {
                                reason: "confirm_high_cost_retry".to_owned(),
                            }),
                        ],
                    )
                    .await?;
                Ok(AppliedRecovery {
                    snapshot: next,
                    schedule: false,
                })
            }
            RecoveryDecision::ReconcileRemoteQuery {
                previous,
                remote_query_id,
            } => {
                state.warnings.push(format!(
                    "reconcile_required:{}:{}",
                    previous.id, remote_query_id
                ));
                let wait = serde_json::json!({
                    "reason": "reconcile_remote_query",
                    "remote_query_id": remote_query_id,
                });
                let next = self
                    .append_state(
                        &current,
                        &state,
                        RunStatus::WaitingForInput,
                        Some(wait),
                        vec![recovery_event(RunEventKind::RunWaiting {
                            reason: "reconcile_remote_query".to_owned(),
                        })],
                    )
                    .await?;
                Ok(AppliedRecovery {
                    snapshot: next,
                    schedule: false,
                })
            }
        }
    }
}

fn recovery_event(kind: RunEventKind) -> ys_agent_core::PendingRunEvent {
    ys_agent_core::PendingRunEvent {
        actor: ys_agent_core::EventActor::System,
        kind,
    }
}

pub(crate) fn new_call_from(previous: &ToolCall) -> ToolCall {
    ToolCall {
        id: ToolCallId::new(),
        provider_call_id: None,
        name: previous.name.clone(),
        arguments: previous.arguments.clone(),
        version: previous.version.clone(),
    }
}

fn validate_event_sequence(run_id: &RunId, events: &[EventEnvelope]) -> CoreResult<()> {
    let mut expected = 1u64;
    for event in events {
        if event.run_id != *run_id || event.sequence != expected {
            return Err(CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: format!(
                    "expected sequence {expected}, found {} for Run {}",
                    event.sequence, event.run_id
                ),
            });
        }
        event
            .event
            .validate_supported()
            .map_err(|error| CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: error.to_string(),
            })?;
        expected = expected
            .checked_add(1)
            .ok_or_else(|| CoreError::CorruptRunHistory {
                run_id: run_id.to_string(),
                reason: "Event sequence overflow".to_owned(),
            })?;
    }
    Ok(())
}
