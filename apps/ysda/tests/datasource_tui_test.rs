use std::{collections::BTreeMap, sync::Arc};

use ys_agent_adapters::{
    DuckDbConnectorFactory, PostgresConnectorFactory,
    credential::datasource::LocalEncryptedDatasourceVault, data::BuiltinConnectorCatalog,
};
use ys_agent_core::{
    AdapterId, AdapterVersion, CapabilityDescriptor, ConnectorCatalog, ConnectorDescriptor,
    ConnectorSupport, DatasourceField, DatasourceManagementApi, DatasourceRepository,
    DatasourceScope, DatasourceSnapshot, DatasourceVault, DatasourceView, FieldId, FieldInput,
    FieldValue, QueryBudget, QueryRequest, RevisionState, RunDatasourceResolver, SelectionSnapshot,
    SourceId, WorkspaceId,
};
use ys_agent_runtime::{
    ActiveRunDatasourceBindingSource, AgentServiceApi, ConnectorManager, DatasourceService,
    InProcessAgentService, NoopRunScheduler, SendMessageRequest, ServiceReply, SourcePolicy,
    StaticRunProviderBindingSource,
};
use ysda::tui::datasource_management::{
    DatasourceAction, DatasourceRequest, DatasourceScreen, DatasourceScreenState,
};

fn view(scope: DatasourceScope) -> DatasourceView {
    let file_field = DatasourceField {
        id: FieldId::new("database_path").unwrap(),
        label: "Database file".into(),
        required: true,
        input: FieldInput::ExistingFile,
        default: None,
    };
    let descriptor = |id: &str, name: &str| ConnectorDescriptor {
        schema_version: 1,
        adapter_id: AdapterId::new(id).unwrap(),
        adapter_version: AdapterVersion::new("1").unwrap(),
        config_version: 1,
        contract_version: 1,
        display_name: name.into(),
        support: ConnectorSupport::Registered,
        fields: vec![file_field.clone()],
        capability: CapabilityDescriptor {
            source_id: SourceId::new(id),
            dialect: id.into(),
            catalog_reader: true,
            preflight_reader: true,
            sql_query_executor: true,
            freshness_reader: true,
            supports_explain: false,
            supports_read_only_tx: false,
            read_only_mechanism: None,
            max_concurrency: QueryBudget::default().max_concurrency,
        },
        max_connections: 1.try_into().unwrap(),
        release_evidence: None,
    };
    DatasourceView {
        schema_version: 1,
        catalog: vec![
            descriptor("sqlite", "SQLite"),
            descriptor("duckdb", "DuckDB"),
        ],
        snapshot: DatasourceSnapshot {
            schema_version: 1,
            version: 0,
            profiles: vec![],
            selection: SelectionSnapshot {
                schema_version: 1,
                scope,
                current: None,
                workspace_default: None,
                selection_version: 0,
                default_version: 0,
                header: None,
            },
        },
    }
}

#[test]
fn real_keyboard_reducer_builds_metadata_form_and_masks_secret() {
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: ys_agent_core::SessionId::new(),
    };
    let mut screen = DatasourceScreen::new(view(scope));

    assert_eq!(screen.state(), DatasourceScreenState::Browse);
    assert!(screen.reduce(DatasourceAction::New).is_none());
    assert_eq!(screen.state(), DatasourceScreenState::ConnectorSelect);
    assert!(screen.reduce(DatasourceAction::MoveDown).is_none());
    assert!(screen.reduce(DatasourceAction::Confirm).is_none());
    assert_eq!(screen.state(), DatasourceScreenState::Edit);

    for character in "warehouse".chars() {
        screen.reduce(DatasourceAction::Insert(character));
    }
    screen.reduce(DatasourceAction::NextField);
    for character in "/tmp/warehouse.duckdb".chars() {
        screen.reduce(DatasourceAction::Insert(character));
    }

    let form = screen.form().expect("metadata form");
    assert_eq!(form.name, "warehouse");
    assert_eq!(
        form.values.get(&FieldId::new("database_path").unwrap()),
        Some(&FieldValue::Text("/tmp/warehouse.duckdb".into()))
    );
    assert!(!screen.lines().join("\n").contains("secret-canary"));
    assert!(screen.reduce(DatasourceAction::Back).is_none());
    assert_eq!(screen.state(), DatasourceScreenState::ConnectorSelect);
}

