use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use ys_agent_core::{
    OperationId, ProfileId, ProfileName, ProfileRevision, ProfileState, ProviderField, ProviderId,
    ProviderModelId, ProviderParameterKey, ProviderParameters, ProviderRemediation,
    RevisionPrecondition, SaveProfileRequest, SaveProfileRevision,
};
use ys_agent_runtime::provider::service::ProviderManagementService;
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

async fn profile_service(
    store: &SqliteRuntimeStore,
) -> (
    ProviderManagementService,
    ys_agent_store::SqliteProviderRepository,
) {
    let repository = store.provider_repository();
    (
        ProviderManagementService::new(Arc::new(repository.clone())),
        repository,
    )
}

fn draft(profile_id: ProfileId, revision: u64, model: &str) -> ProfileRevision {
    ProfileRevision::draft(
        profile_id,
        revision,
        ProviderId::DeepSeek,
        ProviderModelId::new(ProviderId::DeepSeek, model).expect("governed model"),
        ProviderParameters::default(),
        None,
    )
    .expect("valid Draft")
}

#[tokio::test]
async fn service_persists_drafts_loads_current_revision_after_restart_and_copies_without_credentials()
 {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open database");
    let (service, repository) = profile_service(&store).await;
    let profile_id = ProfileId::new();
    let original_name = ProfileName::new("Primary").expect("valid Profile name");

    let saved = service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id,
                    expected_current_revision: None,
                },
                name: original_name.clone(),
                revision: draft(profile_id, 1, "deepseek/first-model"),
            },
        })
        .await
        .expect("an incomplete configuration remains a saved Draft");
    assert_eq!(saved.revision, 1);
    assert_eq!(saved.summary.state, ProfileState::Draft);
    assert_eq!(saved.summary.name, "Primary");
    assert!(saved.credential_generation.is_none());
    assert!(saved.validation_id.is_none());

    let renamed = ProfileName::new("Primary edited").expect("valid Profile name");
    let edited = service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id,
                    expected_current_revision: Some(1),
                },
                name: renamed.clone(),
                revision: draft(profile_id, 2, "deepseek/second-model"),
            },
        })
        .await
        .expect("editing appends a new Draft revision");
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.model.as_str(), "deepseek/second-model");
    assert_eq!(
        repository
            .load_revision(profile_id, 1)
            .await
            .expect("immutable prior revision")
            .model()
            .as_str(),
        "deepseek/first-model"
    );

    let reopened_store = SqliteRuntimeStore::open(&database)
        .await
        .expect("reopen database");
    let (reopened, _) = profile_service(&reopened_store).await;
    let loaded = reopened
        .load_profile(profile_id)
        .await
        .expect("load the current revision after restart");
    assert_eq!(loaded.revision, 2);
    assert_eq!(loaded.summary.name, "Primary edited");

    let copied = reopened
        .copy_profile(
            profile_id,
            ProfileName::new("Copy").expect("valid Profile name"),
        )
        .await
        .expect("copy non-sensitive configuration only");
    assert_ne!(copied.summary.profile_id, profile_id);
    assert_eq!(copied.revision, 1);
    assert_eq!(copied.summary.state, ProfileState::Draft);
    assert_eq!(copied.model.as_str(), "deepseek/second-model");
    assert!(copied.credential_generation.is_none());
    assert!(copied.validation_id.is_none());
    assert_eq!(
        reopened.list_profiles().await.expect("offline list").len(),
        2
    );
}

#[tokio::test]
async fn service_returns_local_field_errors_without_replacing_a_saved_profile() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open database");
    let (service, _) = profile_service(&store).await;
    let existing_id = ProfileId::new();
    let existing_name = ProfileName::new("Primary").expect("valid Profile name");
    service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id: existing_id,
                    expected_current_revision: None,
                },
                name: existing_name.clone(),
                revision: draft(existing_id, 1, "deepseek/existing"),
            },
        })
        .await
        .expect("save incomplete Draft");

    let duplicate = service
        .copy_profile(existing_id, existing_name)
        .await
        .expect_err("duplicate name must be a local field error");
    assert_eq!(duplicate.code(), "provider.profile.name_conflict");
    assert_eq!(duplicate.field(), Some(&ProviderField::ProfileName));
    assert_eq!(duplicate.remediation(), ProviderRemediation::ReturnToEdit);
    assert_eq!(profile_count(&service).await, 1);

    let invalid_parameters: ProviderParameters = serde_json::from_value(json!({
        "temperature": 3.0,
        "max_tokens": null,
        "timeout_seconds": 30,
        "retry_count": 0,
        "provider_specific": {}
    }))
    .expect("typed parameter fixture");
    let invalid_id = ProfileId::new();
    let invalid = service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id: invalid_id,
                    expected_current_revision: None,
                },
                name: ProfileName::new("Invalid").expect("valid Profile name"),
                revision: ProfileRevision::draft(
                    invalid_id,
                    1,
                    ProviderId::DeepSeek,
                    ProviderModelId::new(ProviderId::DeepSeek, "deepseek/invalid")
                        .expect("governed model"),
                    invalid_parameters,
                    None,
                )
                .expect("Draft construction only checks structural invariants"),
            },
        })
        .await
        .expect_err("invalid parameters must not be persisted as a replacement revision");
    assert_eq!(invalid.code(), "provider.model.incompatible");
    assert_eq!(
        invalid.field(),
        Some(&ProviderField::Parameter(ProviderParameterKey::Temperature))
    );
    assert_eq!(invalid.remediation(), ProviderRemediation::ReturnToEdit);
    assert_eq!(profile_count(&service).await, 1);
}

