use std::sync::Arc;

use tempfile::TempDir;
use ys_agent_adapters::credential::keyring::InMemoryCredentialVault;
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialMutation, CredentialMutationIntent,
    CredentialMutationRequest, CredentialVault, CredentialViewStatus, OperationId, ProfileId,
    ProfileName, ProfileRevision, ProtectedCredentialWrite, ProviderCredentialReference,
    ProviderErrorCode, ProviderId, ProviderManagementError, ProviderModelId, ProviderParameters,
    ProviderRemediation, ProviderResult, RevisionPrecondition, RunId, RunProviderBinding,
    RunProviderBindingRepository, SaveProfileRevision, SecretValue,
};
use ys_agent_runtime::provider::service::CredentialService;
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

struct RetainsCredentialForInFlightRun;

#[async_trait::async_trait]
impl RunProviderBindingRepository for RetainsCredentialForInFlightRun {
    async fn load_run_binding(&self, _run_id: RunId) -> ProviderResult<RunProviderBinding> {
        Err(ProviderManagementError::new(
            ProviderErrorCode::Internal,
            None,
            ProviderRemediation::ContactSupport,
        ))
    }

    async fn credential_status(
        &self,
        _credential: CredentialGeneration,
    ) -> ProviderResult<CredentialViewStatus> {
        Ok(CredentialViewStatus::Saved)
    }

    async fn has_nonterminal_profile_references(
        &self,
        _profile_id: ProfileId,
    ) -> ProviderResult<bool> {
        Ok(false)
    }

    async fn has_nonterminal_credential_references(
        &self,
        _credential: CredentialGeneration,
    ) -> ProviderResult<bool> {
        Ok(true)
    }
}

fn api_key_write(
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

fn credential_reference(
    profile_id: ProfileId,
    generation: CredentialGeneration,
) -> ProviderCredentialReference {
    ProviderCredentialReference {
        profile_id,
        generation,
    }
}

async fn seed_draft(
    repository: &ys_agent_store::SqliteProviderRepository,
    profile_id: ProfileId,
    name: &str,
) {
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new(name).expect("valid Profile name"),
            revision: ProfileRevision::draft(
                profile_id,
                1,
                ProviderId::DeepSeek,
                ProviderModelId::new(ProviderId::DeepSeek, "deepseek/test-model")
                    .expect("governed model"),
                ProviderParameters::default(),
                None,
            )
            .expect("valid Draft"),
        })
        .await
        .expect("seed Profile Draft");
}

#[tokio::test]
async fn api_key_creation_records_the_journal_writes_the_vault_then_appends_a_draft_revision() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Credentialed Profile").await;

    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let generation = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
        .expect("first API-key generation");
    let detail = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, generation)
                .expect("valid creation intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                generation,
                "credential-canary-must-not-leak",
            )),
        })
        .await
        .expect("credential creation completes as one visible new Draft");

    assert_eq!(detail.revision, 2);
    assert_eq!(detail.credential_generation, Some(generation));
    assert_eq!(
        detail.summary.credential_status,
        CredentialViewStatus::Saved
    );
    assert!(detail.validation_id.is_none());
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("load current revision")
            .credential_generation(),
        Some(generation)
    );
    assert_eq!(
        vault
            .credential_status(ProviderCredentialReference {
                profile_id,
                generation,
            })
            .await
            .expect("read masked Vault status"),
        CredentialViewStatus::Saved
    );
    assert!(
        repository
            .pending_credential_mutations()
            .await
            .expect("journal completes")
            .is_empty()
    );
}