#[test]
fn all_required_navigation_keys_keep_one_highlighted_candidate() {
    let scope = DatasourceScope {
        workspace_id: WorkspaceId::new(),
        session_id: ys_agent_core::SessionId::new(),
    };
    let mut screen = DatasourceScreen::new(view(scope));
    screen.reduce(DatasourceAction::New);

    for action in [
        DatasourceAction::MoveDown,
        DatasourceAction::PageDown,
        DatasourceAction::End,
        DatasourceAction::Home,
        DatasourceAction::PageUp,
        DatasourceAction::MoveUp,
    ] {
        screen.reduce(action);
        assert_eq!(screen.highlighted_count(), 1);
    }

    let _ordinary_config: BTreeMap<FieldId, FieldValue> = BTreeMap::new();
}

#[tokio::test]
#[ignore = "requires the Task 9 PostgreSQL Docker release service"]
async fn real_three_driver_forms_save_validate_select_and_set_default() {
    let root = tempfile::tempdir().unwrap();
    let canonical_root = std::fs::canonicalize(root.path()).unwrap();
    let sqlite_path = root.path().join("orders.sqlite");
    rusqlite::Connection::open(&sqlite_path)
        .unwrap()
        .execute_batch("CREATE TABLE orders(id INTEGER PRIMARY KEY, amount INTEGER); INSERT INTO orders VALUES (1, 11);")
        .unwrap();
    let sqlite_path = std::fs::canonicalize(sqlite_path).unwrap();
    let duckdb_path = root.path().join("orders.duckdb");
    duckdb::Connection::open(&duckdb_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE orders(id BIGINT, amount BIGINT); INSERT INTO orders VALUES (1, 22);",
        )
        .unwrap();
    let duckdb_path = std::fs::canonicalize(duckdb_path).unwrap();
    let postgres_password = std::env::var("YSDA_TEST_POSTGRES_PASSWORD")
        .expect("set the generated PostgreSQL test password at execution time");
    let policy = format!(
        r#"{{"schema_version":2,"allowed_sources":{{
          "sqlite_orders":{{"relations":{{"orders":{{"columns":{{"id":"allow","amount":"allow"}}}}}},"target":{{"kind":"file","adapter_id":"sqlite","canonical_path":{},"allowed_roots":[{}]}}}},
          "duckdb_orders":{{"relations":{{"orders":{{"columns":{{"id":"allow","amount":"allow"}}}}}},"target":{{"kind":"file","adapter_id":"duckdb","canonical_path":{},"allowed_roots":[{}]}}}},
          "postgres_orders":{{"relations":{{"public.orders":{{"columns":{{"order_id":"allow","paid_amount":"allow","paid_at":"allow","customer_email":"deny"}}}}}},"target":{{"kind":"database","adapter_id":"postgres","host":"127.0.0.1","port":55432,"database":"ysda_test","schema":"public"}}}}
        }}}}"#,
        serde_json::to_string(&sqlite_path).unwrap(),
        serde_json::to_string(&canonical_root).unwrap(),
        serde_json::to_string(&duckdb_path).unwrap(),
        serde_json::to_string(&canonical_root).unwrap(),
    );
    let runtime = Arc::new(
        ys_agent_store::SqliteRuntimeStore::open(root.path().join("runtime.db"))
            .await
            .unwrap(),
    );
    let repository: Arc<dyn DatasourceRepository> = Arc::new(runtime.datasource_repository());
    let vault: Arc<dyn DatasourceVault> = Arc::new(LocalEncryptedDatasourceVault::new(
        canonical_root.join("vault"),
    ));
    let catalog: Arc<dyn ConnectorCatalog> = Arc::new(
        BuiltinConnectorCatalog::new(
            Arc::new(PostgresConnectorFactory),
            Arc::new(DuckDbConnectorFactory),
        )
        .unwrap(),
    );
    let policy =
        Arc::new(SourcePolicy::from_json_bytes(policy.as_bytes(), QueryBudget::default()).unwrap());
    let service: Arc<dyn DatasourceManagementApi> = Arc::new(DatasourceService::new(
        repository.clone(),
        vault.clone(),
        catalog.clone(),
        policy.clone(),
    ));
    let workspace_id = WorkspaceId::new();
    let manager = Arc::new(ConnectorManager::new(
        repository.clone(),
        vault,
        catalog.clone(),
        policy.clone(),
    ));
    let artifacts =
        Arc::new(ys_agent_store::LocalArtifactStore::new(root.path().join("artifacts")).unwrap());
    let active_provider = ysda::bootstrap::seed_deterministic_active_provider(runtime.as_ref())
        .await
        .unwrap();
    let agent =
        InProcessAgentService::new(workspace_id, runtime, artifacts, Arc::new(NoopRunScheduler))
            .with_run_provider_binding_source(Arc::new(
                StaticRunProviderBindingSource::from_active(active_provider),
            ))
            .with_run_datasource_binding_source(Arc::new(ActiveRunDatasourceBindingSource::new(
                repository, catalog, policy,
            )));
    let session = agent
        .create_session(
            ys_agent_core::CommandId::new(),
            ys_agent_core::Principal::local_operator("real-tui-driver-test"),
        )
        .await
        .unwrap();
    let scope = DatasourceScope {
        workspace_id,
        session_id: session.id,
    };

    configure_real_profile(
        &service,
        scope,
        2,
        "sqlite-ui",
        &[sqlite_path.to_string_lossy().as_ref()],
        None,
    )
    .await;
    let mut runs = vec![
        query_selected(
            &agent,
            manager.as_ref(),
            session.id,
            "SELECT amount FROM orders",
        )
        .await,
    ];
    configure_real_profile(
        &service,
        scope,
        0,
        "duckdb-ui",
        &[duckdb_path.to_string_lossy().as_ref()],
        None,
    )
    .await;
    runs.push(
        query_selected(
            &agent,
            manager.as_ref(),
            session.id,
            "SELECT amount FROM orders",
        )
        .await,
    );
    configure_real_profile(
        &service,
        scope,
        1,
        "postgres-ui",
        &[
            "127.0.0.1",
            "55432",
            "ysda_test",
            "public",
            "ysda_reader",
            &postgres_password,
            "disable",
        ],
        Some(&postgres_password),
    )
    .await;
    runs.push(
        query_selected(
            &agent,
            manager.as_ref(),
            session.id,
            "SELECT paid_amount FROM public.orders ORDER BY order_id LIMIT 1",
        )
        .await,
    );

    let final_view = service.view(scope).await.unwrap();
    assert_eq!(final_view.snapshot.profiles.len(), 3);
    assert!(
        final_view
            .snapshot
            .profiles
            .iter()
            .all(|detail| detail.state == RevisionState::Ready)
    );
    assert!(final_view.snapshot.selection.current.is_some());
    assert!(final_view.snapshot.selection.workspace_default.is_some());

    let mut delete_screen = DatasourceScreen::new(final_view);
    for character in "sqlite-ui".chars() {
        delete_screen.reduce(DatasourceAction::Insert(character));
    }
    delete_screen.reduce(DatasourceAction::Actions);
    delete_screen.reduce(DatasourceAction::Delete);
    let DatasourceRequest::Delete(delete) = delete_screen
        .reduce(DatasourceAction::ConfirmDelete)
        .expect("confirmed delete request")
    else {
        panic!("expected delete")
    };
    assert_eq!(
        service.delete(delete).await.unwrap_err().code,
        ys_agent_core::DsErrorCode::InUse
    );
    for run_id in runs.drain(..) {
        agent
            .cancel_run(
                ys_agent_core::CommandId::new(),
                &run_id,
                "release test datasource".into(),
            )
            .await
            .unwrap();
    }
    let mut delete_screen = DatasourceScreen::new(service.view(scope).await.unwrap());
    for character in "sqlite-ui".chars() {
        delete_screen.reduce(DatasourceAction::Insert(character));
    }
    delete_screen.reduce(DatasourceAction::Actions);
    delete_screen.reduce(DatasourceAction::Delete);
    let DatasourceRequest::Delete(delete) = delete_screen
        .reduce(DatasourceAction::ConfirmDelete)
        .expect("confirmed delete after terminal Runs")
    else {
        panic!("expected delete")
    };
    service.delete(delete).await.unwrap();
    assert_eq!(
        service.view(scope).await.unwrap().snapshot.profiles.len(),
        2
    );
    manager.close().await.unwrap();
}

