use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    ActivateProfileRequest, ActiveProviderSnapshot, ActiveProviderView, AllowedDataScope,
    ArtifactAccessContext, ArtifactMetadata, ArtifactRef, CommandId, CommandReceipt,
    CompatibilityEvidenceView, ContextEvidence, CoreError, CoreResult, CredentialLease,
    CredentialMutationIntent, CredentialMutationRequest, CredentialPointerCommit,
    CredentialProtectionStatus, CredentialViewStatus, DeleteProfileRequest,
    DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel, EventEnvelope,
    FreshnessObservation, MetricDefinition, ModelCapabilities, ModelRequest, ModelResponse,
    OAuthConnectionView, ObservedSchema, OperationId, PendingRunEvent, Principal, ProfileDetail,
    ProfileId, ProfileRevision, ProfileSummary, ProtectedCredentialWrite, ProviderCatalogView,
    ProviderClientBinding, ProviderCredentialReference, ProviderDoctorView, ProviderResult,
    PutArtifact, QueryBudget, QueryPreflight, QueryRequest, QueryResult, RemoteRevocationOutcome,
    ResolvedRunProvider, RunId, RunProviderBinding, RunSnapshot, SaveProfileRequest,
    SaveProfileRevision, Session, SessionId, SourceId, Task, TaskId, ToolCallId, ToolOutcome,
    ToolSpec, ValidateProfileRequest, ValidationCommit, WorkspaceId,
};

/// A production Run is only created from this complete, immutable Provider snapshot. The Store
/// integration added by the Provider-management feature persists this command atomically with the
/// Run and its initial lifecycle events.
#[derive(Debug, Clone)]
pub struct CreateRunCommand {
    snapshot: RunSnapshot,
    provider_binding: RunProviderBinding,
    initial_events: Vec<PendingRunEvent>,
}

/// Resolves the immutable Provider snapshot for one newly-created production Run. Implementors
/// must fail closed when there is no active Ready Profile rather than selecting another Provider.
#[async_trait]
pub trait RunProviderBindingSource: Send + Sync {
    async fn bind_new_run(&self, run_id: RunId) -> ProviderResult<RunProviderBinding>;
}

impl CreateRunCommand {
    pub fn new(
        snapshot: RunSnapshot,
        provider_binding: RunProviderBinding,
        initial_events: Vec<PendingRunEvent>,
    ) -> CoreResult<Self> {
        if snapshot.run_id != provider_binding.run_id() {
            return Err(CoreError::validation(
                "run_provider_binding_mismatch",
                "a Run creation binding must belong to the Run being created",
            ));
        }
        if initial_events
            .iter()
            .any(|event| matches!(event.kind, crate::RunEventKind::ProviderBound { .. }))
        {
            return Err(CoreError::validation(
                "duplicate_provider_bound_event",
                "ProviderBound is generated exactly once by the Run creation command",
            ));
        }

        let mut events = Vec::with_capacity(initial_events.len() + 1);
        events.push(PendingRunEvent {
            actor: crate::EventActor::System,
            kind: crate::RunEventKind::ProviderBound {
                fingerprint: provider_binding.fingerprint().clone(),
            },
        });
        events.extend(initial_events);

        Ok(Self {
            snapshot,
            provider_binding,
            initial_events: events,
        })
    }

    pub fn snapshot(&self) -> &RunSnapshot {
        &self.snapshot
    }

    pub fn provider_binding(&self) -> &RunProviderBinding {
        &self.provider_binding
    }

    pub fn initial_events(&self) -> &[PendingRunEvent] {
        &self.initial_events
    }
}

/// Atomic control-plane mutation unit for RuntimeStore::commit_command.
#[derive(Debug, Clone)]
pub struct RuntimeCommandBatch {
    pub command_id: CommandId,
    pub command_fingerprint: String,
    pub receipt: CommandReceipt,
    pub new_session: Option<Session>,
    pub new_task: Option<Task>,
    /// A production Run cannot be represented here without a complete Provider binding. Task 2.4
    /// persists it in the same transaction as the Run.
    pub create_run: Option<CreateRunCommand>,
    pub new_artifact: Option<ArtifactMetadata>,
    pub pending_events: Vec<PendingRunEvent>,
    pub snapshot_update: Option<RunSnapshot>,
}

