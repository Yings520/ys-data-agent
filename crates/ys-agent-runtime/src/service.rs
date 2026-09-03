use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use ys_agent_core::{
    ActivateProfileRequest, ActiveProviderView, ArtifactAccessContext, ArtifactAccessPurpose,
    ArtifactId, ArtifactKind, ArtifactMetadata, ArtifactRef, ArtifactStore, CellValue, CommandId,
    CommandReceipt, CommandResultKind, CompatibilityEvidence, CompatibilityEvidenceView,
    ContextManifest, CoreError, CoreResult, CredentialGeneration, CredentialKind,
    CredentialMutationRequest, CredentialVault, CredentialViewStatus, DeleteProfileRequest,
    DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel, EventActor, EventEnvelope,
    ExportFormat, ListModelCandidatesRequest, ModelCandidateBatch, ModelMessage, ModelProvider,
    ModelRequest, ModelRole, ModelSelectionSnapshot, OAuthConnectionView, OperationId,
    PendingRunEvent, Principal, ProfileDetail, ProfileId, ProfileName, ProfileRevision,
    ProfileRevisionRepository, ProfileSummary, ProviderCatalogView, ProviderCredentialReference,
    ProviderDoctorView, ProviderErrorCode, ProviderField, ProviderId, ProviderManagementApi,
    ProviderManagementError, ProviderModelId, ProviderParameters, ProviderRemediation,
    ProviderResult, PutArtifact, QueryResult, RemoteRevocationOutcome, RetentionPolicy, Run,
    RunEventKind, RunId, RunProviderBinding, RunProviderBindingRepository,
    RunProviderBindingSource, RunSnapshot, RunStatus, RuntimeCommandBatch, RuntimeStore,
    Sensitivity, Session, SessionId, SwitchModelRequest, Task, TaskId, ValidateProfileRequest,
    ValidationVersions, WorkflowKind, WorkspaceId,
};

use crate::{
    coordinator::{CoordinationDecision, Coordinator, FutureWorkflow, RuleBasedCoordinator},
    doctor::{DoctorReport, DoctorRunner},
    export::ArtifactExportService,
};

