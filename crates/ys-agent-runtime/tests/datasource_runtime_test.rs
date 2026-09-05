use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroU64,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use ys_agent_adapters::credential::datasource::LocalEncryptedDatasourceVault;
use ys_agent_adapters::data::BuiltinConnectorCatalog;
use ys_agent_adapters::{ConnectorRegistry, DuckDbConnectorFactory, PostgresConnectorFactory};
use ys_agent_core::*;
use ys_agent_runtime::{
    ActiveRunDatasourceBindingSource, AgentServiceApi, ConnectorManager, DatasourceService,
    InProcessAgentService, NoopRunScheduler, SendMessageRequest, ServiceReply, SourcePolicy,
    StaticRunProviderBindingSource,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

fn file_policy(path: &std::path::Path, root: &std::path::Path) -> SourcePolicy {
    SourcePolicy::from_json_bytes(
        serde_json::to_vec(&json!({
            "schema_version": 2,
            "allowed_sources": {
                "managed_sqlite": {
                    "relations": {
                        "readings": {"columns": {
                            "value": "allow",
                            "recorded_at": "allow",
                            "secret": "redact"
                        }}
                    },
                    "target": {
                        "kind": "file",
                        "adapter_id": "sqlite",
                        "canonical_path": path,
                        "allowed_roots": [root]
                    }
                }
            }
        }))
        .unwrap()
        .as_slice(),
        QueryBudget {
            max_concurrency: 1,
            ..QueryBudget::default()
        },
    )
    .unwrap()
}

fn write(scope: DatasourceScope, version: u64, head: Option<u64>) -> DatasourceWriteContext {
    DatasourceWriteContext {
        command_id: CommandId::new(),
        scope,
        expected_version: version,
        expected_head_revision: head.and_then(std::num::NonZeroU64::new),
    }
}

fn postgres_policy() -> SourcePolicy {
    SourcePolicy::from_json_bytes(
        br#"{"schema_version":2,"allowed_sources":{"warehouse":{"relations":{"public.orders":{"columns":{"order_id":"allow"}}},"target":{"kind":"database","adapter_id":"postgres","host":"127.0.0.1","port":55432,"database":"ysda_test","schema":"public"}}}}"#,
        QueryBudget {
            max_concurrency: 2,
            ..QueryBudget::default()
        },
    )
    .unwrap()
}

fn postgres_save(
    scope: DatasourceScope,
    version: u64,
    profile_id: Option<ProfileId>,
    head: Option<u64>,
    secret: SecretEdit,
) -> SaveDatasource {
    SaveDatasource {
        write: write(scope, version, head),
        profile_id,
        name: DatasourceName::new("Warehouse").unwrap(),
        adapter_id: "postgres".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        fields: [
            (
                FieldId::new("host").unwrap(),
                FieldValue::Text("127.0.0.1".into()),
            ),
            (FieldId::new("port").unwrap(), FieldValue::Integer(55432)),
            (
                FieldId::new("database").unwrap(),
                FieldValue::Text("ysda_test".into()),
            ),
            (
                FieldId::new("schema").unwrap(),
                FieldValue::Text("public".into()),
            ),
            (
                FieldId::new("username").unwrap(),
                FieldValue::Text("ysda_reader".into()),
            ),
            (
                FieldId::new("tls").unwrap(),
                FieldValue::Text("disable".into()),
            ),
        ]
        .into(),
        context: DatabaseContext::Database {
            catalog: Some("127.0.0.1:55432".into()),
            database: "ysda_test".into(),
            schema: "public".into(),
        },
        secret,
    }
}

#[tokio::test]
async fn service_runs_real_sqlite_crud_validate_select_and_delete() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("managed.db");
    let database = rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("CREATE TABLE readings(value INTEGER, recorded_at TEXT, secret TEXT); INSERT INTO readings VALUES (1, '2026-01-01T00:00:00Z', 'canary')").unwrap();
    drop(database);

    let store = SqliteRuntimeStore::open(root.join("runtime.db"))
        .await
        .unwrap();
    let repository = Arc::new(store.datasource_repository());
    let vault = Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault")));
    let catalog = Arc::new(
        BuiltinConnectorCatalog::new(
            Arc::new(PostgresConnectorFactory),
            Arc::new(DuckDbConnectorFactory),
        )
        .unwrap(),
    );
    let service = DatasourceService::new(
        repository.clone(),
        vault,
        catalog,
        Arc::new(file_policy(&path, &root)),
    );
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };

    let create_write = write(scope, 0, None);
    let save_request = |name: &str| SaveDatasource {
        write: create_write,
        profile_id: None,
        name: DatasourceName::new(name).unwrap(),
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        fields: [(
            FieldId::new("database_path").unwrap(),
            FieldValue::Text(path.to_string_lossy().into_owned()),
        )]
        .into(),
        context: DatabaseContext::File {
            canonical_path: path.clone(),
        },
        secret: SecretEdit::Keep,
    };
    let saved = service.save(save_request("Local analytics")).await.unwrap();
    assert_eq!(saved.state, RevisionState::Draft);
    assert_eq!(
        saved.profile.source_id.as_ref().unwrap().as_str(),
        "managed_sqlite"
    );
    let replayed = service.save(save_request("Local analytics")).await.unwrap();
    assert_eq!(replayed.revision, saved.revision);
    assert_eq!(
        service
            .save(save_request("Different request"))
            .await
            .unwrap_err()
            .code,
        DsErrorCode::Conflict
    );

    let cancelled_operation = OperationId::new();
    service.cancel(cancelled_operation).await.unwrap();
    let cancelled = service
        .validate(ValidateDatasource {
            write: write(scope, 1, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: cancelled_operation,
        })
        .await
        .unwrap_err();
    assert_eq!(cancelled.code, DsErrorCode::Cancelled);
    assert_eq!(service.view(scope).await.unwrap().snapshot.version, 1);

    let local = service
        .validate(ValidateDatasource {
            write: write(scope, 1, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Local,
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    assert!(local.fields.is_empty());
    assert_eq!(local.state, RevisionState::Draft);

    let connected = service
        .validate(ValidateDatasource {
            write: write(scope, 2, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    assert_eq!(connected.state, RevisionState::Ready);
    assert!(connected.evidence.is_some());
    let doctor = service
        .doctor(DatasourceDoctorRequest {
            scope,
            revision: Some(saved.revision.identity()),
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    assert!(doctor.findings.is_empty());
    assert_eq!(
        doctor
            .validation
            .as_ref()
            .and_then(|report| report.evidence.as_ref())
            .map(|evidence| evidence.inputs().revision()),
        Some(saved.revision.identity())
    );

    let selection = service
        .select(SelectDatasource {
            write: write(scope, 3, Some(1)),
            revision: saved.revision.identity(),
            kind: DatasourceSelectionKind::Session,
        })
        .await
        .unwrap();
    assert_eq!(selection.current, Some(saved.revision.identity()));
    assert_eq!(
        selection.header.as_ref().unwrap().name.as_str(),
        "Local analytics"
    );

    let edited = service
        .save(SaveDatasource {
            write: write(scope, 4, Some(1)),
            profile_id: Some(saved.profile.profile_id),
            name: DatasourceName::new("Renamed analytics").unwrap(),
            adapter_id: "sqlite".try_into().unwrap(),
            adapter_version: "1".try_into().unwrap(),
            config_version: 1,
            fields: [(
                FieldId::new("database_path").unwrap(),
                FieldValue::Text(path.to_string_lossy().into_owned()),
            )]
            .into(),
            context: DatabaseContext::File {
                canonical_path: path,
            },
            secret: SecretEdit::Keep,
        })
        .await
        .unwrap();
    assert_eq!(edited.revision.number(), 2);
    assert_eq!(edited.state, RevisionState::Draft);
    assert_eq!(
        service
            .view(scope)
            .await
            .unwrap()
            .snapshot
            .selection
            .current,
        Some(saved.revision.identity()),
        "editing does not switch the selected Ready revision"
    );

    let stale = service
        .delete(DeleteDatasource {
            write: write(scope, 4, Some(2)),
            profile_id: saved.profile.profile_id,
            disposition: DeleteDatasourceDisposition::ConfirmUnconfigured,
        })
        .await
        .unwrap_err();
    assert_eq!(stale.code, DsErrorCode::Conflict);

    let deleted = service
        .delete(DeleteDatasource {
            write: write(scope, 5, Some(2)),
            profile_id: saved.profile.profile_id,
            disposition: DeleteDatasourceDisposition::ConfirmUnconfigured,
        })
        .await
        .unwrap();
    assert!(deleted.current.is_none());
    assert!(
        service
            .view(scope)
            .await
            .unwrap()
            .snapshot
            .profiles
            .is_empty()
    );
}

#[tokio::test]
async fn service_keeps_and_replaces_isolated_secret_generations_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let runtime_path = root.join("runtime.db");
    let repository = Arc::new(
        SqliteRuntimeStore::open(&runtime_path)
            .await
            .unwrap()
            .datasource_repository(),
    );
    let vault = Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault")));
    let catalog = Arc::new(
        BuiltinConnectorCatalog::new(
            Arc::new(PostgresConnectorFactory),
            Arc::new(DuckDbConnectorFactory),
        )
        .unwrap(),
    );
    let service = DatasourceService::new(
        repository,
        vault.clone(),
        catalog,
        Arc::new(postgres_policy()),
    );
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let first = service
        .save(postgres_save(
            scope,
            0,
            None,
            None,
            SecretEdit::Replace(SecretValue::from_utf8("first-canary".into())),
        ))
        .await
        .unwrap();
    let first_reference = first.revision.input().credential.unwrap();
    assert_eq!(first_reference.generation(), 1);

    let kept = service
        .save(postgres_save(
            scope,
            1,
            Some(first.profile.profile_id),
            Some(1),
            SecretEdit::Keep,
        ))
        .await
        .unwrap();
    assert_eq!(kept.revision.input().credential, Some(first_reference));

    let replaced = service
        .save(postgres_save(
            scope,
            2,
            Some(first.profile.profile_id),
            Some(2),
            SecretEdit::Replace(SecretValue::from_utf8("second-canary".into())),
        ))
        .await
        .unwrap();
    let second_reference = replaced.revision.input().credential.unwrap();
    assert_eq!(second_reference.generation(), 2);
    assert!(vault.read(first_reference).await.is_err());
    assert_eq!(
        vault
            .read(second_reference)
            .await
            .unwrap()
            .value
            .with_exposed(str::to_owned),
        "second-canary"
    );
    let persisted = std::fs::read(runtime_path).unwrap();
    let persisted = String::from_utf8_lossy(&persisted);
    assert!(!persisted.contains("first-canary"));
    assert!(!persisted.contains("second-canary"));
}

#[tokio::test]
async fn service_runs_real_duckdb_save_validate_and_select() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("managed.duckdb");
    let database = duckdb::Connection::open(&path).unwrap();
    database
        .execute_batch("CREATE TABLE readings(value BIGINT); INSERT INTO readings VALUES (1)")
        .unwrap();
    drop(database);
    let policy = SourcePolicy::from_json_bytes(
        serde_json::to_vec(&json!({
            "schema_version": 2,
            "allowed_sources": {"managed_duckdb": {
                "relations": {"readings": {"columns": {"value": "allow"}}},
                "target": {"kind": "file", "adapter_id": "duckdb", "canonical_path": path, "allowed_roots": [root]}
            }}
        }))
        .unwrap()
        .as_slice(),
        QueryBudget {
            max_concurrency: 1,
            ..QueryBudget::default()
        },
    )
    .unwrap();
    let repository = Arc::new(
        SqliteRuntimeStore::open(root.join("runtime.db"))
            .await
            .unwrap()
            .datasource_repository(),
    );
    let service = DatasourceService::new(
        repository,
        Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault"))),
        Arc::new(
            BuiltinConnectorCatalog::new(
                Arc::new(PostgresConnectorFactory),
                Arc::new(DuckDbConnectorFactory),
            )
            .unwrap(),
        ),
        Arc::new(policy),
    );
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let saved = service
        .save(SaveDatasource {
            write: write(scope, 0, None),
            profile_id: None,
            name: DatasourceName::new("Duck analytics").unwrap(),
            adapter_id: "duckdb".try_into().unwrap(),
            adapter_version: "1".try_into().unwrap(),
            config_version: 1,
            fields: [(
                FieldId::new("database_path").unwrap(),
                FieldValue::Text(path.to_string_lossy().into_owned()),
            )]
            .into(),
            context: DatabaseContext::File {
                canonical_path: path,
            },
            secret: SecretEdit::Keep,
        })
        .await
        .unwrap();
    let validated = service
        .validate(ValidateDatasource {
            write: write(scope, 1, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    assert_eq!(validated.state, RevisionState::Ready);
    assert_eq!(
        service
            .select(SelectDatasource {
                write: write(scope, 2, Some(1)),
                revision: saved.revision.identity(),
                kind: DatasourceSelectionKind::Session,
            })
            .await
            .unwrap()
            .current,
        Some(saved.revision.identity())
    );
    let default = service
        .select(SelectDatasource {
            write: write(scope, 3, Some(1)),
            revision: saved.revision.identity(),
            kind: DatasourceSelectionKind::WorkspaceDefault,
        })
        .await
        .unwrap();
    assert_eq!(default.workspace_default, Some(saved.revision.identity()));
}

#[tokio::test]
#[ignore = "requires fixtures/postgres/compose.yaml"]
async fn service_runs_real_postgres_save_validate_and_select() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let repository = Arc::new(
        SqliteRuntimeStore::open(root.join("runtime.db"))
            .await
            .unwrap()
            .datasource_repository(),
    );
    let service = DatasourceService::new(
        repository,
        Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault"))),
        Arc::new(
            BuiltinConnectorCatalog::new(
                Arc::new(PostgresConnectorFactory),
                Arc::new(DuckDbConnectorFactory),
            )
            .unwrap(),
        ),
        Arc::new(postgres_policy()),
    );
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let saved = service
        .save(postgres_save(
            scope,
            0,
            None,
            None,
            SecretEdit::Replace(SecretValue::from_utf8("ysda-reader-test".into())),
        ))
        .await
        .unwrap();
    let validated = service
        .validate(ValidateDatasource {
            write: write(scope, 1, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    assert_eq!(validated.state, RevisionState::Ready);
    assert_eq!(
        service
            .select(SelectDatasource {
                write: write(scope, 2, Some(1)),
                revision: saved.revision.identity(),
                kind: DatasourceSelectionKind::Session,
            })
            .await
            .unwrap()
            .current,
        Some(saved.revision.identity())
    );
}

#[test]
fn source_policy_v2_matches_exact_targets_and_v1_cannot_authorize_profiles() {
    let v1 = br#"{"schema_version":1,"allowed_sources":{}}"#;
    assert_eq!(
        SourcePolicy::from_json_bytes(v1, QueryBudget::default())
            .unwrap_err()
            .code,
        DsErrorCode::ConfigIncompatible
    );

    let policy = SourcePolicy::from_json_bytes(
        br#"{"schema_version":2,"allowed_sources":{"warehouse":{"relations":{"public.orders":{"columns":{"id":"allow"}}},"target":{"kind":"database","adapter_id":"postgres","host":"db.internal","port":5432,"database":"analytics","schema":"public"}}}}"#,
        QueryBudget::default(),
    )
    .unwrap();
    let fields: BTreeMap<FieldId, FieldValue> = [
        (
            FieldId::new("host").unwrap(),
            FieldValue::Text("db.internal".into()),
        ),
        (FieldId::new("port").unwrap(), FieldValue::Integer(5432)),
        (
            FieldId::new("database").unwrap(),
            FieldValue::Text("analytics".into()),
        ),
        (
            FieldId::new("schema").unwrap(),
            FieldValue::Text("public".into()),
        ),
    ]
    .into();
    assert_eq!(
        policy
            .match_target(
                &"postgres".try_into().unwrap(),
                &fields,
                &DatabaseContext::Database {
                    catalog: Some("db.internal:5432".into()),
                    database: "analytics".into(),
                    schema: "public".into(),
                },
            )
            .unwrap()
            .0
            .as_str(),
        "warehouse"
    );

    let directory = tempfile::tempdir().unwrap();
    let allowed = directory.path().join("allowed");
    let unrelated = directory.path().join("unrelated");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&unrelated).unwrap();
    let path = allowed.join("managed.db");
    let document = serde_json::to_vec(&json!({
        "schema_version": 2,
        "allowed_sources": {"local": {
            "relations": {"readings": {"columns": {"value": "allow"}}},
            "target": {
                "kind": "file",
                "adapter_id": "sqlite",
                "canonical_path": path,
                "allowed_roots": [unrelated, allowed]
            }
        }}
    }))
    .unwrap();
    let normal = SourcePolicy::from_json_bytes(&document, QueryBudget::default()).unwrap();
    let restricted = SourcePolicy::from_json_bytes(
        &document,
        QueryBudget {
            max_rows: 1,
            ..QueryBudget::default()
        },
    )
    .unwrap();
    let file_fields = [(
        FieldId::new("database_path").unwrap(),
        FieldValue::Text(path.to_string_lossy().into_owned()),
    )]
    .into();
    let (_, normal_governance) = normal
        .match_target(
            &"sqlite".try_into().unwrap(),
            &file_fields,
            &DatabaseContext::File {
                canonical_path: path.clone(),
            },
        )
        .unwrap();
    let (_, restricted_governance) = restricted
        .match_target(
            &"sqlite".try_into().unwrap(),
            &file_fields,
            &DatabaseContext::File {
                canonical_path: path,
            },
        )
        .unwrap();
    assert_ne!(
        normal_governance.policy_digest, restricted_governance.policy_digest,
        "budget changes invalidate prior validation evidence"
    );
}

struct FakeRepository {
    detail: Mutex<DatasourceDetail>,
    bindings: HashMap<RunId, RunDatasourceBinding>,
}

#[async_trait]
impl DatasourceRepository for FakeRepository {
    async fn load(&self, _: DatasourceScope) -> DsResult<DatasourceSnapshot> {
        Err(ds_error(DsErrorCode::Storage))
    }
    async fn load_revision(&self, _: DatasourceRevisionId) -> DsResult<DatasourceDetail> {
        Ok(self.detail.lock().unwrap().clone())
    }
    async fn commit(&self, _: DatasourceCommit) -> DsResult<DatasourceReceipt> {
        Err(ds_error(DsErrorCode::Storage))
    }
    async fn receipt(&self, _: CommandId) -> DsResult<Option<DatasourceReceipt>> {
        Ok(None)
    }
    async fn pending_secret_mutations(&self, _: WorkspaceId) -> DsResult<Vec<SecretMutation>> {
        Ok(vec![])
    }
    async fn load_run_binding(&self, run: RunId) -> DsResult<RunDatasourceBinding> {
        self.bindings
            .get(&run)
            .cloned()
            .ok_or_else(|| ds_error(DsErrorCode::ConfigIncompatible))
    }
    async fn claim_secret_cleanup(&self, _: DatasourceSecretRef) -> DsResult<()> {
        Ok(())
    }
    async fn finish_secret_cleanup(&self, _: DatasourceSecretRef) -> DsResult<()> {
        Ok(())
    }
    async fn obsolete_secret_generations(
        &self,
        _: WorkspaceId,
    ) -> DsResult<Vec<DatasourceSecretRef>> {
        Ok(vec![])
    }
    async fn finish_secret_mutation(&self, _: OperationId) -> DsResult<()> {
        Ok(())
    }
}

struct NoSecretVault;

#[async_trait]
impl DatasourceVault for NoSecretVault {
    async fn protection(&self) -> DsResult<ProtectionStatus> {
        Ok(ProtectionStatus::OwnerOnlyEncryptedFile)
    }
    async fn write(&self, _: DatasourceSecretRef, _: SecretValue) -> DsResult<()> {
        Err(ds_error(DsErrorCode::CredentialMissing))
    }
    async fn read(&self, _: DatasourceSecretRef) -> DsResult<SecretLease> {
        Err(ds_error(DsErrorCode::CredentialMissing))
    }
    async fn remove(&self, _: DatasourceSecretRef) -> DsResult<()> {
        Ok(())
    }
}

struct CountingFactory {
    opens: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
}

#[async_trait]
impl ConnectorFactory for CountingFactory {
    fn validate_config(&self, _: &DatasourceRevision) -> Vec<FieldIssue> {
        vec![]
    }
    async fn open(&self, _: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(CountingConnector {
            closes: self.closes.clone(),
            closed: AtomicBool::new(false),
        }))
    }
}

struct CountingConnector {
    closes: Arc<AtomicUsize>,
    closed: AtomicBool,
}

#[async_trait]
impl CatalogReader for CountingConnector {
    async fn observe_schema(&self, source: &SourceId) -> CoreResult<ObservedSchema> {
        Ok(ObservedSchema {
            source_id: source.clone(),
            kind: SchemaKnowledgeKind::Observed,
            relations: vec![],
            observed_at: Utc::now(),
        })
    }
}

#[async_trait]
impl QueryPreflightReader for CountingConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision: QueryPreflightDecision::Allowed,
            cost: QueryCostEstimate {
                estimated_cost_units: None,
                scanned_bytes: None,
                estimator_version: None,
            },
            reason_codes: vec![],
            warnings: vec![],
        })
    }
}

#[async_trait]
impl SqlQueryExecutor for CountingConnector {
    async fn execute_query(&self, _: QueryRequest) -> CoreResult<QueryResult> {
        Err(CoreError::validation("not_used", "not used"))
    }
}

#[async_trait]
impl FreshnessReader for CountingConnector {
    async fn read_freshness(
        &self,
        source: &SourceId,
        relation: &str,
        _: &str,
    ) -> CoreResult<FreshnessObservation> {
        Ok(FreshnessObservation {
            source_id: source.clone(),
            relation: relation.into(),
            observed_at: Utc::now(),
            data_as_of: None,
            lag_seconds: None,
        })
    }
}

#[async_trait]
impl ManagedConnector for CountingConnector {
    async fn probe(&self) -> DsResult<ProbeEvidence> {
        Ok(ProbeEvidence {
            authenticated: true,
            target_verified: true,
            read_only_verified: true,
            least_privilege_verified: true,
            capabilities_verified: true,
        })
    }
    async fn close(&self) -> DsResult<()> {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

struct OneCatalog {
    descriptor: ConnectorDescriptor,
    factory: Arc<dyn ConnectorFactory>,
}

impl ConnectorCatalog for OneCatalog {
    fn descriptors(&self) -> DsResult<Vec<ConnectorDescriptor>> {
        Ok(vec![self.descriptor.clone()])
    }
    fn factory(
        &self,
        id: &AdapterId,
        version: &AdapterVersion,
    ) -> DsResult<Arc<dyn ConnectorFactory>> {
        if id == &self.descriptor.adapter_id && version == &self.descriptor.adapter_version {
            Ok(self.factory.clone())
        } else {
            Err(ds_error(DsErrorCode::ConfigIncompatible))
        }
    }
}

#[tokio::test]
async fn manager_merges_full_identity_and_closes_after_last_run() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("fake.db");
    std::fs::write(&path, b"fixture").unwrap();
    let policy = Arc::new(file_policy(&path, &root));
    let workspace_id = WorkspaceId::new();
    let profile_id = ProfileId::new();
    let source = SourceId::new("managed_sqlite");
    let revision = DatasourceRevision::new(DatasourceRevisionInput {
        schema_version: 1,
        workspace_id,
        profile_id,
        revision: 1,
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        source_id: Some(source.clone()),
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
    .unwrap();
    let capability = CapabilityDescriptor {
        source_id: source,
        dialect: "sqlite".into(),
        catalog_reader: true,
        preflight_reader: true,
        sql_query_executor: true,
        freshness_reader: true,
        supports_explain: false,
        supports_read_only_tx: false,
        read_only_mechanism: Some(ReadOnlyMechanism::FileReadOnly),
        max_concurrency: 1,
    };
    let governance = policy.governance_for(&revision).unwrap();
    let inputs =
        DatasourceValidationInputs::new(&revision, &capability, governance.policy_digest).unwrap();
    let evidence = ValidationEvidence::new(
        inputs.clone(),
        "1".try_into().unwrap(),
        ProbeEvidence {
            authenticated: true,
            target_verified: true,
            read_only_verified: true,
            least_privilege_verified: true,
            capabilities_verified: true,
        },
        Utc::now(),
    )
    .unwrap();
    let run_a = RunId::new();
    let run_b = RunId::new();
    let scope = DatasourceScope {
        workspace_id,
        session_id: SessionId::new(),
    };
    let binding = |run| {
        RunDatasourceBinding::from_validated(run, scope, 1, &revision, &evidence, &inputs).unwrap()
    };
    let bindings = [(run_a, binding(run_a)), (run_b, binding(run_b))].into();
    let detail = DatasourceDetail {
        schema_version: 1,
        profile: DatasourceProfile {
            schema_version: 1,
            workspace_id,
            profile_id,
            source_id: revision.input().source_id.clone(),
            name: DatasourceName::new("Manager fixture").unwrap(),
            head_revision: NonZeroU64::new(1).unwrap(),
            deleted_at: None,
        },
        revision,
        state: RevisionState::Ready,
        validation: Some(evidence),
    };
    let repository = Arc::new(FakeRepository {
        detail: Mutex::new(detail),
        bindings,
    });
    let opens = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let descriptor = ConnectorDescriptor {
        schema_version: 1,
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "1".try_into().unwrap(),
        config_version: 1,
        contract_version: 1,
        display_name: "SQLite".into(),
        support: ConnectorSupport::Registered,
        fields: vec![],
        capability,
        max_connections: NonZeroU64::new(1).unwrap(),
        release_evidence: None,
    };
    let manager = Arc::new(ConnectorManager::new(
        repository.clone(),
        Arc::new(NoSecretVault),
        Arc::new(OneCatalog {
            descriptor,
            factory: Arc::new(CountingFactory {
                opens: opens.clone(),
                closes: closes.clone(),
            }),
        }),
        policy,
    ));
    let (a, b) = tokio::join!(manager.resolve(run_a), manager.resolve(run_b));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(Arc::ptr_eq(&a.connector, &b.connector));
    assert_eq!(opens.load(Ordering::SeqCst), 1);

    repository.detail.lock().unwrap().state = RevisionState::Invalid(DsErrorCode::ValidationStale);
    assert!(
        manager.resolve(run_a).await.is_ok(),
        "an existing Run keeps its lease"
    );
    manager.release(run_a).await.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 0);
    manager.release(run_b).await.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    manager.close().await.unwrap();
    assert_eq!(
        manager.resolve(run_a).await.err().expect("closed").code,
        DsErrorCode::Cancelled
    );
}

fn ds_error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: DsRemediation::Retry,
        operation_id: None,
    }
}

#[tokio::test]
async fn active_binding_source_requires_explicit_ready_session_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("binding.db");
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch("CREATE TABLE readings(value INTEGER); INSERT INTO readings VALUES (7)")
        .unwrap();
    drop(database);
    let repository = Arc::new(
        SqliteRuntimeStore::open(root.join("runtime.db"))
            .await
            .unwrap()
            .datasource_repository(),
    );
    let policy = Arc::new(file_policy(&path, &root));
    let catalog = Arc::new(
        BuiltinConnectorCatalog::new(
            Arc::new(PostgresConnectorFactory),
            Arc::new(DuckDbConnectorFactory),
        )
        .unwrap(),
    );
    let service = DatasourceService::new(
        repository.clone(),
        Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault"))),
        catalog.clone(),
        policy.clone(),
    );
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let saved = service
        .save(SaveDatasource {
            write: write(scope, 0, None),
            profile_id: None,
            name: DatasourceName::new("Binding source").unwrap(),
            adapter_id: "sqlite".try_into().unwrap(),
            adapter_version: "1".try_into().unwrap(),
            config_version: 1,
            fields: [(
                FieldId::new("database_path").unwrap(),
                FieldValue::Text(path.to_string_lossy().into_owned()),
            )]
            .into(),
            context: DatabaseContext::File {
                canonical_path: path,
            },
            secret: SecretEdit::Keep,
        })
        .await
        .unwrap();
    service
        .validate(ValidateDatasource {
            write: write(scope, 1, Some(1)),
            revision: saved.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: OperationId::new(),
        })
        .await
        .unwrap();
    service
        .select(SelectDatasource {
            write: write(scope, 2, Some(1)),
            revision: saved.revision.identity(),
            kind: DatasourceSelectionKind::Session,
        })
        .await
        .unwrap();

    let source =
        ActiveRunDatasourceBindingSource::new(repository.clone(), catalog.clone(), policy.clone());
    let binding = source
        .bind_new_run(RunId::new(), Some(scope), None)
        .await
        .unwrap();
    assert_eq!(binding.revision(), saved.revision.identity());
    assert_eq!(binding.selection_version(), 1);
    assert_eq!(
        source
            .bind_new_run(RunId::new(), None, None)
            .await
            .unwrap_err()
            .code,
        DsErrorCode::ValidationStale
    );
    let manager = ConnectorManager::new(repository, Arc::new(NoSecretVault), catalog, policy);
    let missing_binding = manager.resolve(RunId::new()).await;
    assert!(
        matches!(
            missing_binding,
            Err(DsError {
                code: DsErrorCode::ConfigIncompatible,
                ..
            })
        ),
        "a legacy/non-terminal Run without a durable datasource binding cannot recover"
    );
}

#[tokio::test]
async fn switched_runs_query_their_immutable_sources_after_manager_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path_a = root.join("source-a.db");
    let path_b = root.join("source-b.db");
    for (path, value) in [(&path_a, 11), (&path_b, 22)] {
        let database = rusqlite::Connection::open(path).unwrap();
        database
            .execute_batch(&format!(
                "CREATE TABLE readings(value INTEGER); INSERT INTO readings VALUES ({value})"
            ))
            .unwrap();
    }
    let policy = Arc::new(
        SourcePolicy::from_json_bytes(
            &serde_json::to_vec(&json!({
                "schema_version": 2,
                "allowed_sources": {
                    "source_a": {
                        "relations": {"readings": {"columns": {"value": "allow"}}},
                        "target": {"kind": "file", "adapter_id": "sqlite", "canonical_path": path_a, "allowed_roots": [root]}
                    },
                    "source_b": {
                        "relations": {"readings": {"columns": {"value": "allow"}}},
                        "target": {"kind": "file", "adapter_id": "sqlite", "canonical_path": path_b, "allowed_roots": [root]}
                    }
                }
            }))
            .unwrap(),
            QueryBudget::default(),
        )
        .unwrap(),
    );
    let store = Arc::new(
        SqliteRuntimeStore::open(root.join("runtime.db"))
            .await
            .unwrap(),
    );
    let repository = Arc::new(store.datasource_repository());
    let vault = Arc::new(LocalEncryptedDatasourceVault::new(root.join("vault")));
    let catalog = Arc::new(
        BuiltinConnectorCatalog::new(
            Arc::new(PostgresConnectorFactory),
            Arc::new(DuckDbConnectorFactory),
        )
        .unwrap(),
    );
    let management = DatasourceService::new(
        repository.clone(),
        vault.clone(),
        catalog.clone(),
        policy.clone(),
    );
    let bootstrap_scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
    };
    let mut version = 0;
    let mut revisions = Vec::new();
    for (name, path) in [("Source A", &path_a), ("Source B", &path_b)] {
        let saved = management
            .save(SaveDatasource {
                write: write(bootstrap_scope, version, None),
                profile_id: None,
                name: DatasourceName::new(name).unwrap(),
                adapter_id: "sqlite".try_into().unwrap(),
                adapter_version: "1".try_into().unwrap(),
                config_version: 1,
                fields: [(
                    FieldId::new("database_path").unwrap(),
                    FieldValue::Text(path.to_string_lossy().into_owned()),
                )]
                .into(),
                context: DatabaseContext::File {
                    canonical_path: path.clone(),
                },
                secret: SecretEdit::Keep,
            })
            .await
            .unwrap();
        version += 1;
        management
            .validate(ValidateDatasource {
                write: write(bootstrap_scope, version, Some(1)),
                revision: saved.revision.identity(),
                mode: ValidationMode::Connection,
                operation_id: OperationId::new(),
            })
            .await
            .unwrap();
        version += 1;
        revisions.push(saved.revision.identity());
    }
    management
        .select(SelectDatasource {
            write: write(bootstrap_scope, version, Some(1)),
            revision: revisions[0],
            kind: DatasourceSelectionKind::WorkspaceDefault,
        })
        .await
        .unwrap();
    version += 1;

    let artifacts = Arc::new(LocalArtifactStore::new(root.join("artifacts")).unwrap());
    let provider = provider_fixture::persisted_test_active_provider(store.as_ref()).await;
    let agent = InProcessAgentService::new(
        bootstrap_scope.workspace_id,
        store.clone(),
        artifacts,
        Arc::new(NoopRunScheduler),
    )
    .with_run_provider_binding_source(Arc::new(StaticRunProviderBindingSource::from_active(
        provider,
    )))
    .with_run_datasource_binding_source(Arc::new(ActiveRunDatasourceBindingSource::new(
        repository.clone(),
        catalog.clone(),
        policy.clone(),
    )));
    let session = agent
        .create_session(CommandId::new(), Principal::local_operator("runtime-test"))
        .await
        .unwrap();
    let run_a = match agent
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            "read source A",
        ))
        .await
        .unwrap()
    {
        ServiceReply::RunScheduled { run_id, .. } => run_id,
        other => panic!("unexpected service reply: {other:?}"),
    };
    let session_scope = DatasourceScope {
        workspace_id: bootstrap_scope.workspace_id,
        session_id: session.id,
    };
    management
        .select(SelectDatasource {
            write: write(session_scope, version, Some(1)),
            revision: revisions[1],
            kind: DatasourceSelectionKind::Session,
        })
        .await
        .unwrap();
    let run_b = match agent
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            "read source B",
        ))
        .await
        .unwrap()
    {
        ServiceReply::RunScheduled { run_id, .. } => run_id,
        other => panic!("unexpected service reply: {other:?}"),
    };
    assert_eq!(
        repository.load_run_binding(run_a).await.unwrap().revision(),
        revisions[0]
    );
    assert_eq!(
        repository.load_run_binding(run_b).await.unwrap().revision(),
        revisions[1]
    );

    let manager = Arc::new(ConnectorManager::new(
        repository.clone(),
        vault.clone(),
        catalog.clone(),
        policy.clone(),
    ));
    let registry = ConnectorRegistry::with_run_resolver(manager.clone());
    let binding_b = repository.load_run_binding(run_b).await.unwrap();
    let governance_b = policy
        .governance_for(
            &repository
                .load_revision(revisions[1])
                .await
                .unwrap()
                .revision,
        )
        .unwrap();
    let result_b = registry
        .resolve(run_b, binding_b.source_id())
        .await
        .unwrap()
        .query
        .execute_query(QueryRequest {
            source_id: binding_b.source_id().clone(),
            sql: "SELECT value FROM readings".into(),
            parameters: vec![],
            budget: governance_b.budget,
            query_tag: "run-b".into(),
            scope: governance_b.data_scope,
            confirmation_granted: false,
        })
        .await
        .unwrap();
    assert_eq!(result_b.rows[0][0], CellValue::Integer(22));
    manager.close().await.unwrap();

    let restarted = ConnectorManager::new(repository, vault, catalog, policy);
    let resolved_a = restarted.resolve(run_a).await.unwrap();
    let result_a = resolved_a
        .connector
        .execute_query(QueryRequest {
            source_id: resolved_a.context.binding.source_id().clone(),
            sql: "SELECT value FROM readings".into(),
            parameters: vec![],
            budget: resolved_a.context.query_budget.clone(),
            query_tag: "run-a-after-restart".into(),
            scope: resolved_a.context.data_scope.clone(),
            confirmation_granted: false,
        })
        .await
        .unwrap();
    assert_eq!(result_a.rows[0][0], CellValue::Integer(11));
    restarted.close().await.unwrap();
}