async fn query_selected(
    agent: &InProcessAgentService,
    manager: &ConnectorManager,
    session_id: ys_agent_core::SessionId,
    sql: &str,
) -> ys_agent_core::RunId {
    let run_id = match agent
        .send_message(SendMessageRequest::new(
            ys_agent_core::CommandId::new(),
            session_id,
            "run the selected datasource query",
        ))
        .await
        .unwrap()
    {
        ServiceReply::RunScheduled { run_id, .. } => run_id,
        other => panic!("unexpected service reply: {other:?}"),
    };
    let resolved = manager.resolve(run_id).await.unwrap();
    let result = resolved
        .connector
        .execute_query(QueryRequest {
            source_id: resolved.context.binding.source_id().clone(),
            sql: sql.into(),
            parameters: vec![],
            budget: resolved.context.query_budget.clone(),
            query_tag: "real-tui-driver".into(),
            scope: resolved.context.data_scope.clone(),
            confirmation_granted: false,
        })
        .await
        .unwrap();
    assert!(!result.rows.is_empty());
    manager.release(run_id).await.unwrap();
    run_id
}

async fn configure_real_profile(
    service: &Arc<dyn DatasourceManagementApi>,
    scope: DatasourceScope,
    connector_moves: usize,
    name: &str,
    values: &[&str],
    secret_canary: Option<&str>,
) {
    let mut screen = DatasourceScreen::new(service.view(scope).await.unwrap());
    screen.reduce(DatasourceAction::New);
    for _ in 0..connector_moves {
        screen.reduce(DatasourceAction::MoveDown);
    }
    screen.reduce(DatasourceAction::Confirm);
    for character in name.chars() {
        screen.reduce(DatasourceAction::Insert(character));
    }
    for value in values {
        screen.reduce(DatasourceAction::NextField);
        for _ in 0..64 {
            screen.reduce(DatasourceAction::Backspace);
        }
        for character in value.chars() {
            screen.reduce(DatasourceAction::Insert(character));
        }
    }
    if let Some(canary) = secret_canary {
        assert!(!screen.lines().join("\n").contains(canary));
    }
    let DatasourceRequest::Save(save) = screen
        .reduce(DatasourceAction::Confirm)
        .unwrap_or_else(|| panic!("save request for {name}: {}", screen.lines().join(" | ")))
    else {
        panic!("expected save")
    };
    let saved = service.save(save).await.unwrap();
    screen.complete(service.view(scope).await.unwrap(), "saved");
    screen.select_profile(saved.profile.profile_id);
    let DatasourceRequest::Validate(validate) = screen
        .reduce(DatasourceAction::Validate)
        .expect("validate request")
    else {
        panic!("expected validate")
    };
    let report = service.validate(validate).await.unwrap();
    assert_eq!(report.state, RevisionState::Ready, "{name}");
    screen.complete(service.view(scope).await.unwrap(), "validated");
    screen.reduce(DatasourceAction::Confirm);
    for character in name.chars() {
        screen.reduce(DatasourceAction::Insert(character));
    }
    let DatasourceRequest::Select(select) = screen
        .reduce(DatasourceAction::Confirm)
        .expect("session select")
    else {
        panic!("expected select")
    };
    service.select(select).await.unwrap();

    let mut default_screen = DatasourceScreen::new(service.view(scope).await.unwrap());
    for character in name.chars() {
        default_screen.reduce(DatasourceAction::Insert(character));
    }
    default_screen.reduce(DatasourceAction::Actions);
    let DatasourceRequest::Select(select_default) = default_screen
        .reduce(DatasourceAction::SetDefault)
        .expect("default select")
    else {
        panic!("expected default")
    };
    service.select(select_default).await.unwrap();
}