const DEFAULT_EVENT_CAPACITY: usize = 64;
const ARTIFACT_PREVIEW_LIMIT: usize = 4_096;
const QUERY_RESULT_PREVIEW_ROW_LIMIT: usize = 100;
const QUERY_RESULT_PREVIEW_CELL_CHAR_LIMIT: usize = 256;
const QUERY_RESULT_PREVIEW_BYTE_LIMIT: usize = 64 * 1024;
const DEFAULT_ARTIFACT_RETENTION_DAYS: u32 = 7;
const ACTIVE_SNAPSHOT_RETRY_LIMIT: u8 = 2;

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
    Conversation {
        message: String,
    },
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
            Self::Conversation { .. } | Self::UnsupportedCapability { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    pub metadata: ArtifactMetadata,
    pub preview: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryResultPreviewView {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    persisted_row_count: usize,
    returned_row_count: usize,
    truncated: bool,
}

impl QueryResultPreviewView {
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    pub const fn persisted_row_count(&self) -> usize {
        self.persisted_row_count
    }

    pub const fn returned_row_count(&self) -> usize {
        self.returned_row_count
    }

    /// Indicates only limits applied while constructing this UI Preview. Query execution
    /// truncation remains a Query Artifact warning and is intentionally not folded into this bit.
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Deserialize)]
struct PersistedQueryResultEnvelope {
    result: QueryResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DatasourceDisplayState {
    Active { display_name: String },
    NotConfigured,
    Unavailable { reason: DatasourceUnavailableReason },
}

impl DatasourceDisplayState {
    pub fn active(display_name: impl Into<String>) -> CoreResult<Self> {
        let display_name = display_name.into();
        validate_display_label(&display_name, "datasource display name")?;
        Ok(Self::Active { display_name })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasourceUnavailableReason {
    ConnectionUnavailable,
    ValidationRequired,
    StatusUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryNonSuccessReason {
    Rejected,
    Failed,
    Cancelled,
    Unsupported,
    StatusUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum QueryDisplayState {
    Ready,
    Running,
    WaitingForInput,
    Completed,
    NonSuccess { reason: QueryNonSuccessReason },
}

impl From<RunStatus> for QueryDisplayState {
    fn from(status: RunStatus) -> Self {
        match status {
            RunStatus::Queued => Self::Ready,
            RunStatus::Running => Self::Running,
            RunStatus::WaitingForInput => Self::WaitingForInput,
            RunStatus::Succeeded => Self::Completed,
            RunStatus::Failed => Self::NonSuccess {
                reason: QueryNonSuccessReason::Failed,
            },
            RunStatus::Cancelled => Self::NonSuccess {
                reason: QueryNonSuccessReason::Cancelled,
            },
        }
    }
}

/// Atomic, non-sensitive inputs read from the authoritative Workspace, datasource, and Query
/// state owners. The service maps this snapshot into the public TUI read model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiDisplayContextInput {
    workspace_display_name: String,
    datasource: DatasourceDisplayState,
    read_only: bool,
    query_state: QueryDisplayState,
}

impl TuiDisplayContextInput {
    pub fn new(
        workspace_display_name: impl Into<String>,
        datasource: DatasourceDisplayState,
        read_only: bool,
        query_state: QueryDisplayState,
    ) -> CoreResult<Self> {
        let workspace_display_name = workspace_display_name.into();
        validate_display_label(&workspace_display_name, "workspace display name")?;
        Ok(Self {
            workspace_display_name,
            datasource,
            read_only,
            query_state,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TuiDisplayContext {
    workspace_display_name: String,
    datasource: DatasourceDisplayState,
    read_only: bool,
    query_state: QueryDisplayState,
}

impl TuiDisplayContext {
    pub fn workspace_display_name(&self) -> &str {
        &self.workspace_display_name
    }

    pub fn datasource(&self) -> &DatasourceDisplayState {
        &self.datasource
    }

    pub const fn read_only(&self) -> bool {
        self.read_only
    }

    pub const fn query_state(&self) -> QueryDisplayState {
        self.query_state
    }
}

impl From<TuiDisplayContextInput> for TuiDisplayContext {
    fn from(input: TuiDisplayContextInput) -> Self {
        Self {
            workspace_display_name: input.workspace_display_name,
            datasource: input.datasource,
            read_only: input.read_only,
            query_state: input.query_state,
        }
    }
}

#[async_trait]
pub trait TuiDisplayContextSource: Send + Sync {
    async fn load(&self) -> CoreResult<TuiDisplayContextInput>;
}

fn validate_display_label(value: &str, label: &str) -> CoreResult<()> {
    if value.trim().is_empty() || value != value.trim() || value.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "invalid_tui_display_label",
            format!("{label} must be non-empty, trimmed, and contain no control characters"),
        ));
    }
    Ok(())
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

struct UnconfiguredDoctor;

#[async_trait]
impl DoctorRunner for UnconfiguredDoctor {
    async fn run(&self) -> CoreResult<DoctorReport> {
        Err(CoreError::validation(
            "workspace_doctor_unconfigured",
            "Workspace Doctor is not configured",
        ))
    }
}

struct UnconfiguredExporter;

#[async_trait]
impl ArtifactExportService for UnconfiguredExporter {
    async fn export(
        &self,
        _command_id: CommandId,
        _artifact_id: &ArtifactId,
        _format: ExportFormat,
        _access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactMetadata> {
        Err(CoreError::validation(
            "artifact_export_unconfigured",
            "Artifact export is not configured",
        ))
    }
}

struct UnconfiguredTuiDisplayContextSource;

#[async_trait]
impl TuiDisplayContextSource for UnconfiguredTuiDisplayContextSource {
    async fn load(&self) -> CoreResult<TuiDisplayContextInput> {
        Err(CoreError::validation(
            "tui_display_context_unavailable",
            "TUI display context is not configured",
        ))
    }
}

#[async_trait]
pub trait AgentServiceApi: Send + Sync {
    async fn tui_display_context(&self) -> CoreResult<TuiDisplayContext> {
        Err(CoreError::validation(
            "tui_display_context_unavailable",
            "TUI display context is not configured",
        ))
    }

    async fn query_result_preview(
        &self,
        _artifact_id: &ArtifactId,
        _access: ArtifactAccessContext,
    ) -> CoreResult<QueryResultPreviewView> {
        Err(CoreError::validation(
            "query_result_preview_unavailable",
            "Query result Preview is not configured",
        ))
    }

    async fn create_session(
        &self,
        command_id: CommandId,
        principal: Principal,
    ) -> CoreResult<Session>;
    async fn create_task(&self, request: CreateTaskRequest) -> CoreResult<Task>;

    async fn send_message(&self, request: SendMessageRequest) -> CoreResult<ServiceReply>;

    async fn resume_task(&self, command_id: CommandId, task_id: &TaskId) -> CoreResult<RunId>;

    async fn answer_clarification(
        &self,
        command_id: CommandId,
        run_id: &RunId,
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
        run_id: &RunId,
        reason: String,
    ) -> CoreResult<()>;

    async fn doctor(&self) -> CoreResult<DoctorReport>;

    /// Returns the composed Provider-management boundary when this process has been bootstrapped
    /// for Provider management. TUI callers use only the default forwarding methods below.
    fn provider_management_api(&self) -> Option<&dyn ProviderManagementApi> {
        None
    }

    async fn provider_catalog(&self) -> ProviderResult<Vec<ProviderCatalogView>> {
        provider_api(self)?.catalog().await
    }

    async fn provider_model_selection_snapshot(&self) -> ProviderResult<ModelSelectionSnapshot> {
        provider_api(self)?.model_selection_snapshot().await
    }

    async fn provider_list_model_candidates(
        &self,
        request: ListModelCandidatesRequest,
    ) -> ProviderResult<ModelCandidateBatch> {
        provider_api(self)?.list_model_candidates(request).await
    }

    async fn provider_switch_model(
        &self,
        request: SwitchModelRequest,
    ) -> ProviderResult<ActiveProviderView> {
        provider_api(self)?.switch_model(request).await
    }

    async fn provider_list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        provider_api(self)?.list_profiles().await
    }

    async fn provider_active(&self) -> ProviderResult<Option<ActiveProviderView>> {
        provider_api(self)?.active_provider().await
    }

    async fn provider_load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
        provider_api(self)?.load_profile(profile_id).await
    }

    async fn provider_save_profile(
        &self,
        request: ys_agent_core::SaveProfileRequest,
    ) -> ProviderResult<ProfileDetail> {
        provider_api(self)?.save_profile(request).await
    }

    async fn provider_copy_profile(
        &self,
        source: ProfileId,
        name: ProfileName,
    ) -> ProviderResult<ProfileDetail> {
        provider_api(self)?.copy_profile(source, name).await
    }

    async fn provider_mutate_credential(
        &self,
        request: CredentialMutationRequest,
    ) -> ProviderResult<ProfileDetail> {
        provider_api(self)?.mutate_credential(request).await
    }

    async fn provider_delete_profile(&self, request: DeleteProfileRequest) -> ProviderResult<()> {
        provider_api(self)?.delete_profile(request).await
    }

    async fn provider_discover_models(
        &self,
        request: DiscoverModelsRequest,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        provider_api(self)?.discover_models(request).await
    }

    async fn provider_validate(
        &self,
        request: ValidateProfileRequest,
    ) -> ProviderResult<CompatibilityEvidenceView> {
        provider_api(self)?.validate_profile(request).await
    }

    async fn provider_activate(
        &self,
        request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderView> {
        provider_api(self)?.activate(request).await
    }

    async fn provider_activate_current(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<ActiveProviderView> {
        provider_api(self)?
            .activate_current(profile_id, operation_id)
            .await
    }

    async fn provider_credential_status(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<CredentialViewStatus> {
        provider_api(self)?.credential_status(profile_id).await
    }

    async fn provider_oauth_connection(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<OAuthConnectionView> {
        provider_api(self)?.oauth_connection(profile_id).await
    }

    async fn provider_doctor(&self) -> ProviderResult<ProviderDoctorView> {
        provider_api(self)?.doctor().await
    }

    async fn cancel_provider_operation(&self, operation_id: OperationId) -> ProviderResult<()> {
        provider_api(self)?.cancel_operation(operation_id).await
    }

    async fn provider_start_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        provider_api(self)?
            .start_oauth(profile_id, operation_id)
            .await
    }

    async fn provider_complete_oauth(
        &self,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        provider_api(self)?.complete_oauth(operation_id).await
    }

    async fn provider_refresh_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        provider_api(self)?
            .refresh_oauth(profile_id, operation_id)
            .await
    }

    async fn provider_reauthorize_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        provider_api(self)?
            .reauthorize_oauth(profile_id, operation_id)
            .await
    }

    async fn provider_logout_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome> {
        provider_api(self)?
            .logout_oauth(profile_id, operation_id)
            .await
    }

    async fn export_artifact(
        &self,
        command_id: CommandId,
        artifact_id: &ArtifactId,
        format: ExportFormat,
        access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactMetadata>;
}

fn provider_api<T: AgentServiceApi + ?Sized>(
    service: &T,
) -> ProviderResult<&dyn ProviderManagementApi> {
    service.provider_management_api().ok_or_else(|| {
        ProviderManagementError::new(
            ProviderErrorCode::Internal,
            Some(ProviderField::Provider),
            ProviderRemediation::ContactSupport,
        )
    })
}

pub struct InProcessAgentService {
    workspace_id: WorkspaceId,
    store: Arc<dyn RuntimeStore>,
    artifacts: Arc<dyn ArtifactStore>,
    scheduler: Arc<dyn RunScheduler>,
    doctor: Arc<dyn DoctorRunner>,
    exporter: Arc<dyn ArtifactExportService>,
    coordinator: RuleBasedCoordinator,
    event_sender: broadcast::Sender<ServiceEvent>,
    artifact_retention_days: u32,
    front_door: Option<FrontDoorAgent>,
    run_provider_bindings: Arc<dyn RunProviderBindingSource>,
    provider_management: Option<Arc<dyn ProviderManagementApi>>,
    tui_display_context_source: Arc<dyn TuiDisplayContextSource>,
}

#[derive(Debug, Default)]
pub struct UnavailableRunProviderBindingSource;

#[async_trait]
impl RunProviderBindingSource for UnavailableRunProviderBindingSource {
    async fn bind_new_run(&self, _run_id: RunId) -> ProviderResult<RunProviderBinding> {
        Err(no_active_profile_error())
    }
}

/// Production binding source. It reads the durable active Ready snapshot for every new Run and
/// verifies the exact credential generation in both durable metadata and the protected vault.
/// SQLite rejects a snapshot changed after this read; `InProcessAgentService` then retries the
/// complete read-and-create operation rather than mixing snapshots.
#[derive(Clone)]
pub struct ActiveRunProviderBindingSource {
    profiles: Arc<dyn ProfileRevisionRepository>,
    bindings: Arc<dyn RunProviderBindingRepository>,
    vault: Arc<dyn CredentialVault>,
}

impl ActiveRunProviderBindingSource {
    pub fn new(
        profiles: Arc<dyn ProfileRevisionRepository>,
        bindings: Arc<dyn RunProviderBindingRepository>,
        vault: Arc<dyn CredentialVault>,
    ) -> Self {
        Self {
            profiles,
            bindings,
            vault,
        }
    }
}

#[async_trait]
impl RunProviderBindingSource for ActiveRunProviderBindingSource {
    async fn bind_new_run(&self, run_id: RunId) -> ProviderResult<RunProviderBinding> {
        let active = self
            .profiles
            .active()
            .await?
            .ok_or_else(no_active_profile_error)?;
        let binding = RunProviderBinding::from_active(run_id, active).map_err(|_| {
            ProviderManagementError::new(
                ProviderErrorCode::ActivationPreconditionFailed,
                Some(ProviderField::Activation),
                ProviderRemediation::WaitForCurrentOperation,
            )
        })?;
        let reference = ProviderCredentialReference {
            profile_id: binding.profile_id(),
            generation: binding.credential_generation(),
        };
        ensure_usable_credential(
            self.bindings
                .credential_status(reference.generation)
                .await?,
        )?;
        ensure_usable_credential(self.vault.credential_status(reference).await?)?;
        Ok(binding)
    }
}

/// Deterministic non-network binding source for Fake/Replay test assemblies only.
#[derive(Clone)]
pub struct StaticRunProviderBindingSource {
    active: ys_agent_core::ActiveProviderSnapshot,
}

impl StaticRunProviderBindingSource {
    pub fn from_active(active: ys_agent_core::ActiveProviderSnapshot) -> Self {
        Self { active }
    }

    pub fn for_test() -> Self {
        let profile_id = ProfileId::new();
        let versions =
            ValidationVersions::new("test-catalog", "test-probe", "test-liter", "test-codec");
        let credential = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
            .expect("test credential generation is valid");
        let mut revision = ProfileRevision::draft(
            profile_id,
            1,
            ProviderId::DeepSeek,
            ProviderModelId::new(ProviderId::DeepSeek, "deepseek/test-model")
                .expect("test model prefix is valid"),
            ProviderParameters::default(),
            Some(credential),
        )
        .expect("test provider revision is valid");
        let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
        revision
            .accept_validation(evidence, versions)
            .expect("test validation evidence matches");
        Self::from_active(
            ys_agent_core::ActiveProviderSnapshot::from_ready(&revision, 1)
                .expect("test active snapshot is valid"),
        )
    }
}

#[async_trait]
impl RunProviderBindingSource for StaticRunProviderBindingSource {
    async fn bind_new_run(&self, run_id: RunId) -> ProviderResult<RunProviderBinding> {
        RunProviderBinding::from_active(run_id, self.active.clone()).map_err(|_| {
            ProviderManagementError::new(
                ProviderErrorCode::Internal,
                None,
                ProviderRemediation::Retry,
            )
        })
    }
}

#[derive(Clone)]
struct FrontDoorAgent {
    provider: Arc<dyn ModelProvider>,
    model_name: String,
}

const FRONT_DOOR_SYSTEM_PROMPT: &str = concat!(
    "You are Ys-da, the v0.2 front-door agent for a trustworthy query product. ",
    "Return one JSON object only, with no Markdown or prose. Do not call tools. ",
    "Classify the user message as exactly one of these actions:\n",
    r#"Chat, greetings, introductions, or questions about you rather than data: {"type":"respond","message":"<concise reply in the user's language>"}. "#,
    r#"A factual Query against configured data (governed metric, ad-hoc read, or metadata): {"type":"start_query"}. "#,
    r#"Work v0.2 cannot execute (analysis/root-cause, build/change, operate/deploy, ML data prep): {"type":"unsupported_capability","capability":"<analysis|build_change|operate|ml_data_prep>","message":"<safe refusal>"}. "#,
    "Never start a Query Run for chat. Never claim that data was queried in a respond message. ",
    "capability must be one of analysis, build_change, operate, ml_data_prep."
);

enum FrontDoorDecision {
    Respond(String),
    StartQuery,
    Unsupported {
        workflow: FutureWorkflow,
        message: String,
    },
}

struct ServiceOptions {
    event_capacity: usize,
    artifact_retention_days: u32,
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
        Self::with_event_capacity_and_dependencies(
            workspace_id,
            store,
            artifacts,
            scheduler,
            Arc::new(UnconfiguredDoctor),
            Arc::new(UnconfiguredExporter),
            ServiceOptions {
                event_capacity,
                artifact_retention_days: DEFAULT_ARTIFACT_RETENTION_DAYS,
            },
        )
    }

    pub fn with_dependencies(
        workspace_id: WorkspaceId,
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        scheduler: Arc<dyn RunScheduler>,
        doctor: Arc<dyn DoctorRunner>,
        exporter: Arc<dyn ArtifactExportService>,
    ) -> Self {
        Self::with_event_capacity_and_dependencies(
            workspace_id,
            store,
            artifacts,
            scheduler,
            doctor,
            exporter,
            ServiceOptions {
                event_capacity: DEFAULT_EVENT_CAPACITY,
                artifact_retention_days: DEFAULT_ARTIFACT_RETENTION_DAYS,
            },
        )
    }

    pub fn with_dependencies_and_retention(
        workspace_id: WorkspaceId,
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        scheduler: Arc<dyn RunScheduler>,
        doctor: Arc<dyn DoctorRunner>,
        exporter: Arc<dyn ArtifactExportService>,
        artifact_retention_days: u32,
    ) -> Self {
        Self::with_event_capacity_and_dependencies(
            workspace_id,
            store,
            artifacts,
            scheduler,
            doctor,
            exporter,
            ServiceOptions {
                event_capacity: DEFAULT_EVENT_CAPACITY,
                artifact_retention_days,
            },
        )
    }

    fn with_event_capacity_and_dependencies(
        workspace_id: WorkspaceId,
        store: Arc<dyn RuntimeStore>,
        artifacts: Arc<dyn ArtifactStore>,
        scheduler: Arc<dyn RunScheduler>,
        doctor: Arc<dyn DoctorRunner>,
        exporter: Arc<dyn ArtifactExportService>,
        options: ServiceOptions,
    ) -> Self {
        let (event_sender, _) = broadcast::channel(options.event_capacity.max(1));
        Self {
            workspace_id,
            store,
            artifacts,
            scheduler,
            doctor,
            exporter,
            coordinator: RuleBasedCoordinator,
            event_sender,
            artifact_retention_days: options.artifact_retention_days,
            front_door: None,
            run_provider_bindings: Arc::new(UnavailableRunProviderBindingSource),
            provider_management: None,
            tui_display_context_source: Arc::new(UnconfiguredTuiDisplayContextSource),
        }
    }

    pub fn with_front_door_agent(
        mut self,
        provider: Arc<dyn ModelProvider>,
        model_name: impl Into<String>,
    ) -> Self {
        self.front_door = Some(FrontDoorAgent {
            provider,
            model_name: model_name.into(),
        });
        self
    }

    pub fn with_conversation_model(
        self,
        provider: Arc<dyn ModelProvider>,
        model_name: impl Into<String>,
    ) -> Self {
        self.with_front_door_agent(provider, model_name)
    }

    pub fn with_run_provider_binding_source(
        mut self,
        source: Arc<dyn RunProviderBindingSource>,
    ) -> Self {
        self.run_provider_bindings = source;
        self
    }

    /// Attaches the single masked Provider-management façade. Composition roots supply this after
    /// repositories, Vault, adapters, and reconciliation are ready; TUI code never receives any
    /// of those implementation dependencies.
    pub fn with_provider_management_api(
        mut self,
        provider_management: Arc<dyn ProviderManagementApi>,
    ) -> Self {
        self.provider_management = Some(provider_management);
        self
    }

    pub fn with_tui_display_context_source(
        mut self,
        source: Arc<dyn TuiDisplayContextSource>,
    ) -> Self {
        self.tui_display_context_source = source;
        self
    }

    async fn classify_front_door(&self, input: &str) -> CoreResult<FrontDoorDecision> {
        let Some(front_door) = self.front_door.as_ref() else {
            return Ok(FrontDoorDecision::StartQuery);
        };
        let response = front_door
            .provider
            .complete(ModelRequest {
                model: front_door.model_name.clone(),
                messages: vec![
                    ModelMessage {
                        role: ModelRole::System,
                        content: FRONT_DOOR_SYSTEM_PROMPT.to_owned(),
                        tool_call_id: None,
                        name: None,
                        assistant_tool_call: None,
                    },
                    ModelMessage {
                        role: ModelRole::User,
                        content: input.to_owned(),
                        tool_call_id: None,
                        name: None,
                        assistant_tool_call: None,
                    },
                ],
                tools: Vec::new(),
                context_manifest: ContextManifest::empty(1_024),
                temperature: Some(0.0),
            })
            .await?;
        match response.action {
            ys_agent_core::AgentAction::Respond { message } => {
                let message = message.trim();
                if message.is_empty() || message.len() > 4_096 {
                    return Err(CoreError::validation(
                        "invalid_front_door_response",
                        "the conversational response must contain 1 to 4096 bytes",
                    ));
                }
                Ok(FrontDoorDecision::Respond(message.to_owned()))
            }
            ys_agent_core::AgentAction::StartQuery => Ok(FrontDoorDecision::StartQuery),
            ys_agent_core::AgentAction::UnsupportedCapability {
                capability,
                message,
            } => {
                let workflow = FutureWorkflow::from_capability(&capability).ok_or_else(|| {
                    CoreError::validation(
                        "invalid_front_door_response",
                        "unsupported_capability.capability is not a known v0.2 exclusion",
                    )
                })?;
                let message = message.trim();
                if message.is_empty() || message.len() > 4_096 {
                    return Err(CoreError::validation(
                        "invalid_front_door_response",
                        "the unsupported response must contain 1 to 4096 bytes",
                    ));
                }
                Ok(FrontDoorDecision::Unsupported {
                    workflow,
                    message: message.to_owned(),
                })
            }
            _ => Err(CoreError::validation(
                "invalid_front_door_response",
                "the front-door agent must return respond, start_query, or unsupported_capability",
            )),
        }
    }

    async fn start_query_run(
        &self,
        command_id: CommandId,
        fingerprint: String,
        session: &Session,
        goal: String,
        text: &str,
    ) -> CoreResult<ServiceReply> {
        let mut task = Task::new(session.workspace_id, session.principal_id, goal);
        task.start()?;
        let snapshot = running_snapshot(task.id, text)?;
        let proposed_run_id = snapshot.run_id;
        let stored = self
            .commit_run(
                command_id,
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

    async fn commit_front_door_reply(
        &self,
        fingerprint: String,
        receipt: CommandReceipt,
    ) -> CoreResult<CommandReceipt> {
        let command_id = receipt.command_id;
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt: receipt.clone(),
                new_session: None,
                new_task: None,
                create_run: None,
                new_artifact: None,
                pending_events: vec![],
                snapshot_update: None,
            })
            .await?;
        Ok(receipt)
    }

    async fn commit_unsupported(
        &self,
        command_id: CommandId,
        fingerprint: String,
        session_id: SessionId,
        task_id: Option<TaskId>,
        workflow: FutureWorkflow,
        message: String,
    ) -> CoreResult<ServiceReply> {
        self.commit_front_door_reply(
            fingerprint.clone(),
            CommandReceipt {
                command_id,
                command_fingerprint: fingerprint,
                result_kind: CommandResultKind::UnsupportedCapability,
                session_id: Some(session_id),
                task_id,
                run_id: None,
                artifact_id: None,
                message: Some(message.clone()),
                capability: Some(workflow.capability_name().to_owned()),
            },
        )
        .await?;
        Ok(ServiceReply::UnsupportedCapability {
            workflow,
            message,
            safe_evidence_refs: Vec::new(),
        })
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
            artifact_id: None,
            message: None,
            capability: None,
        };
        for attempt in 0..=ACTIVE_SNAPSHOT_RETRY_LIMIT {
            let binding = self
                .run_provider_bindings
                .bind_new_run(snapshot.run_id)
                .await
                .map_err(|error| CoreError::validation(error.code(), error.to_string()))?;
            let create_run =
                ys_agent_core::CreateRunCommand::new(snapshot.clone(), binding, events.clone())?;
            let result = self
                .store
                .commit_command(RuntimeCommandBatch {
                    command_id,
                    command_fingerprint: fingerprint.clone(),
                    receipt: receipt.clone(),
                    new_session: None,
                    new_task: task.clone(),
                    create_run: Some(create_run),
                    new_artifact: None,
                    pending_events: Vec::new(),
                    snapshot_update: None,
                })
                .await;
            match result {
                Err(error)
                    if attempt < ACTIVE_SNAPSHOT_RETRY_LIMIT && active_snapshot_changed(&error) =>
                {
                    continue;
                }
                result => return result,
            }
        }
        unreachable!("the bounded active snapshot retry loop always returns")
    }
}

fn no_active_profile_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::NoActiveProfile,
        Some(ProviderField::Activation),
        ProviderRemediation::EnterNoActiveProvider,
    )
}

fn ensure_usable_credential(status: CredentialViewStatus) -> ProviderResult<()> {
    match status {
        CredentialViewStatus::Saved => Ok(()),
        CredentialViewStatus::Missing => Err(ProviderManagementError::new(
            ProviderErrorCode::CredentialMissing,
            Some(ProviderField::Credential),
            ProviderRemediation::ConfigureCredentialStore,
        )),
        CredentialViewStatus::Expired | CredentialViewStatus::Revoked => {
            Err(ProviderManagementError::new(
                ProviderErrorCode::AuthenticationInvalid,
                Some(ProviderField::Credential),
                ProviderRemediation::Reauthorize,
            ))
        }
        CredentialViewStatus::ProtectionUnavailable
        | CredentialViewStatus::ReconciliationRequired => Err(ProviderManagementError::new(
            ProviderErrorCode::CredentialProtectionUnavailable,
            Some(ProviderField::Credential),
            ProviderRemediation::ConfigureCredentialStore,
        )),
    }
}

fn active_snapshot_changed(error: &CoreError) -> bool {
    error.code() == "active_provider_snapshot_changed"
}

#[async_trait]
impl AgentServiceApi for InProcessAgentService {
    async fn tui_display_context(&self) -> CoreResult<TuiDisplayContext> {
        self.tui_display_context_source.load().await.map(Into::into)
    }

    async fn query_result_preview(
        &self,
        artifact_id: &ArtifactId,
        access: ArtifactAccessContext,
    ) -> CoreResult<QueryResultPreviewView> {
        ensure_workspace(self.workspace_id, access.workspace_id)?;
        let metadata = self.store.load_artifact(artifact_id).await?;
        authorize_query_result_preview(&metadata, &access)?;
        let bytes = self
            .artifacts
            .get(&ArtifactRef::new(metadata), &access)
            .await?;
        let envelope: PersistedQueryResultEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| {
                CoreError::validation(
                    "malformed_query_result_artifact",
                    "persisted Query result does not match the supported schema",
                )
            })?;
        build_query_result_preview(envelope.result)
    }

    fn provider_management_api(&self) -> Option<&dyn ProviderManagementApi> {
        self.provider_management.as_deref()
    }

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
            artifact_id: None,
            message: None,
            capability: None,
        };

