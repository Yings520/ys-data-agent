use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use ys_agent_adapters::SqliteConnector;
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
        let path = root.join("trusted.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE readings (value INTEGER, recorded_at TEXT, secret TEXT); INSERT INTO readings VALUES (42, '2026-01-01T00:00:00Z', 'canary'), (43, '2026-01-02T00:00:00Z', 'canary');").unwrap();
        drop(db);
        let workspace_id = WorkspaceId::new();
        let source = SourceId::new("managed_test");
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
                adapter_id: "sqlite".to_owned().try_into().unwrap(),
                adapter_version: "1".to_owned().try_into().unwrap(),
                config_version: 1,
                source_id: Some(source.clone()),
                fields: [(
                    FieldId::new("database_path").unwrap(),
                    FieldValue::Text(path.to_str().unwrap().into()),
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
                    source_id: source.as_str().into(),
                    relations: relations.clone(),
                },
                result_policy: relations,
                budget: QueryBudget {
                    max_concurrency: 1,
                    ..QueryBudget::default()
                },
                policy_digest: DatasourceDigest::of(&"explicit-test-authority").unwrap(),
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
            query_tag: "contract".into(),
            scope: self.input.governance.data_scope.clone(),
            confirmation_granted: false,
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
async fn output_alias_cannot_downgrade_a_restricted_input() {
    let fixture = Fixture::new();
    let request = fixture.request("SELECT secret AS value FROM readings");
    let connector = SqliteConnector::open_managed(fixture.input).await.unwrap();
    let result = connector.execute_query(request).await.unwrap();
    assert!(!result.model_preview.contains("canary"));
    assert_eq!(result.rows[0][0], CellValue::Text("[REDACTED]".into()));
}

#[tokio::test]
async fn duplicate_output_names_cannot_hide_restricted_provenance() {
    let fixture = Fixture::new();
    let request = fixture.request("SELECT secret AS value, value FROM readings");
    let connector = SqliteConnector::open_managed(fixture.input).await.unwrap();
    assert!(connector.execute_query(request).await.is_err());
}

#[tokio::test]
async fn managed_sqlite_real_contract_and_close() {
    let fixture = Fixture::new();
    let request = fixture.request("SELECT value FROM readings ORDER BY value");
    let secret = fixture.request("SELECT value, secret FROM readings ORDER BY value");
    let connector = SqliteConnector::open_managed(fixture.input).await.unwrap();
    let result = connector.execute_query(secret).await.unwrap();
    assert_eq!(result.rows[0][0], CellValue::Integer(42));
    assert_eq!(result.rows[0][1], CellValue::Text("[REDACTED]".into()));
    assert!(!result.model_preview.contains("canary"));
    connector_contract::assert_contract(
        &connector,
        request,
        CellValue::Integer(42),
        "readings",
        "recorded_at",
    )
    .await;
}

#[tokio::test]
async fn managed_sqlite_rejects_missing_outside_and_replaced_files() {
    let mut fixture = Fixture::new();
    fixture.input.governance.allowed_roots.clear();
    assert!(SqliteConnector::open_managed(fixture.input).await.is_err());

    let fixture = Fixture::new();
    let path = fixture.path();
    std::fs::remove_file(&path).unwrap();
    assert!(SqliteConnector::open_managed(fixture.input).await.is_err());
    assert!(!path.exists(), "opening a missing target never creates it");

    let fixture = Fixture::new();
    let path = fixture.path();
    let request = fixture.request("SELECT value FROM readings");
    let connector = SqliteConnector::open_managed(fixture.input).await.unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE readings(value); INSERT INTO readings VALUES(99)")
        .unwrap();
    assert!(
        connector.execute_query(request).await.is_err(),
        "replacement invalidates the handle"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn managed_sqlite_rejects_symlinks() {
    let fixture = Fixture::new();
    let path = fixture.path();
    let original = path.with_extension("original");
    std::fs::rename(&path, &original).unwrap();
    std::os::unix::fs::symlink(&original, &path).unwrap();
    assert!(SqliteConnector::open_managed(fixture.input).await.is_err());
}

#[tokio::test]
async fn managed_sqlite_enforces_bound_scope_rows_and_actual_timeout() {
    let mut fixture = Fixture::new();
    fixture.input.governance.budget.max_rows = 1;
    let request = fixture.request("SELECT value FROM readings ORDER BY value");
    let mut slow = fixture.request("SELECT count(*) FROM readings a CROSS JOIN readings b CROSS JOIN readings c CROSS JOIN readings d CROSS JOIN readings e CROSS JOIN readings f CROSS JOIN readings g CROSS JOIN readings h CROSS JOIN readings i CROSS JOIN readings j CROSS JOIN readings k CROSS JOIN readings l CROSS JOIN readings m CROSS JOIN readings n CROSS JOIN readings o CROSS JOIN readings p CROSS JOIN readings q CROSS JOIN readings r CROSS JOIN readings s CROSS JOIN readings t CROSS JOIN readings u CROSS JOIN readings v CROSS JOIN readings w CROSS JOIN readings x CROSS JOIN readings y CROSS JOIN readings z");
    slow.budget.statement_timeout_ms = 10;
    let mut wrong_scope = request.clone();
    wrong_scope.scope.workspace_id = WorkspaceId::new();
    let connector = Arc::new(SqliteConnector::open_managed(fixture.input).await.unwrap());
    assert!(connector.execute_query(wrong_scope).await.is_err());
    let result = connector.execute_query(request.clone()).await.unwrap();
    assert_eq!(result.row_count, 1);
    assert!(result.truncated);
    let error = tokio::time::timeout(Duration::from_secs(2), connector.execute_query(slow))
        .await
        .expect("actual work must stop")
        .unwrap_err();
    assert_eq!(error.code(), "datasource_timeout");
    assert!(
        connector.execute_query(request).await.is_ok(),
        "timeout releases its connection/permit"
    );
    tokio::time::timeout(Duration::from_secs(2), connector.close())
        .await
        .unwrap()
        .unwrap();
}
