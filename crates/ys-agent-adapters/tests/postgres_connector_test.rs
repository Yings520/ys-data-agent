use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ys_agent_adapters::{
    PostgresConnector, PostgresConnectorConfig, ResultPolicy, SqlReadOnlyPolicy, SupportedDialect,
};
use ys_agent_core::{
    CatalogReader, CellValue, FreshnessReader, QueryBudget, QueryPreflightDecision,
    QueryPreflightReader, QueryRequest, SourceId, SqlQueryExecutor, WorkspaceId,
};

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn request(
    source_id: &SourceId,
    scope: &ys_agent_core::AllowedDataScope,
    sql: &str,
) -> QueryRequest {
    QueryRequest {
        source_id: source_id.clone(),
        sql: sql.to_owned(),
        parameters: Vec::new(),
        budget: QueryBudget {
            max_rows: 2,
            max_result_bytes: 16 * 1024,
            max_estimated_cost_units: Some(10_000),
            ..QueryBudget::default()
        },
        query_tag: "postgres-integration".to_owned(),
        scope: scope.clone(),
        confirmation_granted: false,
    }
}

#[tokio::test]
#[ignore = "requires fixtures/postgres/compose.yaml"]
async fn postgres_exposes_bounded_read_only_query_capabilities() {
    let database_url = std::env::var("YSDA_TEST_POSTGRES_URL")
        .expect("YSDA_TEST_POSTGRES_URL is required for ignored integration test");
    let policy_bytes =
        fs::read(fixture_path("fixtures/policy/query-policy.json")).expect("read query policy");
    let result_policy = ResultPolicy::from_json_bytes(&policy_bytes).expect("valid policy");
    let source_id = SourceId::new("postgres_demo");
    let scope = result_policy
        .allowed_scope(WorkspaceId::new(), &source_id)
        .expect("PostgreSQL scope");
    let connector = PostgresConnector::connect(
        PostgresConnectorConfig {
            source_id: source_id.clone(),
            max_connections: 2,
            acquire_timeout: Duration::from_secs(5),
            default_statement_timeout: Duration::from_secs(30),
            confirmation_cost_units: 1_000,
            freshness_columns: [("public.orders".to_owned(), "paid_at".to_owned())]
                .into_iter()
                .collect(),
        },
        &database_url,
        SqlReadOnlyPolicy::new(SupportedDialect::Postgres, 16_384),
        result_policy,
    )
    .await
    .expect("connect PostgreSQL fixture");

    let capabilities = connector.capabilities();
    assert!(capabilities.catalog_reader);
    assert!(capabilities.sql_query_executor);
    assert!(capabilities.freshness_reader);
    assert!(capabilities.supports_explain);
    assert!(capabilities.supports_read_only_tx);

    let schema = connector
        .observe_schema(&source_id)
        .await
        .expect("inspect PostgreSQL catalog");
    assert!(
        schema
            .relations
            .iter()
            .any(|relation| relation.name == "public.orders")
    );

    let select = request(
        &source_id,
        &scope,
        "SELECT order_id, paid_amount FROM public.orders ORDER BY order_id",
    );
    let preflight = connector
        .preflight(&select)
        .await
        .expect("preflight SELECT");
    assert_eq!(preflight.decision, QueryPreflightDecision::Allowed);
    let result = connector
        .execute_query(select)
        .await
        .expect("execute SELECT");
    assert_eq!(result.row_count, 2);
    assert!(result.truncated);
    assert!(matches!(result.rows[0][1], CellValue::Text(_)));
    assert!(result.remote_query_id.is_some());

    let freshness = connector
        .read_freshness(&source_id, "public.orders", "paid_at")
        .await
        .expect("read PostgreSQL freshness");
    assert!(freshness.data_as_of.is_some());

    let delete = request(&source_id, &scope, "DELETE FROM public.orders");
    let rejected = connector
        .preflight(&delete)
        .await
        .expect("preflight DELETE");
    assert_eq!(rejected.decision, QueryPreflightDecision::Rejected);
    let error = connector
        .execute_query(delete)
        .await
        .expect_err("DELETE must be rejected before the server");
    assert_eq!(error.code(), "statement_not_read_only");

    let count = request(
        &source_id,
        &scope,
        "SELECT order_id FROM public.orders ORDER BY order_id",
    );
    let rows_after_rejection = connector.execute_query(count).await.expect("rows remain");
    assert_eq!(rows_after_rejection.row_count, 2);
}