        let stored = self
            .store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: Some(session),
                new_task: None,
                create_run: None,
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
            artifact_id: None,
            message: None,
            capability: None,
        };
        let stored = self
            .store
            .commit_command(RuntimeCommandBatch {
                command_id: request.command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: Some(task),
                create_run: None,
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
                "focused_task_id": request.focused_task_id,
                "text": request.text,
            }),
        )?;
        let replayed_receipt = self
            .replayed_receipt(&request.command_id, &fingerprint)
            .await?;

        let session = self.store.load_session(&request.session_id).await?;
        ensure_workspace(self.workspace_id, session.workspace_id)?;
        let focused = self
            .load_focused_task(&session, request.focused_task_id)
            .await?;
        if let Some(receipt) = replayed_receipt {
            return reply_from_receipt(&receipt);
        }

        let decision = self
            .coordinator
            .route(&session, focused.as_ref(), &request.text)
            .await?;

        match decision {
            CoordinationDecision::FrontDoor { input } => {
                match self.classify_front_door(&input).await? {
                    FrontDoorDecision::Respond(message) => {
                        self.commit_front_door_reply(
                            fingerprint.clone(),
                            CommandReceipt {
                                command_id: request.command_id,
                                command_fingerprint: fingerprint,
                                result_kind: CommandResultKind::ConversationResponded,
                                session_id: Some(session.id),
                                task_id: focused.as_ref().map(|task| task.id),
                                run_id: None,
                                artifact_id: None,
                                message: Some(message.clone()),
                                capability: None,
                            },
                        )
                        .await?;
                        Ok(ServiceReply::Conversation { message })
                    }
                    FrontDoorDecision::StartQuery => {
                        self.start_query_run(
                            request.command_id,
                            fingerprint,
                            &session,
                            input,
                            &request.text,
                        )
                        .await
                    }
                    FrontDoorDecision::Unsupported { workflow, message } => {
                        self.commit_unsupported(
                            request.command_id,
                            fingerprint,
                            session.id,
                            focused.as_ref().map(|task| task.id),
                            workflow,
                            message,
                        )
                        .await
                    }
                }
            }
            CoordinationDecision::CreateNewTask { goal } => {
                self.start_query_run(
                    request.command_id,
                    fingerprint,
                    &session,
                    goal,
                    &request.text,
                )
                .await
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
                workflow, message, ..
            } => {
                self.commit_unsupported(
                    request.command_id,
                    fingerprint,
                    session.id,
                    focused.as_ref().map(|task| task.id),
                    workflow,
                    message,
                )
                .await
            }
        }
    }

    async fn resume_task(&self, command_id: CommandId, task_id: &TaskId) -> CoreResult<RunId> {
        let fingerprint = command_fingerprint("resume_task", json!({ "task_id": task_id }))?;
        if let Some(receipt) = self.replayed_receipt(&command_id, &fingerprint).await? {
            return required_run_id(&receipt);
        }

        let task = self.store.load_task(task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        if task.is_terminal() {
            return Err(CoreError::validation(
                "terminal_task",
                "a completed or cancelled task cannot be resumed",
            ));
        }
        let runs = self.store.list_runs_for_task(task_id).await?;
        let Some(previous) = runs.last() else {
            let snapshot = running_snapshot(task.id, &task.goal)?;
            let run_id = snapshot.run_id;
            self.commit_run(
                command_id,
                fingerprint,
                None,
                snapshot,
                vec![system_event(RunEventKind::RunStarted)],
            )
            .await?;
            self.scheduler.schedule(run_id).await?;
            self.event_publisher().notify(run_id, u64::MAX);
            return Ok(run_id);
        };

        if previous.status == RunStatus::Failed {
            let retry = RunSnapshot {
                run_id: RunId::new(),
                task_id: previous.task_id,
                workflow: previous.workflow,
                status: RunStatus::Running,
                attempt: previous.attempt + 1,
                retry_of_run_id: Some(previous.run_id),
                version: 1,
                workflow_state: previous.workflow_state.clone(),
                pending_wait_metadata: None,
                primary_artifact_id: None,
                last_completed_step_id: None,
            };
            self.commit_run(
                command_id,
                fingerprint,
                None,
                retry.clone(),
                vec![system_event(RunEventKind::RunStarted)],
            )
            .await?;
            self.scheduler.schedule(retry.run_id).await?;
            self.event_publisher().notify(retry.run_id, u64::MAX);
            return Ok(retry.run_id);
        }

        let recovery = crate::RecoveryManager::new(self.store.clone());
        let applied = recovery
            .apply(
                &previous.run_id,
                crate::RecoveryRequest {
                    explicit_resume: true,
                    high_cost_retry_confirmed: false,
                },
            )
            .await?;
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::RunResumed,
            session_id: None,
            task_id: Some(*task_id),
            run_id: Some(applied.snapshot.run_id),
            artifact_id: None,
            message: None,
            capability: None,
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                create_run: None,
                new_artifact: None,
                pending_events: Vec::new(),
                snapshot_update: None,
            })
            .await?;
        if applied.schedule {
            self.scheduler.schedule(applied.snapshot.run_id).await?;
        }
        self.event_publisher()
            .notify(applied.snapshot.run_id, u64::MAX);
        Ok(applied.snapshot.run_id)
    }

    async fn answer_clarification(
        &self,
        command_id: CommandId,
        run_id: &RunId,
        answer: String,
    ) -> CoreResult<()> {
        if answer.trim().is_empty() {
            return Err(CoreError::validation(
                "empty_clarification_answer",
                "Clarification answer cannot be empty",
            ));
        }
        let normalized_answer = answer.trim().to_ascii_lowercase();
        let fingerprint = command_fingerprint(
            "answer_clarification",
            json!({ "run_id": run_id, "answer": &answer }),
        )?;
        if self
            .replayed_receipt(&command_id, &fingerprint)
            .await?
            .is_some()
        {
            return Ok(());
        }

        let current = self.store.load_run(run_id).await?;
        if current.status != RunStatus::WaitingForInput {
            return Err(CoreError::validation(
                "run_not_waiting_for_input",
                "Clarification can answer only a WaitingForInput Run",
            ));
        }
        let pending = current.pending_wait_metadata.as_ref().ok_or_else(|| {
            CoreError::validation(
                "missing_wait_metadata",
                "Waiting Run has no pending clarification metadata",
            )
        })?;
        let clarification_id = pending
            .get("clarification_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::validation(
                    "missing_clarification_id",
                    "Wait metadata has no clarification ID",
                )
            })?
            .to_owned();
        let mut state = crate::workflow::query::QueryWorkflowState::from_snapshot(
            current.workflow_state.clone(),
        )?;
        let state_id = state
            .pending_clarification
            .as_ref()
            .map(|need| need.id.as_str());
        if state_id != Some(clarification_id.as_str()) {
            return Err(CoreError::validation(
                "clarification_id_mismatch",
                "Snapshot clarification ID does not match wait metadata",
            ));
        }

        if clarification_id.starts_with("confirm-high-cost-retry-") {
            if !matches!(normalized_answer.as_str(), "yes" | "confirm" | "retry") {
                return Err(CoreError::validation(
                    "high_cost_retry_not_confirmed",
                    "Answer must explicitly confirm the retry",
                ));
            }
            let next_call = {
                let previous = state.pending_recovery_call.as_ref().ok_or_else(|| {
                    CoreError::validation(
                        "pending_recovery_call_missing",
                        "High-cost confirmation has no pending Tool call",
                    )
                })?;
                crate::recovery::new_call_from(previous)
            };
            state.pending_recovery_call = Some(next_call);
            state.recovery_confirmation_granted = true;
        }

        let task = self.store.load_task(&current.task_id).await?;
        ensure_workspace(self.workspace_id, task.workspace_id)?;
        let restricted =
            pending.get("answer_sensitivity").and_then(Value::as_str) == Some("restricted");
        let (retention_policy, expires_at) =
            clarification_retention(restricted, self.artifact_retention_days, Utc::now());
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: task.workspace_id,
                task_id: task.id,
                run_id: *run_id,
                kind: ArtifactKind::ContextEvidence,
                media_type: "text/plain; charset=utf-8".to_owned(),
                bytes: answer.into_bytes(),
                sensitivity: if restricted {
                    Sensitivity::Restricted
                } else {
                    Sensitivity::Internal
                },
                owner: restricted.then_some(task.created_by),
                retention_policy,
                expires_at,
                producer_step_id: None,
            })
            .await?;
        state
            .clarification_evidence
            .push(ArtifactRef::new(metadata.clone()));
        if !state.answered_clarification_ids.contains(&clarification_id) {
            state
                .answered_clarification_ids
                .push(clarification_id.clone());
        }
        state.pending_clarification = None;
        let resumed = RunSnapshot {
            run_id: current.run_id,
            task_id: current.task_id,
            workflow: current.workflow,
            status: RunStatus::Running,
            attempt: current.attempt,
            retry_of_run_id: current.retry_of_run_id,
            version: current.version + 1,
            workflow_state: state.to_snapshot()?,
            pending_wait_metadata: None,
            primary_artifact_id: current.primary_artifact_id,
            last_completed_step_id: current.last_completed_step_id,
        };
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::ClarificationAnswered,
            session_id: None,
            task_id: Some(task.id),
            run_id: Some(*run_id),
            artifact_id: None,
            message: None,
            capability: None,
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                create_run: None,
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
        self.scheduler.schedule(*run_id).await?;
        self.event_publisher().notify(*run_id, u64::MAX);
        Ok(())
    }
    async fn cancel_run(
        &self,
        command_id: CommandId,
        run_id: &RunId,
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

        let current = self.store.load_run(run_id).await?;
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
            run_id: Some(*run_id),
            artifact_id: None,
            message: None,
            capability: None,
        };
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                create_run: None,
                new_artifact: None,
                pending_events: vec![system_event(RunEventKind::RunCancelled { reason })],
                snapshot_update: Some(cancelled),
            })
            .await?;
        self.event_publisher().notify(*run_id, u64::MAX);
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
        let full_safe_query = matches!(
            access.purpose,
            ArtifactAccessPurpose::RuntimeVerification | ArtifactAccessPurpose::TuiPreview
        ) && access.max_sensitivity <= Sensitivity::Internal
            && metadata.kind == ArtifactKind::Query
            && metadata.sensitivity <= Sensitivity::Internal;
        let preview_limit = if full_safe_query {
            bytes.len()
        } else {
            ARTIFACT_PREVIEW_LIMIT
        };
        let truncated = bytes.len() > preview_limit;
        let preview = bytes.into_iter().take(preview_limit).collect();
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

    async fn doctor(&self) -> CoreResult<DoctorReport> {
        self.doctor.run().await
    }

    async fn export_artifact(
        &self,
        command_id: CommandId,
        artifact_id: &ArtifactId,
        format: ExportFormat,
        access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactMetadata> {
        self.exporter
            .export(command_id, artifact_id, format, access)
            .await
    }
}

