use ys_agent_core::*;

#[allow(dead_code)] // Shared by pure core contracts and persistence integration tests.
pub fn datasource_binding(run: RunId) -> RunDatasourceBinding {
    binding_in(run, WorkspaceId::new(), false).0
}

fn binding_in(
    run: RunId,
    workspace: WorkspaceId,
    secret: bool,
) -> (RunDatasourceBinding, DatasourceRevision) {
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
        credential: secret.then(|| DatasourceSecretRef::new(workspace, profile, 1).unwrap()),
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
    let binding = RunDatasourceBinding::from_validated(
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
    .unwrap();
    (binding, revision)
}

#[allow(dead_code)] // Store/runtime test fixtures must satisfy the same durable constraints.
pub async fn persisted_binding(
    repository: &dyn DatasourceRepository,
    run: RunId,
    workspace: WorkspaceId,
) -> RunDatasourceBinding {
    persist(repository, run, workspace, false).await
}

#[allow(dead_code)]
pub async fn persisted_secret_binding(
    repository: &dyn DatasourceRepository,
    run: RunId,
    workspace: WorkspaceId,
) -> RunDatasourceBinding {
    persist(repository, run, workspace, true).await
}

async fn persist(
    repository: &dyn DatasourceRepository,
    run: RunId,
    workspace: WorkspaceId,
    secret: bool,
) -> RunDatasourceBinding {
    let (binding, revision) = binding_in(run, workspace, secret);
    let scope = binding.scope();
    let profile = DatasourceProfile {
        schema_version: 1,
        workspace_id: workspace,
        profile_id: revision.identity().profile_id,
        source_id: revision.input().source_id.clone(),
        name: DatasourceName::new(format!("Contract {}", revision.identity().profile_id)).unwrap(),
        head_revision: revision.identity().revision,
        deleted_at: None,
    };
    let mut version = repository.load(scope).await.unwrap().version;
    let mutation_id = secret.then(OperationId::new);
    for change in [
        DatasourceChange::SaveRevision {
            profile,
            revision: revision.clone(),
            mutation_id,
        },
        DatasourceChange::Validation {
            revision: revision.identity(),
            state: RevisionState::Ready,
            evidence: Some(binding.evidence().clone()),
        },
        DatasourceChange::Selection {
            revision: revision.identity(),
            kind: DatasourceSelectionKind::Session,
        },
    ] {
        let expected_head_revision = if matches!(change, DatasourceChange::SaveRevision { .. }) {
            None
        } else {
            Some(revision.identity().revision)
        };
        let command = DatasourceCommit {
            schema_version: 1,
            write: DatasourceWriteContext {
                command_id: CommandId::new(),
                scope,
                expected_version: version,
                expected_head_revision,
            },
            command_digest: DatasourceDigest::of(&change).unwrap(),
            change,
        };
        if let DatasourceChange::SaveRevision {
            mutation_id: Some(id),
            ..
        } = command.change
        {
            for phase in [
                SecretMutationPhase::Prepared,
                SecretMutationPhase::VaultWritten,
            ] {
                let mutation = SecretMutation {
                    schema_version: 1,
                    mutation_id: id,
                    write: command.write,
                    profile_id: revision.identity().profile_id,
                    old: None,
                    new: revision.input().credential,
                    phase,
                    command_digest: command.command_digest.clone(),
                };
                repository
                    .commit(DatasourceCommit {
                        change: DatasourceChange::SecretJournal { mutation },
                        ..command.clone()
                    })
                    .await
                    .unwrap();
            }
        }
        version = repository
            .commit(command.clone())
            .await
            .unwrap()
            .committed_version;
        if let DatasourceChange::SaveRevision {
            mutation_id: Some(id),
            ..
        } = command.change
        {
            repository.finish_secret_mutation(id).await.unwrap();
        }
    }
    binding
}