#[tokio::test]
async fn copy_preserves_same_model_conditional_parameters_but_resets_validation_and_credential() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open database");
    let (service, repository) = profile_service(&store).await;
    let source_id = ProfileId::new();
    let mut parameters = ProviderParameters::default();
    parameters
        .set_temperature(Some(0.5))
        .expect("finite conditional parameter");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id: source_id,
                expected_current_revision: None,
            },
            name: ProfileName::new("Conditional source").expect("valid Profile name"),
            revision: ProfileRevision::draft(
                source_id,
                1,
                ProviderId::DeepSeek,
                ProviderModelId::new(ProviderId::DeepSeek, "deepseek/conditional")
                    .expect("governed model"),
                parameters,
                None,
            )
            .expect("structurally valid Draft"),
        })
        .await
        .expect("seed existing Draft");

    let copied = service
        .copy_profile(
            source_id,
            ProfileName::new("Conditional copy").expect("valid Profile name"),
        )
        .await
        .expect("copy keeps same-model conditional configuration for fresh validation");
    assert_eq!(copied.parameters.temperature(), Some(0.5));
    assert_eq!(copied.summary.state, ProfileState::Draft);
    assert!(copied.credential_generation.is_none());
    assert!(copied.validation_id.is_none());
}

async fn profile_count(service: &ProviderManagementService) -> usize {
    service
        .list_profiles()
        .await
        .expect("offline profile list")
        .len()
}

#[tokio::test]
async fn failed_profile_save_preserves_the_committed_active_snapshot() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open database");
    let active = provider_fixture::persisted_test_active_provider(&store).await;
    let (service, repository) = profile_service(&store).await;
    let current = repository
        .load_current_revision(active.profile_id())
        .await
        .expect("current active revision");
    let candidate = ProfileRevision::draft(
        active.profile_id(),
        current.revision() + 1,
        current.provider(),
        current.model().clone(),
        current.parameters().clone(),
        current.credential_generation(),
    )
    .expect("well-formed attempted edit");

    let error = service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id: active.profile_id(),
                    expected_current_revision: Some(current.revision() - 1),
                },
                name: ProfileName::new("conflicting edit").expect("valid Profile name"),
                revision: candidate,
            },
        })
        .await
        .expect_err("stale save must fail without changing committed state");
    assert_eq!(error.code(), "provider.storage.conflict");

    let after = service
        .active_snapshot()
        .await
        .expect("read current active snapshot")
        .expect("active snapshot remains available");
    assert_eq!(after.profile_id(), active.profile_id());
    assert_eq!(after.profile_revision(), active.profile_revision());
    assert_eq!(after.activation_revision(), active.activation_revision());
}

#[tokio::test]
async fn editing_an_active_profile_appends_a_draft_without_moving_the_active_revision() {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.path().join("runtime.db");
    let store = SqliteRuntimeStore::open(&database)
        .await
        .expect("open database");
    let active = provider_fixture::persisted_test_active_provider(&store).await;
    let (service, repository) = profile_service(&store).await;
    let current = repository
        .load_current_revision(active.profile_id())
        .await
        .expect("current active revision");
    let draft = ProfileRevision::draft(
        active.profile_id(),
        current.revision() + 1,
        current.provider(),
        current.model().clone(),
        current.parameters().clone(),
        current.credential_generation(),
    )
    .expect("well-formed edit Draft");

    let saved = service
        .save_profile(SaveProfileRequest {
            operation_id: OperationId::new(),
            revision: SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id: active.profile_id(),
                    expected_current_revision: Some(current.revision()),
                },
                name: ProfileName::new("Edited active Profile").expect("valid Profile name"),
                revision: draft,
            },
        })
        .await
        .expect("editing an active Profile saves a new Draft");
    assert_eq!(saved.summary.state, ProfileState::Draft);
    assert_eq!(saved.revision, current.revision() + 1);

    let after = service
        .active_snapshot()
        .await
        .expect("read active snapshot")
        .expect("existing active snapshot remains");
    assert_eq!(after.profile_id(), active.profile_id());
    assert_eq!(after.profile_revision(), active.profile_revision());
    assert_eq!(after.activation_revision(), active.activation_revision());
    assert_eq!(
        repository
            .load_revision(active.profile_id(), active.profile_revision())
            .await
            .expect("old active revision remains immutable")
            .state(),
        ProfileState::Ready
    );
}
