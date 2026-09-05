use ys_agent_core::*;

pub fn datasource_binding(run: RunId) -> RunDatasourceBinding {
    let workspace = WorkspaceId::new();
    let profile = ProfileId::new();
    let source = SourceId::new("contract-test");
    let revision = DatasourceRevision::new(DatasourceRevisionInput {
        schema_version: 1,
        workspace_id: workspace,
        profile_id: profile,
        revision: 1,
        adapter_id: "sqlite".try_into().unwrap(),
        adapter_version: "test".try_into().unwrap(),
        config_version: 1,
        source_id: Some(source.clone()),
        fields: Default::default(),
        context: DatabaseContext::Database {
            catalog: None,
            database: "contract".into(),
            schema: "main".into(),
        },
        credential: None,
    })
    .unwrap();
    let capability = CapabilityDescriptor {
        source_id: source,
        dialect: "sqlite".into(),
        catalog_reader: true,
        sql_query_executor: true,
        freshness_reader: true,
        supports_explain: false,
        supports_read_only_tx: false,
        max_concurrency: 1,
        preflight_reader: true,
        read_only_mechanism: Some(ReadOnlyMechanism::FileReadOnly),
    };
    let inputs = DatasourceValidationInputs::new(
        &revision,
        &capability,
        DatasourceDigest::of(&"test-policy").unwrap(),
    )
    .unwrap();
    let evidence = ValidationEvidence::new(
        inputs.clone(),
        "test".try_into().unwrap(),
        ProbeEvidence {
            authenticated: true,
            target_verified: true,
            read_only_verified: true,
            least_privilege_verified: true,
            capabilities_verified: true,
        },
        chrono::Utc::now(),
    )
    .unwrap();
    RunDatasourceBinding::from_validated(
        run,
        DatasourceScope {
            workspace_id: workspace,
            session_id: SessionId::new(),
        },
        1,
        &revision,
        &evidence,
        &inputs,
    )
    .unwrap()
}
