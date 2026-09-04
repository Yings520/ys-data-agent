//! End-to-end persistence and recovery contracts for Provider management.

use std::sync::Arc;

use tempfile::TempDir;
use ys_agent_adapters::credential::memory::InMemoryCredentialVault;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ActiveRevisionPrecondition,
    CompatibilityEvidence, CredentialGeneration, CredentialKind, CredentialMutation,
    CredentialMutationIntent, CredentialMutationRequest, CredentialProtectionStatus,
    CredentialVault, CredentialViewStatus, DeleteProfileRequest, OperationId, ProfileId,
    ProfileName, ProfileRevision, ProfileState, ProtectedCredentialWrite,
    ProviderCredentialReference, ProviderErrorCode, ProviderId, ProviderModelId,
    ProviderParameters, RevisionPrecondition, RunId, RunProviderBindingSource, SaveProfileRequest,
    SaveProfileRevision, SecretValue, ValidationCommit, ValidationCommitPrecondition,
    ValidationVersions,
};
use ys_agent_runtime::{
    UnavailableRunProviderBindingSource,
    provider::service::{CredentialService, ProviderManagementService},
};
use ys_agent_store::SqliteRuntimeStore;

fn initial_draft(profile_id: ProfileId) -> ProfileRevision {
    ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, "deepseek/e2e-model").expect("governed model"),
        ProviderParameters::default(),
        None,
    )
    .expect("initial Draft")
}

fn credential_write(
    profile_id: ProfileId,
    generation: CredentialGeneration,
    value: &str,
) -> ProtectedCredentialWrite {
    ProtectedCredentialWrite {
        reference: ProviderCredentialReference {
            profile_id,
            generation,
        },
        secret: SecretValue::from_utf8(value.to_owned()),
    }
}

fn create_credential_request(
    operation_id: OperationId,
    profile_id: ProfileId,
    generation: CredentialGeneration,
    value: &str,
) -> CredentialMutationRequest {
    CredentialMutationRequest {
        intent: CredentialMutationIntent::create(operation_id, profile_id, 1, generation)
            .expect("creation intent"),
        mutation: CredentialMutation::Replace(credential_write(profile_id, generation, value)),
    }
}

#[tokio::test]
async fn profile_lifecycle_recovers_from_restart_without_losing_the_active_snapshot() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    let generation = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("first credential generation");

    {
        let store = SqliteRuntimeStore::open(&database)
            .await
            .expect("open empty Provider store");
        let repository = store.provider_repository();
        let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
        let saved = profiles
            .save_profile(SaveProfileRequest {
                operation_id: OperationId::new(),
                revision: SaveProfileRevision {
                    precondition: RevisionPrecondition {
                        profile_id,
                        expected_current_revision: None,
                    },
                    name: ProfileName::new("E2E primary").expect("Profile name"),
                    revision: initial_draft(profile_id),
                },
            })
            .await
            .expect("an incomplete profile persists as a Draft");
        assert_eq!(saved.summary.state, ProfileState::Draft);
        assert_eq!(
            saved.summary.credential_status,
            CredentialViewStatus::Missing
        );
        assert!(
            profiles
                .active_snapshot()
                .await
                .expect("active snapshot")
                .is_none()
        );

        let credentials = CredentialService::new(
            Arc::new(repository.clone()),
            Arc::new(store.run_binding_repository()),
            vault.clone(),
        );
        let credentialed = credentials
            .mutate(create_credential_request(
                OperationId::new(),
                profile_id,
                generation,
                "e2e-fixture-credential",
            ))
            .await
            .expect("protected credential write appends a Draft");
        assert_eq!(credentialed.revision, 2);
        assert_eq!(credentialed.credential_generation, Some(generation));
        assert_eq!(credentialed.summary.state, ProfileState::Draft);
        assert!(
            repository
                .pending_credential_mutations()
                .await
                .expect("complete journal")
                .is_empty()
        );
    }

    let (active, copy_id) = {
        let store = SqliteRuntimeStore::open(&database)
            .await
            .expect("restart after credential mutation");
        let repository = store.provider_repository();
        let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
        let recovered = profiles
            .load_profile(profile_id)
            .await
            .expect("reload exact current revision after restart");
        assert_eq!(recovered.revision, 2);
        assert_eq!(recovered.credential_generation, Some(generation));
        assert_eq!(
            recovered.summary.credential_status,
            CredentialViewStatus::Saved
        );

        let copied = profiles
            .copy_profile(
                profile_id,
                ProfileName::new("E2E copy").expect("copy Profile name"),
            )
            .await
            .expect("copy only non-sensitive configuration");
        assert_eq!(copied.summary.state, ProfileState::Draft);
        assert!(copied.credential_generation.is_none());
        assert!(copied.validation_id.is_none());

        let current = repository
            .load_current_revision(profile_id)
            .await
            .expect("current credential-backed Draft");
        let versions =
            ValidationVersions::new("e2e-catalog", "e2e-probe", "e2e-liter", "e2e-codec");
        let evidence = CompatibilityEvidence::passing(current.validation_inputs(versions.clone()));
        let validation_id = evidence.id();
        let validation_digest = evidence.digest();
        let ready = profiles
            .commit_validation(ValidationCommit {
                precondition: ValidationCommitPrecondition {
                    operation_id: OperationId::new(),
                    profile_id,
                    revision: current.revision(),
                    credential_generation: generation,
                    validation_digest: validation_digest.clone(),
                },
                evidence,
                versions,
            })
            .await
            .expect("model evidence makes only the current revision Ready");
        assert_eq!(ready.summary.state, ProfileState::Ready);

        profiles
            .activate(ActivateProfileRequest {
                operation_id: OperationId::new(),
                precondition: ActivationPrecondition {
                    profile_id,
                    revision: current.revision(),
                    validation_id,
                    validation_digest,
                    expected_activation_revision: None,
                },
            })
            .await
            .expect("explicit activation commits one active snapshot");
        (
            profiles
                .active_snapshot()
                .await
                .expect("committed active snapshot")
                .expect("activated Profile"),
            copied.summary.profile_id,
        )
    };

    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("restart after activation");
    let repository = store.provider_repository();
    let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
    let recovered_active = profiles
        .active_snapshot()
        .await
        .expect("reload active snapshot")
        .expect("active Profile persists");
    assert_eq!(recovered_active.profile_id(), active.profile_id());
    assert_eq!(
        recovered_active.profile_revision(),
        active.profile_revision(),
        "restart cannot reinterpret the active revision"
    );
    assert_eq!(recovered_active.credential_generation(), generation);

    let run_bindings = store.run_binding_repository();
    profiles
        .delete_profile(
            DeleteProfileRequest {
                operation_id: OperationId::new(),
                profile_id,
                expected_revision: recovered_active.profile_revision(),
                expected_active: Some(ActiveRevisionPrecondition {
                    profile_id,
                    revision: recovered_active.profile_revision(),
                    activation_revision: recovered_active.activation_revision(),
                }),
                enter_no_active_provider: true,
            },
            vault.as_ref(),
            &run_bindings,
        )
        .await
        .expect("confirmed active deletion removes the exact protected generation");
    assert!(
        profiles
            .active_snapshot()
            .await
            .expect("no-active state")
            .is_none()
    );
    profiles
        .delete_profile(
            DeleteProfileRequest {
                operation_id: OperationId::new(),
                profile_id: copy_id,
                expected_revision: 1,
                expected_active: None,
                enter_no_active_provider: false,
            },
            vault.as_ref(),
            &run_bindings,
        )
        .await
        .expect(
            "non-active credential-free copy deletes after an intentional no-active transition",
        );
    assert_eq!(
        vault
            .credential_status(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await
            .expect("masked Vault status after deletion"),
        CredentialViewStatus::Missing
    );
    assert_eq!(
        UnavailableRunProviderBindingSource
            .bind_new_run(RunId::new())
            .await
            .expect_err("new Query cannot choose the copied Draft")
            .code(),
        ProviderErrorCode::NoActiveProfile.as_str()
    );
    assert!(
        profiles
            .list_profiles()
            .await
            .expect("all Profiles removed")
            .is_empty()
    );
}

