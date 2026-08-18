use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use ys_agent_core::{
    ArtifactAccessContext, ArtifactId, ArtifactKind, ArtifactMetadata, ArtifactRef, ArtifactStore,
    CommandId, CommandReceipt, CommandResultKind, CoreError, CoreResult, EventActor, EventEnvelope,
    PendingRunEvent, Principal, PutArtifact, Run, RunEventKind, RunId, RunSnapshot, RunStatus,
    RuntimeCommandBatch, RuntimeStore, Sensitivity, Session, SessionId, Task, TaskId, WorkflowKind,
    WorkspaceId,
};

use crate::coordinator::{CoordinationDecision, Coordinator, FutureWorkflow, RuleBasedCoordinator};

const DEFAULT_EVENT_CAPACITY: usize = 64;
const ARTIFACT_PREVIEW_LIMIT: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub command_id: CommandId,
    pub session_id: SessionId,
    pub goal: String,
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub command_id: CommandId,
    pub session_id: SessionId,
    pub focused_task_id: Option<TaskId>,
    pub text: String,
}

impl SendMessageRequest {
    pub fn new(command_id: CommandId, session_id: SessionId, text: impl Into<String>) -> Self {
        Self {
            command_id,
            session_id,
            focused_task_id: None,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceReply {
    RunScheduled {
        task_id: TaskId,
        run_id: RunId,
    },
    ClarificationRequired {
        task_id: TaskId,
        run_id: RunId,
        question: String,
    },
    UnsupportedCapability {
        workflow: FutureWorkflow,
        message: String,
        safe_evidence_refs: Vec<ArtifactId>,
    },
}

impl ServiceReply {
    pub fn run_id(&self) -> Option<RunId> {
        match self {
            Self::RunScheduled { run_id, .. } | Self::ClarificationRequired { run_id, .. } => {
                Some(*run_id)
            }
            Self::UnsupportedCapability { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    pub metadata: ArtifactMetadata,
    pub preview: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceEvent {
    pub run_id: RunId,
    pub through_sequence: u64,
}

#[derive(Clone)]
pub struct ServiceEventPublisher {
    sender: broadcast::Sender<ServiceEvent>,
}

impl ServiceEventPublisher {
    pub fn notify(&self, run_id: RunId, through_sequence: u64) {
        let _ = self.sender.send(ServiceEvent {
            run_id,
            through_sequence,
        });
    }
}

pub struct EventSubscription {
    store: Arc<dyn RuntimeStore>,
    run_id: RunId,
    cursor: u64,
    pending: VecDeque<EventEnvelope>,
    receiver: broadcast::Receiver<ServiceEvent>,
}

impl EventSubscription {
    pub async fn next(&mut self) -> CoreResult<EventEnvelope> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                self.cursor = event.sequence;
                return Ok(event);
            }

            match self.receiver.recv().await {
                Ok(notification) if notification.run_id == self.run_id => self.reload().await?,
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => self.reload().await?,
                Err(broadcast::error::RecvError::Closed) => {
                    self.reload().await?;
                    if self.pending.is_empty() {
                        return Err(CoreError::Storage {
                            message: "service event channel closed".to_owned(),
                        });
                    }
                }
            }
        }
    }

    pub fn last_sequence(&self) -> u64 {
        self.cursor
    }

    async fn reload(&mut self) -> CoreResult<()> {
        let events = self.store.load_events(&self.run_id, self.cursor).await?;
        self.pending.extend(events);
        Ok(())
    }
}

#[async_trait]
pub trait RunScheduler: Send + Sync {
    /// Implementations must deduplicate calls by RunId.
    async fn schedule(&self, run_id: RunId) -> CoreResult<()>;
}

#[derive(Debug, Default)]
pub struct NoopRunScheduler;

#[async_trait]
impl RunScheduler for NoopRunScheduler {
    async fn schedule(&self, _run_id: RunId) -> CoreResult<()> {
        Ok(())
    }
}

#[async_trait]
pub trait AgentServiceApi: Send + Sync {
    async fn create_session(
        &self,
        command_id: CommandId,
        principal: Principal,
    ) -> CoreResult<Session>;
    async fn create_task(&self, request: CreateTaskRequest) -> CoreResult<Task>;

    async fn send_message(&self, request: SendMessageRequest) -> CoreResult<ServiceReply>;

    async fn resume_task(&self, command_id: CommandId, task_id: TaskId) -> CoreResult<RunId>;

    async fn answer_clarification(
        &self,
        command_id: CommandId,
        run_id: RunId,
        answer: String,
    ) -> CoreResult<()>;

    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>>;

    async fn get_task(&self, task_id: &TaskId) -> CoreResult<Task>;

    async fn get_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot>;

    async fn get_artifact(
        &self,
        artifact_id: &ArtifactId,
        access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactView>;

    async fn subscribe_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<EventSubscription>;

    async fn cancel_run(
        &self,
        command_id: CommandId,
        run_id: RunId,
        reason: String,
    ) -> CoreResult<()>;
}

pub struct InProcessAgentService {
    workspace_id: WorkspaceId,
    store: Arc<dyn RuntimeStore>,
    artifacts: Arc<dyn ArtifactStore>,
    scheduler: Arc<dyn RunScheduler>,
    coordinator: RuleBasedCoordinator,
    event_sender: broadcast::Sender<ServiceEvent>,
}

impl InProcessAgentService {
    pub fn new(
        workspace_id: WorkspaceId,
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        scheduler: Arc<dyn RunScheduler>,
    ) -> Self {
        Self::with_event_capacity(
            workspace_id,
            store,
            artifacts,
            scheduler,
            DEFAULT_EVENT_CAPACITY,
        )
    }

    pub fn with_event_capacity(
        workspace_id: WorkspaceId,
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        scheduler: Arc<dyn RunScheduler>,
        event_capacity: usize,
    ) -> Self {
        let (event_sender, _) = broadcast::channel(event_capacity.max(1));
        Self {
            workspace_id,
            store,
            artifacts,
            scheduler,
            coordinator: RuleBasedCoordinator,
            event_sender,
        }
    }

    pub fn event_publisher(&self) -> ServiceEventPublisher {
        ServiceEventPublisher {
            sender: self.event_sender.clone(),
        }
    }

    async fn replayed_receipt(
        &self,
        command_id: &CommandId,
        fingerprint: &str,
    ) -> CoreResult<Option<CommandReceipt>> {
        let receipt = self.store.load_command(command_id).await?;
        if let Some(receipt) = &receipt
            && receipt.command_fingerprint != fingerprint
        {
            return Err(CoreError::IdempotencyConflict {
                command_id: command_id.to_string(),
            });
        }
        Ok(receipt)
    }

    async fn load_focused_task(
        &self,
        session: &Session,
        requested: Option<TaskId>,
    ) -> CoreResult<Option<Task>> {
        let Some(task_id) = requested.or(session.focused_task_id) else {
            return Ok(None);
        };
        self.store.load_task(&task_id).await.map(Some)
    }

    async fn commit_run(
        &self,
        command_id: CommandId,
        fingerprint: String,
        task: Option<Task>,
        snapshot: RunSnapshot,
        events: Vec<PendingRunEvent>,
    ) -> CoreResult<CommandReceipt> {
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::RunStarted,
            session_id: None,
            task_id: Some(snapshot.task_id),
            run_id: Some(snapshot.run_id),
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: task,
                new_run_snapshot: Some(snapshot),
                new_artifact: None,
                pending_events: events,
                snapshot_update: None,
            })
            .await
    }
}

#[async_trait]
impl AgentServiceApi for InProcessAgentService {
    async fn create_session(
        &self,
        command_id: CommandId,
        principal: Principal,
    ) -> CoreResult<Session> {
        let fingerprint = command_fingerprint(
            "create_session",
            json!({
                "workspace_id": self.workspace_id,
                "principal": principal,
            }),
        )?;
        if let Some(receipt) = self.replayed_receipt(&command_id, &fingerprint).await? {
            return self
                .store
                .load_session(&required_session_id(&receipt)?)
                .await;
        }

        let session = Session::new(self.workspace_id, principal.id);
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::SessionCreated,
            session_id: Some(session.id),
            task_id: None,
            run_id: None,
        };

        let stored = self
            .store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: Some(session),
                new_task: None,
                new_run_snapshot: None,
                new_artifact: None,
                pending_events: vec![],
                snapshot_update: None,
            })
            .await?;
        self.store
            .load_session(&required_session_id(&stored)?)
            .await
    }

