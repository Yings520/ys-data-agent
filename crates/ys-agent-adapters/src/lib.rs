//! Adapters for external systems and infrastructure.

pub mod context;
pub mod data;
pub mod model;

pub use context::{DbtManifestAdapter, FileMetricRegistry};
pub use data::{
    ColumnAction, GovernedQueryResult, PostgresConnector, PostgresConnectorConfig,
    RestrictedResultContext, RestrictedResultPayload, ResultPolicy, SqlPolicyDecision,
    SqlPolicyDisposition, SqlPolicyReason, SqlReadOnlyPolicy, SqliteConnector,
    SqliteConnectorConfig, SupportedDialect,
};
