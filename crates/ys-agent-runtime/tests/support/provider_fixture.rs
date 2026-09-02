pub async fn persisted_test_active_provider(
    store: &ys_agent_store::SqliteRuntimeStore,
) -> ys_agent_core::ActiveProviderSnapshot {
    let repository = store.provider_repository();
    if let Some(active) = repository
        .active()
        .await
        .expect("load test active Provider")
    {
        return active;
    }

    let profile_id = ys_agent_core::ProfileId::new();
    let name =
        ys_agent_core::ProfileName::new("Runtime Fake Provider").expect("valid test Profile name");
    let model = ys_agent_core::ProviderModelId::new(
        ys_agent_core::ProviderId::DeepSeek,
        "deepseek/test-model",
    )
    .expect("valid test Provider model");
    repository
        .save_revision(ys_agent_core::SaveProfileRevision {
            precondition: ys_agent_core::RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name,
            revision: ys_agent_core::ProfileRevision::draft(
                profile_id,
                1,
                ys_agent_core::ProviderId::DeepSeek,
                model.clone(),
                ys_agent_core::ProviderParameters::default(),
                None,
            )
            .expect("valid initial test revision"),
        })
        .await
        .expect("save initial test Profile");

    let credential = ys_agent_core::CredentialGeneration::new(
        profile_id,
        1,
        ys_agent_core::CredentialKind::ApiKey,
    )
    .expect("valid test Credential generation");
    let mutation_id = ys_agent_core::OperationId::new();
    repository
        .begin_credential_mutation(
            ys_agent_core::CredentialMutationIntent::create(mutation_id, profile_id, 1, credential)
                .expect("valid test Credential intent"),
        )
        .await
        .expect("begin test Credential mutation");
    repository
        .record_credential_vault_write(mutation_id)
        .await
        .expect("record deterministic protected generation");
    let candidate = ys_agent_core::ProfileRevision::draft(
        profile_id,
        2,
        ys_agent_core::ProviderId::DeepSeek,
        model,
        ys_agent_core::ProviderParameters::default(),
        Some(credential),
    )
    .expect("valid credential-backed test revision");
    repository
        .commit_credential_pointer(
            ys_agent_core::CredentialPointerCommit::new(
                mutation_id,
                profile_id,
                1,
                candidate.clone(),
            )
            .expect("valid test Credential pointer"),
        )
        .await
        .expect("commit test Credential pointer");
    repository
        .complete_credential_mutation(mutation_id)
        .await
        .expect("complete test Credential mutation");

    let evidence = ys_agent_core::CompatibilityEvidence::passing(candidate.validation_inputs(
        ys_agent_core::ValidationVersions::new(
            "test-catalog",
            "test-probe",
            "test-liter",
            "test-codec",
        ),
    ));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    repository
        .save_validation(ys_agent_core::ValidationCommit {
            precondition: ys_agent_core::ValidationCommitPrecondition {
                operation_id: ys_agent_core::OperationId::new(),
                profile_id,
                revision: 2,
                credential_generation: credential,
                validation_digest: validation_digest.clone(),
            },
            evidence,
        })
        .await
        .expect("save test Provider evidence");
    repository
        .activate(ys_agent_core::ActivateProfileRequest {
            operation_id: ys_agent_core::OperationId::new(),
            precondition: ys_agent_core::ActivationPrecondition {
                profile_id,
                revision: 2,
                validation_id,
                validation_digest,
                expected_activation_revision: None,
            },
        })
        .await
        .expect("activate test Provider")
}
