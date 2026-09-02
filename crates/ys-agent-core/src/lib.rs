mod artifact;
mod command;
mod connector;
mod context;
mod error;
mod event;
mod identity;
mod ids;
mod metric;
mod model;
mod ports;
mod provider;
mod query;
mod run;
mod session;
mod task;
mod tool;

pub use artifact::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactKind, ArtifactMetadata,
    ArtifactMetadataBuilder, ArtifactRef, ExportFormat, PutArtifact, RetentionPolicy, Sensitivity,
};

pub use command::{CommandReceipt, CommandResultKind};

pub use connector::{
    CapabilityDescriptor, CellValue, CredentialReference, FreshnessObservation, ObservedColumn,
    ObservedRelation, ObservedSchema, QueryCostEstimate, QueryPreflight, QueryPreflightDecision,
    QueryRequest, QueryResult, SchemaKnowledgeKind, SourceId,
};

pub use context::{
    ContextEvidence, ContextManifest, ContextOmission, ContextSourceType, InstructionTrust,
};
pub use error::{CoreError, CoreResult};
pub use event::{
    EventActor, EventEnvelope, PendingRunEvent, PolicyDecision, RunEventKind, VersionedRunEvent,
};

pub use identity::{Capability, Principal};
pub use ids::{
    ArtifactId, CommandId, EventId, OperationId, PrincipalId, ProfileId, RunId, SessionId, StepId,
    TaskId, ToolCallId, ValidationId, WorkspaceId,
};

pub use metric::{MetricDefinition, MetricStatus};
pub use model::{
    AgentAction, AssistantToolCall, ModelCapabilities, ModelMessage, ModelRequest, ModelResponse,
    ModelRole, ModelUsage,
};

pub use ports::{
    ArtifactStore, CatalogReader, CreateRunCommand, CredentialMutationRepository, CredentialVault,
    FreshnessReader, MetricProvider, ModelDiscovery, ModelProvider, OAuthConnectionService,
    ProfileRevisionRepository, ProviderClientFactory, ProviderManagementApi,
    ProviderProfileRepository, QueryContextProvider, QueryPreflightReader,
    RunModelProviderResolver, RunProviderBindingRepository, RunProviderBindingSource,
    RuntimeCommandBatch, RuntimeStore, SqlQueryExecutor, Tool, ToolExecutionContext,
    ValidationActivationRepository,
};
pub use provider::{
    ActivateProfileRequest, ActivationConfirmation, ActivationPrecondition, ActiveProviderSlot,
    ActiveProviderSnapshot, ActiveProviderView, ActiveRevisionPrecondition, CompatibilityEvidence,
    CompatibilityEvidenceView, CredentialGeneration, CredentialKind, CredentialLease,
    CredentialMutation, CredentialMutationIntent, CredentialMutationOperation,
    CredentialMutationPhase, CredentialMutationRecord, CredentialMutationRequest,
    CredentialPointerCommit, CredentialProtectionStatus, CredentialViewStatus,
    DeleteProfileRequest, DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel,
    OAuthConnectionStatus, OAuthConnectionView, ParameterApplicability, ParameterValue,
    PersistedCompatibilityEvidence, PersistedCredentialMutationRecord, PersistedProfileRevision,
    ProfileDetail, ProfileHistory, ProfileName, ProfileRevision, ProfileState, ProfileSummary,
    ProtectedCredentialWrite, ProviderCatalogView, ProviderClientBinding,
    ProviderCredentialReference, ProviderDoctorView, ProviderErrorCategory, ProviderErrorCode,
    ProviderField, ProviderFingerprint, ProviderId, ProviderManagementError, ProviderModelId,
    ProviderParameterKey, ProviderParameters, ProviderProfile, ProviderProfileRevision,
    ProviderRemediation, ProviderResult, ProviderRetryability, ProviderSupportStatus,
    RemoteRevocationOutcome, ResolvedRunProvider, RevisionPrecondition, RunProviderBinding,
    SaveProfileRequest, SaveProfileRevision, SecretValue, ValidateProfileRequest, ValidationCommit,
    ValidationCommitPrecondition, ValidationDigest, ValidationInputs, ValidationVersions,
};

pub use query::{
    AllowedDataScope, ColumnPolicy, QueryBudget, QueryExecutionPlan, QueryIntent, QueryParameter,
    QueryPlan, SemanticStatus, TimeRange,
};
pub use run::{Run, RunSnapshot, RunStatus, WorkflowKind};
pub use session::Session;
pub use task::{Task, TaskStatus};
pub use tool::{
    CostClass, SideEffect, ToolCall, ToolFailure, ToolFailureCategory, ToolOutcome, ToolRisk,
    ToolSpec,
};