fn running_snapshot(task_id: TaskId, message: &str) -> CoreResult<RunSnapshot> {
    let mut run = Run::new(task_id, WorkflowKind::Query);
    run.start()?;
    let state = crate::workflow::query::QueryWorkflowState::new(message)?;
    Ok(run.snapshot(state.to_snapshot()?, None, None, None))
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
    let mut state = crate::workflow::query::QueryWorkflowState::new(message)?;
    state.pending_clarification = Some(crate::workflow::query::ClarificationNeed {
        id: clarification_id.to_owned(),
        question: question.to_owned(),
        reason: "clarification".to_owned(),
    });
    let mut snapshot = run.snapshot(
        state.to_snapshot()?,
        Some(json!({
            "clarification_id": clarification_id,
            "question": question,
            "reason": "clarification",
            "answer_sensitivity": "internal",
        })),
        None,
        None,
    );
    // The initial Running and Waiting Events commit atomically with this first Snapshot.
    snapshot.version = 1;
    Ok(snapshot)
}

fn system_event(kind: RunEventKind) -> PendingRunEvent {
    PendingRunEvent {
        actor: EventActor::System,
        kind,
    }
}

fn reply_from_receipt(receipt: &CommandReceipt) -> CoreResult<ServiceReply> {
    match receipt.result_kind {
        CommandResultKind::ConversationResponded => Ok(ServiceReply::Conversation {
            message: receipt
                .message
                .clone()
                .ok_or_else(|| malformed_receipt("message"))?,
        }),
        CommandResultKind::RunStarted | CommandResultKind::TaskCreated => {
            Ok(ServiceReply::RunScheduled {
                task_id: required_task_id(receipt)?,
                run_id: required_run_id(receipt)?,
            })
        }
        CommandResultKind::UnsupportedCapability | CommandResultKind::NoopReplay => {
            let workflow = receipt
                .capability
                .as_deref()
                .and_then(FutureWorkflow::from_capability)
                .unwrap_or(FutureWorkflow::Analysis);
            Ok(ServiceReply::UnsupportedCapability {
                workflow,
                message: receipt.message.clone().unwrap_or_else(|| {
                    format!(
                        "{} is not executable in v0.2; no Run was created.",
                        workflow.display_name()
                    )
                }),
                safe_evidence_refs: Vec::new(),
            })
        }
        CommandResultKind::ClarificationAnswered
        | CommandResultKind::RunResumed
        | CommandResultKind::RunCancelled
        | CommandResultKind::ArtifactExported
        | CommandResultKind::SessionCreated => Err(malformed_receipt("result_kind")),
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

fn authorize_query_result_preview(
    metadata: &ArtifactMetadata,
    access: &ArtifactAccessContext,
) -> CoreResult<()> {
    if metadata.kind != ArtifactKind::QueryResult {
        return Err(CoreError::validation(
            "artifact_kind_mismatch",
            "Query result Preview requires a QueryResult Artifact",
        ));
    }
    if metadata.workspace_id != access.workspace_id {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "artifact belongs to another workspace".to_owned(),
        });
    }
    if metadata
        .owner
        .is_some_and(|owner| owner != access.principal_id)
    {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "artifact belongs to another principal".to_owned(),
        });
    }
    if access.purpose != ArtifactAccessPurpose::TuiPreview
        || metadata.sensitivity > access.max_sensitivity
    {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "purpose or sensitivity is not allowed".to_owned(),
        });
    }
    Ok(())
}

