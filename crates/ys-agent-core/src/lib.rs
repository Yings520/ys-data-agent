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
    ArtifactId, CommandId, EventId, PrincipalId, RunId, SessionId, StepId, TaskId, ToolCallId,
    WorkspaceId,
};

pub use metric::{MetricDefinition, MetricStatus};
pub use model::{
    AgentAction, ModelCapabilities, ModelMessage, ModelRequest, ModelResponse, ModelRole,
    ModelUsage,
};

pub use ports::{
    ArtifactStore, CatalogReader, FreshnessReader, MetricProvider, ModelProvider,
    QueryContextProvider, QueryPreflightReader, RuntimeCommandBatch, RuntimeStore,
    SqlQueryExecutor, Tool, ToolExecutionContext,
};

pub use query::{AllowedDataScope, ColumnPolicy, QueryBudget, QueryIntent};
pub use run::{Run, RunSnapshot, RunStatus, WorkflowKind};
pub use session::Session;
pub use task::{Task, TaskStatus};
pub use tool::{
    CostClass, SideEffect, ToolCall, ToolFailure, ToolFailureCategory, ToolOutcome, ToolRisk,
    ToolSpec,
};