#[tokio::test]
async fn replace_and_delete_are_profile_scoped_and_clean_unreferenced_generations() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let primary = ProfileId::new();
    let other = ProfileId::new();
    seed_draft(&repository, primary, "Primary").await;
    seed_draft(&repository, other, "Other").await;
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );

    let first =
        CredentialGeneration::new(primary, 1, CredentialKind::ApiKey).expect("first generation");
    service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), primary, 1, first)
                .expect("valid create intent"),
            mutation: CredentialMutation::Replace(api_key_write(primary, first, "primary-one")),
        })
        .await
        .expect("create primary credential");

    let other_first = CredentialGeneration::new(other, 1, CredentialKind::ApiKey)
        .expect("other first generation");
    service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), other, 1, other_first)
                .expect("valid isolated create intent"),
            mutation: CredentialMutation::Replace(api_key_write(other, other_first, "other-one")),
        })
        .await
        .expect("create other Profile credential");

    let replacement = CredentialGeneration::new(primary, 2, CredentialKind::ApiKey)
        .expect("replacement generation");
    let replaced = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::replace(
                OperationId::new(),
                primary,
                2,
                first,
                replacement,
            )
            .expect("valid replacement intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                primary,
                replacement,
                "primary-two",
            )),
        })
        .await
        .expect("replace only the primary credential");
    assert_eq!(replaced.revision, 3);
    assert_eq!(replaced.credential_generation, Some(replacement));
    assert_eq!(
        vault
            .credential_status(credential_reference(primary, first))
            .await
            .expect("old primary status"),
        CredentialViewStatus::Missing,
        "an unreferenced prior generation is cleaned"
    );
    assert_eq!(
        vault
            .credential_status(credential_reference(other, other_first))
            .await
            .expect("other Profile remains isolated"),
        CredentialViewStatus::Saved
    );

    let rollback = CredentialGeneration::new(primary, 3, CredentialKind::ApiKey)
        .expect("delete rollback generation");
    let deleted = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::delete(
                OperationId::new(),
                primary,
                3,
                replacement,
                rollback,
            )
            .expect("valid delete intent"),
            mutation: CredentialMutation::Delete,
        })
        .await
        .expect("delete primary credential without affecting the other Profile");
    assert_eq!(deleted.revision, 4);
    assert!(deleted.credential_generation.is_none());
    assert_eq!(
        deleted.summary.credential_status,
        CredentialViewStatus::Missing
    );
    assert_eq!(
        vault
            .credential_status(credential_reference(primary, replacement))
            .await
            .expect("deleted primary generation"),
        CredentialViewStatus::Missing
    );
    assert_eq!(
        vault
            .credential_status(credential_reference(primary, rollback))
            .await
            .expect("temporary rollback copy is cleaned"),
        CredentialViewStatus::Missing
    );
    assert_eq!(
        vault
            .credential_status(credential_reference(other, other_first))
            .await
            .expect("deleting one Profile cannot read or remove another generation"),
        CredentialViewStatus::Saved
    );
    assert!(
        repository
            .pending_credential_mutations()
            .await
            .expect("all mutations complete")
            .is_empty()
    );
}

#[tokio::test]
async fn failed_vault_write_rolls_back_the_journal_and_leaves_the_prior_draft_visible() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Fault injected").await;
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");
    assert_eq!(
        vault
            .protection_status()
            .await
            .expect("complete the native protection probe before fault injection"),
        ys_agent_core::CredentialProtectionStatus::ConfirmedNative
    );
    vault.fail_next(ys_agent_adapters::credential::keyring::InMemoryVaultOperation::Write);

    let error = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, generation)
                .expect("valid create intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                generation,
                "fault-injected-canary",
            )),
        })
        .await
        .expect_err("Vault write failure must not advance the Profile pointer");
    assert_eq!(error.code(), "provider.credential.protection_unavailable");
    assert!(
        !format!("{error:?}").contains("fault-injected-canary"),
        "stable error output must not retain the failed secret"
    );
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("prior revision remains current")
            .revision(),
        1
    );
    assert!(vault.stored_accounts().is_empty());
    assert!(
        repository
            .pending_credential_mutations()
            .await
            .expect("rolled-back journal is terminal")
            .is_empty()
    );
}

#[tokio::test]
async fn unconfirmed_native_protection_blocks_the_profile_without_writing_a_secret() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::with_protection(
        ys_agent_core::CredentialProtectionStatus::Unconfirmed,
    ));
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Unconfirmed protection").await;
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");

    let error = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, generation)
                .expect("valid create intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                generation,
                "never-written-canary",
            )),
        })
        .await
        .expect_err("unconfirmed protection must fail closed");
    assert_eq!(error.code(), "provider.credential.protection_unavailable");
    assert!(vault.stored_accounts().is_empty());
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("the existing Draft is unchanged")
            .credential_generation(),
        None
    );
    let pending = repository
        .pending_credential_mutations()
        .await
        .expect("blocked state is durable");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].blocks_profile_use());
}

#[tokio::test]
async fn stale_credential_intent_is_rejected_before_it_can_write_or_append_a_revision() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Stale request").await;
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let generation =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");

    let error = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 2, generation)
                .expect("structurally valid late intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                generation,
                "late-write-canary",
            )),
        })
        .await
        .expect_err("a late intent cannot overwrite a newer or different current revision");
    assert_eq!(error.code(), "provider.operation.stale");
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("current Draft remains")
            .revision(),
        1
    );
    assert!(vault.stored_accounts().is_empty());
    assert!(
        repository
            .pending_credential_mutations()
            .await
            .expect("no intent is journaled for a stale operation")
            .is_empty()
    );
}