fn build_query_result_preview(result: QueryResult) -> CoreResult<QueryResultPreviewView> {
    if result.row_count < result.rows.len() {
        return Err(CoreError::validation(
            "malformed_query_result_artifact",
            "persisted Query result row count is smaller than its stored rows",
        ));
    }
    for row in &result.rows {
        if row.len() != result.columns.len() {
            return Err(CoreError::validation(
                "malformed_query_result_artifact",
                "persisted Query result row width does not match its columns",
            ));
        }
    }

    let persisted_row_count = result.row_count;
    let mut truncated = result.rows.len() > QUERY_RESULT_PREVIEW_ROW_LIMIT;
    let mut columns = Vec::with_capacity(result.columns.len());
    for column in &result.columns {
        let (column, was_truncated) = truncate_display_text(column);
        truncated |= was_truncated;
        columns.push(column);
    }

    let mut preview = QueryResultPreviewView {
        columns,
        rows: Vec::new(),
        persisted_row_count,
        returned_row_count: 0,
        truncated,
    };

    while serialized_preview_len(&preview)? > QUERY_RESULT_PREVIEW_BYTE_LIMIT {
        if preview.columns.pop().is_none() {
            return Err(CoreError::validation(
                "query_result_preview_limit_too_small",
                "Query result Preview metadata exceeds its byte limit",
            ));
        }
        preview.truncated = true;
    }

    let visible_columns = preview.columns.len();
    if visible_columns < result.columns.len() {
        preview.truncated = true;
    }

    for row in result.rows.iter().take(QUERY_RESULT_PREVIEW_ROW_LIMIT) {
        let mut projected = Vec::with_capacity(visible_columns);
        for cell in row.iter().take(visible_columns) {
            let (value, was_truncated) = preview_cell(cell)?;
            preview.truncated |= was_truncated;
            projected.push(value);
        }

        preview.rows.push(projected);
        preview.returned_row_count = preview.rows.len();
        if serialized_preview_len(&preview)? > QUERY_RESULT_PREVIEW_BYTE_LIMIT {
            preview.rows.pop();
            preview.returned_row_count = preview.rows.len();
            preview.truncated = true;
            break;
        }
    }

    if preview.returned_row_count < result.rows.len() {
        preview.truncated = true;
    }
    Ok(preview)
}