    async fn create_task(&self, request: CreateTaskRequest) -> CoreResult<Task> {
        let fingerprint = command_fingerprint(
            "create_task",
            json!({
                "session_id": request.session_id,
                "goal": request.goal,
                "acceptance_criteria": request.acceptance_criteria,
            }),
        )?;
        if let Some(receipt) = self
            .replayed_receipt(&request.command_id, &fingerprint)
            .await?
        {
            return self.store.load_task(&required_task_id(&receipt)?).await;
        }

        let session = self.store.load_session(&request.session_id).await?;
        ensure_workspace(self.workspace_id, session.workspace_id)?;
        let task = Task::new(session.workspace_id, session.principal_id, request.goal)
            .with_acceptance_criteria(request.acceptance_criteria);
        let receipt = CommandReceipt {
            command_id: request.command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::TaskCreated,
            session_id: Some(session.id),
            task_id: Some(task.id),
            run_id: None,
        };
        let stored = self
            .store
            .commit_command(RuntimeCommandBatch {
                command_id: request.command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: Some(task),
                new_run_snapshot: None,
                new_artifact: None,
                pending_events: vec![],
                snapshot_update: None,
            })
            .await?;
        self.store.load_task(&required_task_id(&stored)?).await
    }

    async fn send_message(&self, request: SendMessageRequest) -> CoreResult<ServiceReply> {
        let fingerprint = command_fingerprint(
            "send_message",
            json!({
                "session_id": request.session_id,
                "text": request.text,
            }),
        )?;

        let session = self.store.load_session(&request.session_id).await?;
        ensure_workspace(self.workspace_id, session.workspace_id)?;
        let focused = self
            .load_focused_task(&session, request.focused_task_id)
            .await?;
        let decision = self
            .coordinator
            .route(&session, focused.as_ref(), &request.text)
            .await?;

        if let Some(receipt) = self
            .replayed_receipt(&request.command_id, &fingerprint)
            .await?
        {
            return reply_from_receipt(decision, &receipt);
        }

        match decision {
            CoordinationDecision::CreateNewTask { goal } => {
                let mut task = Task::new(session.workspace_id, session.principal_id, goal);
                task.start()?;
                let snapshot = running_snapshot(task.id, &request.text)?;
                let proposed_run_id = snapshot.run_id;
                let stored = self
                    .commit_run(
                        request.command_id,
                        fingerprint,
                        Some(task),
                        snapshot,
                        vec![system_event(RunEventKind::RunStarted)],
                    )
                    .await?;
                let run_id = required_run_id(&stored)?;
                if run_id == proposed_run_id {
                    self.scheduler.schedule(run_id).await?;
                    self.event_publisher().notify(run_id, 1);
                }
                Ok(ServiceReply::RunScheduled {
                    task_id: required_task_id(&stored)?,
                    run_id,
                })
            }

            CoordinationDecision::ContinueCurrentTask { task_id } => {
                let snapshot = running_snapshot(task_id, &request.text)?;
                let proposed_run_id = snapshot.run_id;
                let stored = self
                    .commit_run(
                        request.command_id,
                        fingerprint,
                        None,
                        snapshot,
                        vec![system_event(RunEventKind::RunStarted)],
                    )
                    .await?;
                let run_id = required_run_id(&stored)?;
                if run_id == proposed_run_id {
                    self.scheduler.schedule(run_id).await?;
                    self.event_publisher().notify(run_id, 1);
                }
                Ok(ServiceReply::RunScheduled { task_id, run_id })
            }

            CoordinationDecision::RequestClarification { question } => {
                let task_id = focused.as_ref().map(|task| task.id).ok_or_else(|| {
                    CoreError::validation(
                        "missing_focused_task",
                        "clarification requires a focused task",
                    )
                })?;
                let clarification_id = format!("clarification-{}", request.command_id);
                let snapshot =
                    waiting_snapshot(task_id, &request.text, &clarification_id, &question)?;
                let stored = self
                    .commit_run(
                        request.command_id,
                        fingerprint,
                        None,
                        snapshot,
                        vec![
                            system_event(RunEventKind::RunStarted),
                            system_event(RunEventKind::ClarificationRequested {
                                clarification_id,
                                question: question.clone(),
                            }),
                            system_event(RunEventKind::RunWaiting {
                                reason: "clarification".to_owned(),
                            }),
                        ],
                    )
                    .await?;
                let run_id = required_run_id(&stored)?;
                self.event_publisher().notify(run_id, 3);
                Ok(ServiceReply::ClarificationRequired {
                    task_id,
                    run_id,
                    question,
                })
            }

            CoordinationDecision::UnsupportedCapability {
                workflow,
                message,
                safe_evidence_refs,
            } => {
                let receipt = CommandReceipt {
                    command_id: request.command_id,
                    command_fingerprint: fingerprint.clone(),
                    result_kind: CommandResultKind::NoopReplay,
                    session_id: Some(session.id),
                    task_id: focused.as_ref().map(|task| task.id),
                    run_id: None,
                };
                self.store
                    .commit_command(RuntimeCommandBatch {
                        command_id: request.command_id,
                        command_fingerprint: fingerprint,
                        receipt,
                        new_session: None,
                        new_task: None,
                        new_run_snapshot: None,
                        new_artifact: None,
                        pending_events: vec![],
                        snapshot_update: None,
                    })
                    .await?;
                Ok(ServiceReply::UnsupportedCapability {
                    workflow,
                    message,
                    safe_evidence_refs,
                })
            }
        }
    }