#[tokio::test]
async fn replacement_retains_a_generation_still_pinned_by_the_active_snapshot() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let active = provider_fixture::persisted_test_active_provider(&store).await;
    let repository = store.provider_repository();
    let current = repository
        .load_current_revision(active.profile_id())
        .await
        .expect("load active revision");
    let prior_generation = current
        .credential_generation()
        .expect("active revision has a Credential");
    let vault = Arc::new(InMemoryCredentialVault::new());
    vault
        .write_generation(api_key_write(
            active.profile_id(),
            prior_generation,
            "active-pinned-key",
        ))
        .await
        .expect("seed protected active generation");
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let replacement = CredentialGeneration::new(
        active.profile_id(),
        prior_generation.number() + 1,
        CredentialKind::ApiKey,
    )
    .expect("next generation");

    let detail = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::replace(
                OperationId::new(),
                active.profile_id(),
                current.revision(),
                prior_generation,
                replacement,
            )
            .expect("valid replacement intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                active.profile_id(),
                replacement,
                "replacement-key",
            )),
        })
        .await
        .expect("replace current Draft without moving the active snapshot");
    assert_eq!(detail.revision, current.revision() + 1);
    assert_eq!(detail.credential_generation, Some(replacement));
    assert_eq!(
        vault
            .credential_status(credential_reference(active.profile_id(), prior_generation))
            .await
            .expect("active-pinned generation status"),
        CredentialViewStatus::Saved
    );
    assert_eq!(
        repository
            .active()
            .await
            .expect("read active snapshot")
            .expect("active snapshot remains")
            .profile_revision(),
        current.revision()
    );
}

#[tokio::test]
async fn replacement_retains_a_generation_still_pinned_by_an_inflight_run() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Run retained").await;
    let setup = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let first =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");
    setup
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, first)
                .expect("valid initial intent"),
            mutation: CredentialMutation::Replace(api_key_write(profile_id, first, "run-key-one")),
        })
        .await
        .expect("create first generation");
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(RetainsCredentialForInFlightRun),
        vault.clone(),
    );
    let replacement = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("replacement generation");

    service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::replace(
                OperationId::new(),
                profile_id,
                2,
                first,
                replacement,
            )
            .expect("valid replacement intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                replacement,
                "run-key-two",
            )),
        })
        .await
        .expect("replace while a Run still holds the previous generation");
    assert_eq!(
        vault
            .credential_status(credential_reference(profile_id, first))
            .await
            .expect("in-flight Run generation remains available"),
        CredentialViewStatus::Saved
    );
}

#[tokio::test]
async fn failed_old_generation_cleanup_leaves_a_durable_fail_closed_profile_state() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let vault = Arc::new(InMemoryCredentialVault::new());
    let profile_id = ProfileId::new();
    seed_draft(&repository, profile_id, "Cleanup fault").await;
    let service = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let first =
        CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey).expect("first generation");
    service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::create(OperationId::new(), profile_id, 1, first)
                .expect("valid initial intent"),
            mutation: CredentialMutation::Replace(api_key_write(profile_id, first, "cleanup-one")),
        })
        .await
        .expect("create first generation");
    let replacement = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("replacement generation");
    vault.fail_next(ys_agent_adapters::credential::keyring::InMemoryVaultOperation::Delete);

    let error = service
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::replace(
                OperationId::new(),
                profile_id,
                2,
                first,
                replacement,
            )
            .expect("valid replacement intent"),
            mutation: CredentialMutation::Replace(api_key_write(
                profile_id,
                replacement,
                "cleanup-two",
            )),
        })
        .await
        .expect_err("an uncertain post-CAS cleanup must fail closed");
    assert_eq!(error.code(), "provider.credential.protection_unavailable");
    assert_eq!(
        repository
            .load_current_revision(profile_id)
            .await
            .expect("the complete new revision is durable")
            .credential_generation(),
        Some(replacement)
    );
    let pending = repository
        .pending_credential_mutations()
        .await
        .expect("blocked journal is durable");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].blocks_profile_use());
    assert_eq!(
        repository
            .list_profiles()
            .await
            .expect("masked status remains readable")[0]
            .credential_status,
        CredentialViewStatus::Revoked
    );
}
