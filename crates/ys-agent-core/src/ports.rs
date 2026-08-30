use async_trait::async_trait;
use serde_json::Value;

use crate::{
    AllowedDataScope, ArtifactAccessContext, ArtifactMetadata, ArtifactRef, CommandId,
    CommandReceipt, ContextEvidence, CoreResult, EventEnvelope, FreshnessObservation,
    MetricDefinition, ModelCapabilities, ModelRequest, ModelResponse, ObservedSchema,
    PendingRunEvent, Principal, PutArtifact, QueryBudget, QueryPreflight, QueryRequest,
    QueryResult, RunId, RunSnapshot, Session, SessionId, SourceId, Task, TaskId, ToolCallId,
    ToolOutcome, ToolSpec, WorkspaceId,
};

/// Atomic control-plane mutation unit for RuntimeStore::commit_command.
#[derive(Debug, Clone)]
pub struct RuntimeCommandBatch {
    pub command_id: CommandId,
    pub command_fingerprint: String,
    pub receipt: CommandReceipt,
    pub new_session: Option<Session>,
    pub new_task: Option<Task>,
    pub new_run_snapshot: Option<RunSnapshot>,
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