fn preview_cell(cell: &CellValue) -> CoreResult<(Value, bool)> {
    match cell {
        CellValue::Null => Ok((Value::Null, false)),
        CellValue::Boolean(value) => Ok((Value::Bool(*value), false)),
        CellValue::Integer(value) => Ok((Value::Number((*value).into()), false)),
        CellValue::Real(value) => serde_json::Number::from_f64(*value)
            .map(|number| (Value::Number(number), false))
            .ok_or_else(|| {
                CoreError::validation(
                    "malformed_query_result_artifact",
                    "persisted Query result contains a non-finite number",
                )
            }),
        CellValue::Text(value) => {
            let (value, truncated) = truncate_display_text(value);
            Ok((Value::String(value), truncated))
        }
        CellValue::BlobSummary { bytes } => {
            Ok((Value::String(format!("[BLOB {bytes} bytes]")), false))
        }
    }
}

fn truncate_display_text(value: &str) -> (String, bool) {
    let mut chars = value.chars();
    let truncated = chars
        .clone()
        .nth(QUERY_RESULT_PREVIEW_CELL_CHAR_LIMIT)
        .is_some();
    let value = chars
        .by_ref()
        .take(QUERY_RESULT_PREVIEW_CELL_CHAR_LIMIT)
        .collect();
    (value, truncated)
}

