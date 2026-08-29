mod inspect_schema;
mod query_data;
mod read_freshness;
mod resolve_metric;

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use ys_agent_core::{
    ArtifactAccessContext, ArtifactId, ArtifactMetadata, ArtifactRef, ArtifactStore, CatalogReader,
    CoreError, CoreResult, CostClass, FreshnessReader, PutArtifact, QueryPreflightReader,
    RuntimeStore, SourceId, SqlQueryExecutor, ToolFailure, ToolFailureCategory, ToolOutcome,
};

pub use inspect_schema::InspectSchemaTool;
pub use query_data::{CompiledQuery, MetricSqlCompiler, QueryDataInput, QueryDataTool};
pub use read_freshness::ReadFreshnessTool;
pub use resolve_metric::ResolveMetricTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricSqlDialect {
    Sqlite,
    Postgres,
}

#[derive(Clone)]
pub struct ConnectorHandle {
    pub dialect: MetricSqlDialect,
    pub catalog: Arc<dyn CatalogReader>,
    pub preflight: Arc<dyn QueryPreflightReader>,
    pub query: Arc<dyn SqlQueryExecutor>,
    pub freshness: Arc<dyn FreshnessReader>,
}

#[derive(Clone, Default)]
pub struct ConnectorRegistry {
    connectors: BTreeMap<String, ConnectorHandle>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(
        &mut self,
        source_id: SourceId,
        dialect: MetricSqlDialect,
        connector: Arc<T>,
    ) -> CoreResult<()>
    where
        T: CatalogReader
            + QueryPreflightReader
            + SqlQueryExecutor
            + FreshnessReader
            + Send
            + Sync
            + 'static,
    {
        let key = source_id.as_str().to_owned();
        if self.connectors.contains_key(&key) {
            return Err(CoreError::validation(
                "duplicate_connector",
                format!("source {key} is already registered"),
            ));
        }
        self.connectors.insert(
            key,
            ConnectorHandle {
                dialect,
                catalog: connector.clone(),
                preflight: connector.clone(),
                query: connector.clone(),
                freshness: connector,
            },
        );
        Ok(())
    }

    pub fn get(&self, source_id: &SourceId) -> CoreResult<ConnectorHandle> {
        self.connectors
            .get(source_id.as_str())
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                entity: "connector",
                id: source_id.as_str().to_owned(),
            })
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub metadata: ArtifactMetadata,
    pub bytes: Vec<u8>,
}

#[async_trait]
pub trait ArtifactLookup: Send + Sync {
    async fn load(
        &self,
        artifact_id: &ArtifactId,
        access: &ArtifactAccessContext,
    ) -> CoreResult<ArtifactRecord>;
}

pub struct RuntimeArtifactLookup {
    runtime: Arc<dyn RuntimeStore>,
    bodies: Arc<dyn ArtifactStore>,
}

impl RuntimeArtifactLookup {
    pub fn new(runtime: Arc<dyn RuntimeStore>, bodies: Arc<dyn ArtifactStore>) -> Self {
        Self { runtime, bodies }
    }
}

#[async_trait]
impl ArtifactLookup for RuntimeArtifactLookup {
    async fn load(
        &self,
        artifact_id: &ArtifactId,
        access: &ArtifactAccessContext,
    ) -> CoreResult<ArtifactRecord> {
        let metadata = self.runtime.load_artifact(artifact_id).await?;
        let bytes = self
            .bodies
            .get(&ArtifactRef::new(metadata.clone()), access)
            .await?;
        Ok(ArtifactRecord { metadata, bytes })
    }
}

pub(crate) fn parse_arguments<T>(arguments: Value) -> Result<T, ToolOutcome>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments).map_err(|_| {
        rejected(
            "invalid_tool_arguments",
            ToolFailureCategory::InvalidArguments,
            "Tool arguments do not match the declared input",
            true,
            CostClass::Low,
        )
    })
}

pub(crate) fn rejected(
    code: impl Into<String>,
    category: ToolFailureCategory,
    message: impl Into<String>,
    parameter_revision_allowed: bool,
    cost_class: CostClass,
) -> ToolOutcome {
    ToolOutcome::Rejected {
        failure: ToolFailure {
            code: code.into(),
            category,
            user_message: message.into(),
            retryable: false,
            parameter_revision_allowed,
            remote_query_id: None,
            cost_class,
        },
    }
}

pub(crate) fn failed(
    code: impl Into<String>,
    category: ToolFailureCategory,
    message: impl Into<String>,
    retryable: bool,
    cost_class: CostClass,
) -> ToolOutcome {
    ToolOutcome::Failed {
        failure: ToolFailure {
            code: code.into(),
            category,
            user_message: message.into(),
            retryable,
            parameter_revision_allowed: false,
            remote_query_id: None,
            cost_class,
        },
    }
}

pub(crate) async fn put_json<T>(
    store: &dyn ArtifactStore,
    request: PutArtifact,
    value: &T,
) -> CoreResult<ArtifactMetadata>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|error| {
        CoreError::validation("artifact_serialization_failed", error.to_string())
    })?;
    store.put(PutArtifact { bytes, ..request }).await
}

pub(crate) fn safe_internal_failure(error: &CoreError, cost: CostClass) -> ToolOutcome {
    failed(
        error.code(),
        ToolFailureCategory::Internal,
        format!("Tool dependency failed with code {}", error.code()),
        false,
        cost,
    )
}