#[tokio::test]
async fn blocked_credential_journal_survives_restart_and_keeps_future_mutations_fail_closed() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let profile_id = ProfileId::new();
    let generation = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("first credential generation");

    {
        let store = SqliteRuntimeStore::open(&database)
            .await
            .expect("open empty Provider store");
        let repository = store.provider_repository();
        let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
        profiles
            .save_profile(SaveProfileRequest {
                operation_id: OperationId::new(),
                revision: SaveProfileRevision {
                    precondition: RevisionPrecondition {
                        profile_id,
                        expected_current_revision: None,
                    },
                    name: ProfileName::new("Blocked recovery").expect("Profile name"),
                    revision: initial_draft(profile_id),
                },
            })
            .await
            .expect("save initial Draft");
        let unavailable_vault = Arc::new(InMemoryCredentialVault::with_protection(
            CredentialProtectionStatus::Unconfirmed,
        ));
        let credentials = CredentialService::new(
            Arc::new(repository.clone()),
            Arc::new(store.run_binding_repository()),
            unavailable_vault,
        );
        let error = credentials
            .mutate(create_credential_request(
                OperationId::new(),
                profile_id,
                generation,
                "never-persisted-fixture",
            ))
            .await
            .expect_err("unconfirmed protection must block rather than partially write");
        assert_eq!(
            error.code(),
            ProviderErrorCode::CredentialProtectionUnavailable.as_str()
        );
    }

    let reopened = SqliteRuntimeStore::open(&database)
        .await
        .expect("restart with a durable blocked journal");
    let repository = reopened.provider_repository();
    let pending = repository
        .pending_credential_mutations()
        .await
        .expect("read persisted reconciliation state");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].blocks_profile_use());
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("original Draft remains durable")
            .credential_generation(),
        None
    );

    let confirmed_vault = Arc::new(InMemoryCredentialVault::new());
    let credentials = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(reopened.run_binding_repository()),
        confirmed_vault.clone(),
    );
    let retry = credentials
        .mutate(create_credential_request(
            OperationId::new(),
            profile_id,
            generation,
            "must-not-bypass-reconciliation",
        ))
        .await
        .expect_err("restart cannot bypass unresolved recovery state");
    assert_eq!(
        retry.code(),
        ProviderErrorCode::CredentialProtectionUnavailable.as_str()
    );
    assert!(confirmed_vault.stored_accounts().is_empty());
}