#[async_trait]
pub trait RuntimeStore: Send + Sync {
    async fn load_command(&self, command_id: &CommandId) -> CoreResult<Option<CommandReceipt>>;

    async fn commit_command(&self, batch: RuntimeCommandBatch) -> CoreResult<CommandReceipt>;

    async fn load_session(&self, session_id: &SessionId) -> CoreResult<Session>;

    async fn load_task(&self, task_id: &TaskId) -> CoreResult<Task>;

    async fn load_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot>;

    async fn list_runs_for_task(&self, task_id: &TaskId) -> CoreResult<Vec<RunSnapshot>>;

    async fn load_artifact(&self, artifact_id: &crate::ArtifactId) -> CoreResult<ArtifactMetadata>;

    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>>;

    async fn load_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<Vec<EventEnvelope>>;

    async fn replace_snapshot_cache(&self, snapshot: &RunSnapshot) -> CoreResult<()>;

    async fn append(
        &self,
        run_id: &RunId,
        expected_version: u64,
        artifacts: Vec<ArtifactMetadata>,
        events: Vec<PendingRunEvent>,
        snapshot: &RunSnapshot,
    ) -> CoreResult<()>;
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, request: PutArtifact) -> CoreResult<ArtifactMetadata>;

    async fn get(
        &self,
        artifact: &ArtifactRef,
        access: &ArtifactAccessContext,
    ) -> CoreResult<Vec<u8>>;
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse>;
}

/// Runtime-owned governance passed to every v0.2 query Tool.
#[derive(Debug, Clone)]
pub struct ToolExecutionContext {
    pub call_id: ToolCallId,
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub principal: Principal,
    pub query_budget: QueryBudget,
    pub data_scope: AllowedDataScope,
    pub confirmation_granted: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    async fn execute(
        &self,
        context: &ToolExecutionContext,
        arguments: Value,
    ) -> CoreResult<ToolOutcome>;
}

#[async_trait]
pub trait CatalogReader: Send + Sync {
    async fn observe_schema(&self, source_id: &SourceId) -> CoreResult<ObservedSchema>;
}

#[async_trait]
pub trait SqlQueryExecutor: Send + Sync {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult>;
}

#[async_trait]
pub trait QueryPreflightReader: Send + Sync {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight>;
}

#[async_trait]
pub trait FreshnessReader: Send + Sync {
    async fn read_freshness(
        &self,
        source_id: &SourceId,
        relation: &str,
        time_column: &str,
    ) -> CoreResult<FreshnessObservation>;
}

#[async_trait]
pub trait MetricProvider: Send + Sync {
    async fn get_metric(&self, metric_id: &str) -> CoreResult<Option<MetricDefinition>>;

    async fn list_active_metrics(&self) -> CoreResult<Vec<MetricDefinition>>;
}

#[async_trait]
pub trait QueryContextProvider: Send + Sync {
    async fn load_evidence(&self, query: &str) -> CoreResult<Vec<ContextEvidence>>;
}

/// Durable Provider profile state. Every mutation includes an explicit compare-and-swap
/// precondition so a late operation cannot replace a newer revision or active selection.
#[async_trait]
pub trait ProviderProfileRepository: Send + Sync {
    async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>>;

    async fn load_revision(
        &self,
        profile_id: ProfileId,
        revision: u64,
    ) -> ProviderResult<ProfileRevision>;

    async fn save_revision(&self, request: SaveProfileRevision) -> ProviderResult<ProfileRevision>;

    async fn save_validation(&self, commit: ValidationCommit) -> ProviderResult<ProfileRevision>;

    async fn activate(
        &self,
        request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderSnapshot>;

    async fn active(&self) -> ProviderResult<Option<ActiveProviderSnapshot>>;

    async fn begin_credential_mutation(
        &self,
        intent: CredentialMutationIntent,
    ) -> ProviderResult<OperationId>;

    async fn commit_credential_pointer(
        &self,
        commit: CredentialPointerCommit,
    ) -> ProviderResult<()>;

    async fn pending_credential_mutations(&self) -> ProviderResult<Vec<CredentialMutationIntent>>;

    async fn delete_profile(&self, request: DeleteProfileRequest) -> ProviderResult<()>;
}

/// Immutable binding persistence is separate from Profile management so the runtime can resolve
/// a Run without reading the mutable active pointer.
#[async_trait]
pub trait RunProviderBindingRepository: Send + Sync {
    async fn load_run_binding(&self, run_id: RunId) -> ProviderResult<crate::RunProviderBinding>;
}

/// OS-backed credential boundary. The only data that reaches a caller is a short-lived opaque
/// lease; locator and secret text never enter Profile, TUI, Doctor, or runtime view types.
#[async_trait]
pub trait CredentialVault: Send + Sync {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus>;

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus>;

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()>;

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease>;