fn serialized_preview_len(preview: &QueryResultPreviewView) -> CoreResult<usize> {
    serde_json::to_vec(preview)
        .map(|bytes| bytes.len())
        .map_err(|_| {
            CoreError::validation(
                "malformed_query_result_artifact",
                "Query result Preview could not be serialized safely",
            )
        })
}

fn clarification_retention(
    restricted: bool,
    artifact_retention_days: u32,
    now: chrono::DateTime<Utc>,
) -> (Option<RetentionPolicy>, Option<chrono::DateTime<Utc>>) {
    if restricted {
        (
            Some(RetentionPolicy::Days {
                days: artifact_retention_days,
            }),
            Some(now + TimeDelta::days(i64::from(artifact_retention_days))),
        )
    } else {
        (Some(RetentionPolicy::Session), None)
    }
}

pub(crate) fn command_fingerprint(operation: &str, payload: Value) -> CoreResult<String> {
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

#[cfg(test)]
mod retention_tests {
    use chrono::{TimeDelta, Utc};
    use ys_agent_core::RetentionPolicy;

    use super::clarification_retention;

    #[test]
    fn restricted_clarification_uses_the_configured_retention_days() {
        let now = Utc::now();

        let (policy, expires_at) = clarification_retention(true, 19, now);

        assert_eq!(policy, Some(RetentionPolicy::Days { days: 19 }));
        assert_eq!(expires_at, Some(now + TimeDelta::days(19)));
    }
}