    async fn resume_task(&self, command_id: CommandId, task_id: TaskId) -> CoreResult<RunId> {
        let fingerprint = command_fingerprint("resume_task", json!({ "task_id": task_id }))?;
        if let Some(receipt) = self.replayed_receipt(&command_id, &fingerprint).await? {
            return required_run_id(&receipt);
        }

        let task = self.store.load_task(&task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        if task.is_terminal() {
            return Err(CoreError::validation(
                "terminal_task",
                "a completed or cancelled task cannot be resumed",
            ));
        }
        let snapshot = running_snapshot(task.id, &task.goal)?;
        let proposed_run_id = snapshot.run_id;
        let stored = self
            .commit_run(
                command_id,
                fingerprint,
                None,
                snapshot,
                vec![system_event(RunEventKind::RunStarted)],
            )
            .await?;
        let run_id = required_run_id(&stored)?;
        if run_id == proposed_run_id {
            self.scheduler.schedule(run_id).await?;
            self.event_publisher().notify(run_id, 1);
        }
        Ok(run_id)
    }

    async fn answer_clarification(
        &self,
        command_id: CommandId,
        run_id: RunId,
        answer: String,
    ) -> CoreResult<()> {
        let fingerprint = command_fingerprint(
            "answer_clarification",
            json!({ "run_id": run_id, "answer": answer }),
        )?;
        if self
            .replayed_receipt(&command_id, &fingerprint)
            .await?
            .is_some()
        {
            return Ok(());
        }

        let current = self.store.load_run(&run_id).await?;
        if current.status != RunStatus::WaitingForInput {
            return Err(CoreError::validation(
                "run_not_waiting",
                "only a waiting Run accepts a clarification answer",
            ));
        }
        let task = self.store.load_task(&current.task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        let clarification_id = current
            .pending_wait_metadata
            .as_ref()
            .and_then(|value| value.get("clarification_id"))
            .and_then(Value::as_str)
            .unwrap_or("clarification")
            .to_owned();
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: task.workspace_id,
                task_id: task.id,
                run_id,
                kind: ArtifactKind::ContextEvidence,
                media_type: "text/plain; charset=utf-8".to_owned(),
                bytes: answer.into_bytes(),
                sensitivity: Sensitivity::Internal,
                owner: Some(task.created_by),
                retention_policy: None,
                expires_at: None,
                producer_step_id: None,
            })
            .await?;
        let mut resumed = current.clone();
        resumed.status = RunStatus::Running;
        resumed.version += 1;
        resumed.pending_wait_metadata = None;
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::ClarificationAnswered,
            session_id: None,
            task_id: Some(task.id),
            run_id: Some(run_id),
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                new_run_snapshot: None,
                new_artifact: Some(metadata.clone()),
                pending_events: vec![
                    system_event(RunEventKind::ClarificationAnswered {
                        clarification_id,
                        answer_artifact_id: metadata.id,
                    }),
                    system_event(RunEventKind::RunResumed),
                ],
                snapshot_update: Some(resumed),
            })
            .await?;
        self.scheduler.schedule(run_id).await?;
        self.event_publisher().notify(run_id, u64::MAX);
        Ok(())
    }
    async fn cancel_run(
        &self,
        command_id: CommandId,
        run_id: RunId,
        reason: String,
    ) -> CoreResult<()> {
        let fingerprint =
            command_fingerprint("cancel_run", json!({ "run_id": run_id, "reason": reason }))?;
        if self
            .replayed_receipt(&command_id, &fingerprint)
            .await?
            .is_some()
        {
            return Ok(());
        }

        let current = self.store.load_run(&run_id).await?;
        if matches!(
            current.status,
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Cancelled
        ) {
            return Err(CoreError::validation(
                "terminal_run",
                "a terminal Run cannot be cancelled again",
            ));
        }

        let mut cancelled = current.clone();
        cancelled.status = RunStatus::Cancelled;
        cancelled.version += 1;
        cancelled.pending_wait_metadata = None;
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::RunCancelled,
            session_id: None,
            task_id: Some(current.task_id),
            run_id: Some(run_id),
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                new_run_snapshot: None,
                new_artifact: None,
                pending_events: vec![system_event(RunEventKind::RunCancelled { reason })],
                snapshot_update: Some(cancelled),
            })
            .await?;
        self.event_publisher().notify(run_id, u64::MAX);
        Ok(())
    }

    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>> {
        ensure_workspace(self.workspace_id, *workspace_id)?;
        self.store.list_tasks(workspace_id).await
    }

    async fn get_task(&self, task_id: &TaskId) -> CoreResult<Task> {
        let task = self.store.load_task(task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        Ok(task)
    }

    async fn get_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot> {
        let run = self.store.load_run(run_id).await?;
        let task = self.store.load_task(&run.task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        Ok(run)
    }

    async fn get_artifact(
        &self,
        artifact_id: &ArtifactId,
        access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactView> {
        ensure_workspace(self.workspace_id, access.workspace_id)?;
        let metadata = self.store.load_artifact(artifact_id).await?;
        let bytes = self
            .artifacts
            .get(&ArtifactRef::new(metadata.clone()), &access)
            .await?;
        let truncated = bytes.len() > ARTIFACT_PREVIEW_LIMIT;
        let preview = bytes.into_iter().take(ARTIFACT_PREVIEW_LIMIT).collect();
        Ok(ArtifactView {
            metadata,
            preview,
            truncated,
        })
    }

    async fn subscribe_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<EventSubscription> {
        self.get_run(run_id).await?;
        let receiver = self.event_sender.subscribe();
        let pending = self.store.load_events(run_id, after_sequence).await?;
        Ok(EventSubscription {
            store: Arc::clone(&self.store),
            run_id: *run_id,
            cursor: after_sequence,
            pending: pending.into(),
            receiver,
        })
    }
}

fn running_snapshot(task_id: TaskId, message: &str) -> CoreResult<RunSnapshot> {
    let mut run = Run::new(task_id, WorkflowKind::Query);
    run.start()?;
    Ok(run.snapshot(json!({ "user_message": message }), None, None, None))
}

fn waiting_snapshot(
    task_id: TaskId,
    message: &str,
    clarification_id: &str,
    question: &str,
) -> CoreResult<RunSnapshot> {
    let mut run = Run::new(task_id, WorkflowKind::Query);
    run.start()?;
    run.wait_for_input(clarification_id)?;
    Ok(run.snapshot(
        json!({ "user_message": message }),
        Some(json!({
            "clarification_id": clarification_id,
            "question": question,
        })),
        None,
        None,
    ))
}

fn system_event(kind: RunEventKind) -> PendingRunEvent {
    PendingRunEvent {
        actor: EventActor::System,
        kind,
    }
}

fn reply_from_receipt(
    decision: CoordinationDecision,
    receipt: &CommandReceipt,
) -> CoreResult<ServiceReply> {
    match decision {
        CoordinationDecision::CreateNewTask { .. }
        | CoordinationDecision::ContinueCurrentTask { .. } => Ok(ServiceReply::RunScheduled {
            task_id: required_task_id(receipt)?,
            run_id: required_run_id(receipt)?,
        }),
        CoordinationDecision::RequestClarification { question } => {
            Ok(ServiceReply::ClarificationRequired {
                task_id: required_task_id(receipt)?,
                run_id: required_run_id(receipt)?,
                question,
            })
        }
        CoordinationDecision::UnsupportedCapability {
            workflow,
            message,
            safe_evidence_refs,
        } => Ok(ServiceReply::UnsupportedCapability {
            workflow,
            message,
            safe_evidence_refs,
        }),
    }
}

fn required_session_id(receipt: &CommandReceipt) -> CoreResult<SessionId> {
    receipt
        .session_id
        .ok_or_else(|| malformed_receipt("session_id"))
}

fn required_task_id(receipt: &CommandReceipt) -> CoreResult<TaskId> {
    receipt.task_id.ok_or_else(|| malformed_receipt("task_id"))
}

fn required_run_id(receipt: &CommandReceipt) -> CoreResult<RunId> {
    receipt.run_id.ok_or_else(|| malformed_receipt("run_id"))
}

fn malformed_receipt(field: &'static str) -> CoreError {
    CoreError::Storage {
        message: format!("stored command receipt is missing {field}"),
    }
}

fn ensure_workspace(expected: WorkspaceId, actual: WorkspaceId) -> CoreResult<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoreError::validation(
            "workspace_mismatch",
            "resource belongs to another workspace",
        ))
    }
}

fn command_fingerprint(operation: &str, payload: Value) -> CoreResult<String> {
    let canonical = canonicalize(json!({
        "operation": operation,
        "payload": payload,
    }));
    let bytes = serde_json::to_vec(&canonical).map_err(|error| CoreError::Storage {
        message: format!("cannot serialize command fingerprint: {error}"),
    })?;
    let digest = Sha256::digest(bytes);
    Ok(format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut sorted = Map::new();
            for (key, value) in entries {
                sorted.insert(key, canonicalize(value));
            }
            Value::Object(sorted)
        }
        scalar => scalar,
    }
}