    async fn delete_generation(&self, reference: ProviderCredentialReference)
    -> ProviderResult<()>;
}

/// Factory contract shared by production adapters and deterministic Fake/Replay implementations.
/// Provider-specific client, HTTP, and `liter-llm` types must not appear in this signature.
#[async_trait]
pub trait ProviderClientFactory: Send + Sync {
    async fn build(
        &self,
        binding: ProviderClientBinding,
        credential: CredentialLease,
    ) -> ProviderResult<Arc<dyn ModelProvider>>;
}

/// Discovery is independent from client construction so discovery failures remain recoverable and
/// a caller can retain a Draft for manual model entry.
#[async_trait]
pub trait ModelDiscovery: Send + Sync {
    async fn discover(
        &self,
        request: DiscoverModelsRequest,
        credential: CredentialLease,
    ) -> ProviderResult<Vec<DiscoveredModel>>;
}

#[async_trait]
pub trait RunModelProviderResolver: Send + Sync {
    async fn resolve(&self, run_id: RunId) -> ProviderResult<ResolvedRunProvider>;
}

#[async_trait]
pub trait OAuthConnectionService: Send + Sync {
    /// Returns only the masked OAuth connection state and its safe remediation.
    async fn view(&self, profile_id: ProfileId) -> ProviderResult<OAuthConnectionView>;

    async fn start(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView>;

    async fn complete(&self, operation_id: OperationId) -> ProviderResult<OAuthConnectionView>;

    async fn refresh(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView>;

    async fn reauthorize(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView>;

    async fn logout(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome>;
}

/// The service-facing, vendor-neutral capability boundary. TUI and Doctor depend on these masked
/// types only; they never receive a repository, Vault, OAuth transport, or HTTP client.
#[async_trait]
pub trait ProviderManagementApi: Send + Sync {
    async fn catalog(&self) -> ProviderResult<Vec<ProviderCatalogView>>;

    async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>>;

    async fn load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail>;

    async fn save_profile(&self, request: SaveProfileRequest) -> ProviderResult<ProfileDetail>;

    /// Copies non-sensitive, applicable configuration into a new Draft without sharing a
    /// Credential or validation result with the source Profile.
    async fn copy_profile(
        &self,
        source: ProfileId,
        name: crate::ProfileName,
    ) -> ProviderResult<ProfileDetail>;

    /// Replaces or deletes one Profile-exclusive Credential. Deletion has no secret payload.
    async fn mutate_credential(
        &self,
        request: CredentialMutationRequest,
    ) -> ProviderResult<ProfileDetail>;

    async fn delete_profile(&self, request: DeleteProfileRequest) -> ProviderResult<()>;

    async fn discover_models(
        &self,
        request: DiscoverModelsRequest,
    ) -> ProviderResult<Vec<DiscoveredModel>>;

    async fn validate_profile(
        &self,
        request: ValidateProfileRequest,
    ) -> ProviderResult<CompatibilityEvidenceView>;

    async fn activate(&self, request: ActivateProfileRequest)
    -> ProviderResult<ActiveProviderView>;

    async fn credential_status(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<CredentialViewStatus>;

    /// Returns the masked OAuth connection state for one Profile.
    async fn oauth_connection(&self, profile_id: ProfileId) -> ProviderResult<OAuthConnectionView>;

    /// Returns the Provider-related Doctor findings without accessing Credential contents.
    async fn doctor(&self) -> ProviderResult<ProviderDoctorView>;

    async fn cancel_operation(&self, operation_id: OperationId) -> ProviderResult<()>;

    async fn start_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView>;

    async fn complete_oauth(
        &self,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView>;

    async fn refresh_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView>;

    async fn reauthorize_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView>;

    async fn logout_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome>;
}
