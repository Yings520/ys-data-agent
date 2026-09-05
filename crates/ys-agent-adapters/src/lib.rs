//! Adapters for external systems and infrastructure.

pub mod context;
pub mod credential;
pub mod data;
pub mod model;
pub mod oauth;
pub mod tools;

pub use context::{DbtManifestAdapter, FileMetricRegistry};
pub use data::{
    ColumnAction, DuckDbConnector, DuckDbConnectorFactory, GovernedQueryResult, PostgresConnector,
    PostgresConnectorConfig, PostgresConnectorFactory, RestrictedResultContext,
    RestrictedResultPayload, ResultPolicy, SqlPolicyDecision, SqlPolicyDisposition,
    SqlPolicyReason, SqlReadOnlyPolicy, SqliteConnector, SqliteConnectorConfig, SupportedDialect,
};
pub use tools::{
    ArtifactLookup, ArtifactRecord, CompiledQuery, ConnectorHandle, ConnectorRegistry,
    InspectSchemaTool, MetricSqlCompiler, MetricSqlDialect, QueryDataInput, QueryDataTool,
    ReadFreshnessTool, ResolveMetricTool, RuntimeArtifactLookup,
};
