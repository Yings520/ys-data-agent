use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use chrono::{TimeZone, Utc};
use ys_agent_adapters::{DuckDbConnectorFactory, MetricSqlCompiler, MetricSqlDialect};
use ys_agent_core::*;

#[path = "support/connector_contract.rs"]
mod connector_contract;

struct Fixture {
    _directory: tempfile::TempDir,
    input: ConnectorOpenInput,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("trusted.duckdb");
        let connection = duckdb::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE readings(value BIGINT, recorded_at TIMESTAMPTZ, secret VARCHAR);\
                 INSERT INTO readings VALUES\
                 (42, TIMESTAMPTZ '2026-01-01T00:00:00Z', 'canary'),\
                 (43, TIMESTAMPTZ '2026-01-02T00:00:00Z', 'canary');",
            )
            .unwrap();
        drop(connection);

        let workspace_id = WorkspaceId::new();
        let source = SourceId::new("duckdb_test");
        let relations: BTreeMap<String, BTreeMap<String, ColumnPolicy>> = [(
            "readings".into(),
            [
                ("value".into(), ColumnPolicy::Allow),
                ("recorded_at".into(), ColumnPolicy::Allow),
                ("secret".into(), ColumnPolicy::Redact),
            ]
            .into(),
        )]
        .into();
        let input = ConnectorOpenInput {
            revision: DatasourceRevision::new(DatasourceRevisionInput {
                schema_version: 1,
                workspace_id,
                profile_id: ProfileId::new(),
                revision: 1,
                adapter_id: "duckdb".try_into().unwrap(),
                adapter_version: "1".try_into().unwrap(),
                config_version: 1,
                source_id: Some(source),
                fields: [(
                    FieldId::new("database_path").unwrap(),
                    FieldValue::Text(path.to_string_lossy().into_owned()),
                )]
                .into(),
                context: DatabaseContext::File {
                    canonical_path: path,
                },
                credential: None,
            })
            .unwrap(),
            secret: None,
            governance: DatasourceGovernanceContext {
                data_scope: AllowedDataScope {
                    workspace_id,
                    source_id: "duckdb_test".into(),
                    relations: relations.clone(),
                },
                result_policy: relations,
                budget: QueryBudget {
                    max_concurrency: 1,
                    statement_timeout_ms: 2_000,
                    acquire_timeout_ms: 1_000,
                    ..QueryBudget::default()
                },
                policy_digest: DatasourceDigest::of(&"explicit-duckdb-test-authority").unwrap(),
                allowed_roots: vec![root],
            },
        };
        Self {
            _directory: directory,
            input,
        }
    }

    fn request(&self, sql: &str) -> QueryRequest {
        QueryRequest {
            source_id: self.input.revision.input().source_id.clone().unwrap(),
            sql: sql.into(),
            parameters: vec![],
            budget: self.input.governance.budget.clone(),
            query_tag: "managed-duckdb-contract".into(),
            scope: self.input.governance.data_scope.clone(),
            confirmation_granted: true,
        }
    }

    fn path(&self) -> PathBuf {
        match &self.input.revision.input().context {
            DatabaseContext::File { canonical_path } => canonical_path.clone(),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn managed_duckdb_real_contract_metric_security_timeout_and_close() {
    let fixture = Fixture::new();
    let query = fixture.request("SELECT value FROM readings ORDER BY value");
    let secret = fixture.request("SELECT value, secret FROM readings ORDER BY value");
    let connector = DuckDbConnectorFactory.open(fixture.input).await.unwrap();

    let redacted = connector.execute_query(secret).await.unwrap();
    assert_eq!(redacted.rows[0][1], CellValue::Text("[REDACTED]".into()));
    assert!(!redacted.model_preview.contains("canary"));
    connector_contract::assert_contract(
        connector.as_ref(),
        query.clone(),
        CellValue::Integer(42),
        "readings",
        "recorded_at",
    )
    .await;

    let limited_fixture = Fixture::new();
    let healthy = limited_fixture.request("SELECT value FROM readings ORDER BY value");
    let typed = limited_fixture.request(
        "SELECT NULL, CAST(value AS DECIMAL(10,2)), recorded_at FROM readings ORDER BY value LIMIT 1",
    );
    let unsupported = limited_fixture.request("SELECT [value] FROM readings LIMIT 1");
    let mut slow = limited_fixture.request(
        "SELECT count(*) FROM readings a CROSS JOIN readings b CROSS JOIN readings c CROSS JOIN readings d CROSS JOIN readings e CROSS JOIN readings f CROSS JOIN readings g CROSS JOIN readings h CROSS JOIN readings i CROSS JOIN readings j CROSS JOIN readings k CROSS JOIN readings l CROSS JOIN readings m CROSS JOIN readings n CROSS JOIN readings o CROSS JOIN readings p CROSS JOIN readings q CROSS JOIN readings r CROSS JOIN readings s CROSS JOIN readings t CROSS JOIN readings u CROSS JOIN readings v CROSS JOIN readings w CROSS JOIN readings x CROSS JOIN readings y CROSS JOIN readings z",
    );
    let closed_request = limited_fixture.request("SELECT value FROM readings");
    let prohibited = [
        "ATTACH ':memory:' AS escape",
        "COPY readings TO '/tmp/ysda-duckdb-escape.csv'",
        "SELECT * FROM read_csv('/tmp/ysda-duckdb-escape.csv')",
        "INSTALL httpfs",
        "LOAD httpfs",
    ]
    .into_iter()
    .map(|sql| (sql, limited_fixture.request(sql)))
    .collect::<Vec<_>>();
    slow.budget.statement_timeout_ms = 10;
    let connector = DuckDbConnectorFactory
        .open(limited_fixture.input)
        .await
        .unwrap();

    let metric = MetricDefinition {
        id: "demo.sum".into(),
        version: "1".into(),
        status: MetricStatus::Active,
        description: "sum".into(),
        source_relation: "readings".into(),
        expression: "SUM(value)".into(),
        time_column: "recorded_at".into(),
        allowed_dimensions: vec![],
        owner: "test".into(),
        freshness_sla_seconds: None,
    };
    let compiled = MetricSqlCompiler::new(MetricSqlDialect::DuckDb)
        .compile(
            healthy.source_id.clone(),
            &metric,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            &[],
        )
        .unwrap();
    let metric_result = connector
        .execute_query(QueryRequest {
            source_id: compiled.source_id,
            sql: compiled.sql,
            parameters: compiled.parameters,
            budget: healthy.budget.clone(),
            query_tag: "duckdb-metric".into(),
            scope: healthy.scope.clone(),
            confirmation_granted: true,
        })
        .await
        .unwrap();
    assert_eq!(metric_result.rows, vec![vec![CellValue::Integer(85)]]);

    let typed_result = connector.execute_query(typed).await.unwrap();
    assert_eq!(typed_result.rows[0][0], CellValue::Null);
    assert_eq!(typed_result.rows[0][1], CellValue::Text("42.00".into()));
    assert!(matches!(typed_result.rows[0][2], CellValue::Text(_)));
    assert_eq!(
        connector
            .execute_query(unsupported)
            .await
            .unwrap_err()
            .code(),
        "unsupported_duckdb_type"
    );

    let mut one_row = healthy.clone();
    one_row.budget.max_rows = 1;
    let limited_rows = connector.execute_query(one_row).await.unwrap();
    assert_eq!(limited_rows.row_count, 1);
    assert!(limited_rows.truncated);

    for (sql, request) in prohibited {
        assert!(
            connector.execute_query(request).await.is_err(),
            "{sql} must be rejected"
        );
    }

    let timeout = tokio::time::timeout(Duration::from_secs(2), connector.execute_query(slow))
        .await
        .expect("the driver is actually interrupted")
        .unwrap_err();
    assert_eq!(timeout.code(), "datasource_timeout");
    assert!(connector.execute_query(healthy).await.is_ok());
    connector.close().await.unwrap();
    connector.close().await.unwrap();
    assert_eq!(
        connector
            .execute_query(closed_request)
            .await
            .unwrap_err()
            .code(),
        "datasource_closed"
    );
}

#[tokio::test]
async fn managed_duckdb_missing_outside_and_replaced_files_fail_closed() {
    let mut outside = Fixture::new();
    outside.input.governance.allowed_roots.clear();
    assert!(DuckDbConnectorFactory.open(outside.input).await.is_err());

    let missing = Fixture::new();
    let missing_path = missing.path();
    std::fs::remove_file(&missing_path).unwrap();
    assert!(DuckDbConnectorFactory.open(missing.input).await.is_err());
    assert!(!missing_path.exists(), "missing database is never created");

    let replaced = Fixture::new();
    let path = replaced.path();
    let request = replaced.request("SELECT value FROM readings");
    let connector = DuckDbConnectorFactory.open(replaced.input).await.unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    let replacement = duckdb::Connection::open(&path).unwrap();
    replacement
        .execute_batch("CREATE TABLE readings(value BIGINT); INSERT INTO readings VALUES (99)")
        .unwrap();
    drop(replacement);
    assert!(connector.execute_query(request).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn managed_duckdb_rejects_symbolic_links() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let original = path.with_extension("original");
    std::fs::rename(&path, &original).unwrap();
    std::os::unix::fs::symlink(&original, &path).unwrap();
    assert!(DuckDbConnectorFactory.open(fixture.input).await.is_err());
}

#[test]
fn duckdb_metric_sql_uses_bound_parameters() {
    let metric = MetricDefinition {
        id: "demo.sum".into(),
        version: "1".into(),
        status: MetricStatus::Active,
        description: "sum".into(),
        source_relation: "readings".into(),
        expression: "SUM(value)".into(),
        time_column: "recorded_at".into(),
        allowed_dimensions: vec![],
        owner: "test".into(),
        freshness_sla_seconds: None,
    };
    let compiled = MetricSqlCompiler::new(MetricSqlDialect::DuckDb)
        .compile(
            SourceId::new("duckdb_test"),
            &metric,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 1, 3, 0, 0, 0).unwrap(),
            &[],
        )
        .unwrap();
    assert!(compiled.sql.contains("\"recorded_at\" >= ?"));
    assert_eq!(compiled.parameters.len(), 2);
}
