use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;
use ys_agent_adapters::{
    ResultPolicy, SqlReadOnlyPolicy, SqliteConnector, SqliteConnectorConfig, SupportedDialect,
};
use ys_agent_core::{
    CatalogReader, CellValue, CoreError, FreshnessReader, QueryBudget, QueryRequest,
    SchemaKnowledgeKind, SourceId, SqlQueryExecutor, WorkspaceId,
};

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

struct SqliteFixture {
    _directory: TempDir,
    connector: SqliteConnector,
    source_id: SourceId,
    scope: ys_agent_core::AllowedDataScope,
}

impl SqliteFixture {
    async fn from_seed() -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database_path = directory.path().join("demo.db");
        let seed = fs::read_to_string(fixture_path("fixtures/sql/sqlite_seed.sql"))
            .expect("read SQLite seed");
        let connection = Connection::open(&database_path).expect("create SQLite fixture");
        connection.execute_batch(&seed).expect("apply SQLite seed");
        drop(connection);

        let policy_bytes =
            fs::read(fixture_path("fixtures/policy/query-policy.json")).expect("read query policy");
        let result_policy = ResultPolicy::from_json_bytes(&policy_bytes).expect("valid policy");
        let source_id = SourceId::new("sqlite_demo");
        let scope = result_policy
            .allowed_scope(WorkspaceId::new(), &source_id)
            .expect("SQLite scope");
        let connector = SqliteConnector::new(
            SqliteConnectorConfig {
                source_id: source_id.clone(),
                database_path,
                max_concurrency: 1,
                freshness_columns: [("mart_orders".to_owned(), "paid_at".to_owned())]
                    .into_iter()
                    .collect(),
            },
            SqlReadOnlyPolicy::new(
                SupportedDialect::SQLite,
                QueryBudget::default().max_sql_bytes,
            ),
            result_policy,
        );

        Self {
            _directory: directory,
            connector,
            source_id,
            scope,
        }
    }

    fn request(&self, sql: &str) -> QueryRequest {
        QueryRequest {
            source_id: self.source_id.clone(),
            sql: sql.to_owned(),
            parameters: Vec::new(),
            budget: QueryBudget {
                max_rows: 2,
                max_result_bytes: 16 * 1024,
                ..QueryBudget::default()
            },
            query_tag: "sqlite-test".to_owned(),
            scope: self.scope.clone(),
            confirmation_granted: false,
        }
    }
}

#[tokio::test]
async fn sqlite_advertises_only_implemented_capabilities() {
    let fixture = SqliteFixture::from_seed().await;
    let descriptor = fixture.connector.capabilities();

    assert!(descriptor.catalog_reader);
    assert!(descriptor.sql_query_executor);
    assert!(descriptor.freshness_reader);
    assert!(!descriptor.supports_explain);
    assert!(!descriptor.supports_read_only_tx);
}

#[tokio::test]
async fn sqlite_is_logically_read_only_and_does_not_change_rows() {
    let fixture = SqliteFixture::from_seed().await;
    let error = fixture
        .connector
        .execute_query(fixture.request("DELETE FROM mart_orders"))
        .await
        .expect_err("writes must be rejected");

    assert_eq!(error.code(), "statement_not_read_only");

    let result = fixture
        .connector
        .execute_query(fixture.request("SELECT order_id FROM mart_orders ORDER BY order_id"))
        .await
        .expect("read rows after rejection");
    assert_eq!(result.row_count, 2);
    assert!(result.truncated);
}

#[tokio::test]
async fn sqlite_catalog_returns_observed_not_inferred_schema() {
    let fixture = SqliteFixture::from_seed().await;
    let schema = fixture
        .connector
        .observe_schema(&fixture.source_id)
        .await
        .expect("inspect SQLite catalog");

    assert_eq!(schema.kind, SchemaKnowledgeKind::Observed);
    assert_eq!(schema.relations.len(), 1, "raw_customers is outside scope");
    assert_eq!(schema.relations[0].name, "mart_orders");
    assert!(
        schema.relations[0]
            .columns
            .iter()
            .any(|column| column.name == "order_id" && column.primary_key_position == Some(1))
    );
}

#[tokio::test]
async fn relation_outside_allowed_scope_is_rejected_before_execution() {
    let fixture = SqliteFixture::from_seed().await;
    let error = fixture
        .connector
        .execute_query(fixture.request("SELECT customer_id FROM raw_customers"))
        .await
        .expect_err("relation ACL must be enforced");

    assert_eq!(error.code(), "relation_not_allowed");
}

#[tokio::test]
async fn restricted_columns_never_enter_model_preview() {
    let fixture = SqliteFixture::from_seed().await;
    let result = fixture
        .connector
        .execute_query(fixture.request("SELECT customer_email FROM mart_orders ORDER BY order_id"))
        .await
        .expect("redacted query");

    assert_eq!(result.rows[0][0], CellValue::Text("[REDACTED]".to_owned()));
    assert!(!result.model_preview.contains('@'));
    assert!(
        result
            .warning_codes
            .contains(&"restricted_column_redacted".to_owned())
    );
}

#[tokio::test]
async fn sqlite_reads_freshness_from_the_configured_column() {
    let fixture = SqliteFixture::from_seed().await;
    let observation = fixture
        .connector
        .read_freshness(&fixture.source_id, "mart_orders", "paid_at")
        .await
        .expect("read freshness");

    assert_eq!(observation.relation, "mart_orders");
    assert!(observation.data_as_of.is_some());
}

#[test]
fn core_error_is_used_at_the_adapter_boundary() {
    let error = CoreError::validation("example", "typed boundary");
    assert_eq!(error.code(), "example");
}
