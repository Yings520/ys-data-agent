use std::sync::Arc;

use tempfile::TempDir;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, CompatibilityEvidence, OperationId, ProfileId,
    ProfileRevision, ProfileState, ProviderErrorCode, RevisionPrecondition, SaveProfileRevision,
    ValidationCommit, ValidationCommitPrecondition, ValidationVersions,
};
use ys_agent_runtime::provider::service::ProviderManagementService;
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

async fn append_credential_backed_draft(
    repository: &ys_agent_store::SqliteProviderRepository,
    profile_id: ProfileId,
    name: &str,
) -> (ProfileRevision, ProfileRevision) {
    let previous = repository
        .load_current_revision(profile_id)
        .await
        .expect("load current revision");
    let candidate = ProfileRevision::draft(
        previous.profile_id(),
        previous.revision() + 1,
        previous.provider(),
        previous.model().clone(),
        previous.parameters().clone(),
        previous.credential_generation(),
    )
    .expect("credential-backed Draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: Some(previous.revision()),
            },
            name: ys_agent_core::ProfileName::new(name).expect("valid Profile name"),
            revision: candidate.clone(),
        })
        .await
        .expect("append candidate Draft");
    (previous, candidate)
}

fn validation_commit(
    candidate: &ProfileRevision,
    operation_id: OperationId,
    evidence: CompatibilityEvidence,
    versions: ValidationVersions,
) -> ValidationCommit {
    ValidationCommit {
        precondition: ValidationCommitPrecondition {
            operation_id,
            profile_id: candidate.profile_id(),
            revision: candidate.revision(),
            credential_generation: candidate
                .credential_generation()
                .expect("candidate credential"),
            validation_digest: evidence.digest(),
        },
        evidence,
        versions,
    }
}

#[tokio::test]
async fn service_commits_current_validation_then_confirms_and_activates_only_the_new_revision() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let previous_active = provider_fixture::persisted_test_active_provider(&store).await;
    let repository = store.provider_repository();
    let service = ProviderManagementService::new(Arc::new(repository.clone()));
    let previous = repository
        .load_current_revision(previous_active.profile_id())
        .await
        .expect("load previous active revision");
    let candidate = ProfileRevision::draft(
        previous.profile_id(),
        previous.revision() + 1,
        previous.provider(),
        previous.model().clone(),
        previous.parameters().clone(),
        previous.credential_generation(),
    )
    .expect("credential-backed Draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id: previous.profile_id(),
                expected_current_revision: Some(previous.revision()),
            },
            name: ys_agent_core::ProfileName::new("New validation revision")
                .expect("valid Profile name"),
            revision: candidate.clone(),
        })
        .await
        .expect("append new Draft");
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();

    let validated = service
        .commit_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id: candidate.profile_id(),
                revision: candidate.revision(),
                credential_generation: candidate
                    .credential_generation()
                    .expect("candidate credential"),
                validation_digest: validation_digest.clone(),
            },
            evidence,
            versions,
        })
        .await
        .expect("current successful evidence makes the Draft Ready");
    assert_eq!(validated.summary.state, ProfileState::Ready);
    assert_eq!(validated.revision, candidate.revision());
    assert_eq!(validated.validation_id, Some(validation_id));
    assert_eq!(
        repository
            .active()
            .await
            .expect("read existing active before explicit activation")
            .expect("prior active remains")
            .profile_revision(),
        previous.revision()
    );

    let request = ActivateProfileRequest {
        operation_id: OperationId::new(),
        precondition: ActivationPrecondition {
            profile_id: candidate.profile_id(),
            revision: candidate.revision(),
            validation_id,
            validation_digest,
            expected_activation_revision: Some(previous_active.activation_revision()),
        },
    };
    let confirmation = service
        .activation_confirmation(&request)
        .await
        .expect("Ready current revision can be explicitly confirmed for activation");
    assert!(confirmation.affects_new_runs_only);
    assert_eq!(confirmation.profile_revision, candidate.revision());

    let active = service
        .activate(request)
        .await
        .expect("activation atomically changes the global singleton after confirmation");
    assert_eq!(active.profile_revision, candidate.revision());
    assert_eq!(
        active.activation_revision,
        previous_active.activation_revision() + 1
    );
}

#[tokio::test]
async fn service_rejects_evidence_not_bound_to_the_current_revision_inputs() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let previous_active = provider_fixture::persisted_test_active_provider(&store).await;
    let repository = store.provider_repository();
    let service = ProviderManagementService::new(Arc::new(repository.clone()));
    let previous = repository
        .load_current_revision(previous_active.profile_id())
        .await
        .expect("load prior active revision");
    let candidate = ProfileRevision::draft(
        previous.profile_id(),
        previous.revision() + 1,
        previous.provider(),
        previous.model().clone(),
        previous.parameters().clone(),
        previous.credential_generation(),
    )
    .expect("credential-backed Draft");
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id: previous.profile_id(),
                expected_current_revision: Some(previous.revision()),
            },
            name: ys_agent_core::ProfileName::new("Unbound validation evidence")
                .expect("valid Profile name"),
            revision: candidate.clone(),
        })
        .await
        .expect("append candidate Draft");

    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(previous.validation_inputs(versions.clone()));
    let error = service
        .commit_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id: candidate.profile_id(),
                revision: candidate.revision(),
                credential_generation: candidate
                    .credential_generation()
                    .expect("candidate credential"),
                validation_digest: evidence.digest(),
            },
            evidence,
            versions,
        })
        .await
        .expect_err("evidence from an old revision must not validate the current Draft");
    assert_eq!(error.code(), ProviderErrorCode::ValidationStale.as_str());
    assert_eq!(
        repository
            .load_current_revision(candidate.profile_id())
            .await
            .expect("load rejected Draft")
            .state(),
        ProfileState::Draft
    );
    assert_eq!(
        repository
            .active()
            .await
            .expect("read active singleton")
            .expect("prior active remains")
            .profile_revision(),
        previous.revision()
    );
}

#[tokio::test]
async fn cancellation_remains_effective_for_a_late_validation_commit() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let previous_active = provider_fixture::persisted_test_active_provider(&store).await;
    let repository = store.provider_repository();
    let service = ProviderManagementService::new(Arc::new(repository.clone()));
    let (previous, candidate) = append_credential_backed_draft(
        &repository,
        previous_active.profile_id(),
        "Cancelled validation revision",
    )
    .await;
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let operation_id = OperationId::new();
    let commit = validation_commit(&candidate, operation_id, evidence, versions);

    service
        .cancel_operation(operation_id)
        .expect("cancel operation");
    service
        .cancel_operation(operation_id)
        .expect("cancellation is idempotent");
    for late_commit in [commit.clone(), commit] {
        let error = service
            .commit_validation(late_commit)
            .await
            .expect_err("a cancelled operation cannot persist evidence later");
        assert_eq!(error.code(), ProviderErrorCode::OperationCancelled.as_str());
    }
    assert_eq!(
        repository
            .load_current_revision(candidate.profile_id())
            .await
            .expect("load cancelled Draft")
            .state(),
        ProfileState::Draft
    );
    assert_eq!(
        repository
            .active()
            .await
            .expect("read active singleton")
            .expect("prior active remains")
            .profile_revision(),
        previous.revision()
    );
}

#[tokio::test]
async fn invalid_and_historical_or_racing_activation_requests_preserve_a_consistent_active_view() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open database");
    let previous_active = provider_fixture::persisted_test_active_provider(&store).await;
    let repository = store.provider_repository();
    let service = Arc::new(ProviderManagementService::new(Arc::new(repository.clone())));
    let (previous, invalid_candidate) = append_credential_backed_draft(
        &repository,
        previous_active.profile_id(),
        "Invalid validation revision",
    )
    .await;
    let invalid_versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let invalid_evidence = CompatibilityEvidence::failing(
        invalid_candidate.validation_inputs(invalid_versions.clone()),
    );
    let invalid_id = invalid_evidence.id();
    let invalid_digest = invalid_evidence.digest();
    let invalid = service
        .commit_validation(validation_commit(
            &invalid_candidate,
            OperationId::new(),
            invalid_evidence,
            invalid_versions,
        ))
        .await
        .expect("failed evidence persists Invalid for caller feedback");
    assert_eq!(invalid.summary.state, ProfileState::Invalid);
    let invalid_request = ActivateProfileRequest {
        operation_id: OperationId::new(),
        precondition: ActivationPrecondition {
            profile_id: invalid_candidate.profile_id(),
            revision: invalid_candidate.revision(),
            validation_id: invalid_id,
            validation_digest: invalid_digest,
            expected_activation_revision: Some(previous_active.activation_revision()),
        },
    };
    let error = service
        .activation_confirmation(&invalid_request)
        .await
        .expect_err("Invalid revisions cannot be confirmed for activation");
    assert_eq!(
        error.code(),
        ProviderErrorCode::ActivationPreconditionFailed.as_str()
    );
    assert_eq!(
        repository
            .active()
            .await
            .expect("read prior active")
            .expect("prior active remains")
            .profile_revision(),
        previous.revision()
    );

    let (_, candidate) = append_credential_backed_draft(
        &repository,
        previous_active.profile_id(),
        "Ready race revision",
    )
    .await;
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    service
        .commit_validation(validation_commit(
            &candidate,
            OperationId::new(),
            evidence,
            versions,
        ))
        .await
        .expect("current passing evidence makes candidate Ready");
    let historical = previous
        .validation()
        .expect("prior active is Ready and validated");
    let historical_error = service
        .activation_confirmation(&ActivateProfileRequest {
            operation_id: OperationId::new(),
            precondition: ActivationPrecondition {
                profile_id: previous.profile_id(),
                revision: previous.revision(),
                validation_id: historical.id(),
                validation_digest: historical.digest(),
                expected_activation_revision: Some(previous_active.activation_revision()),
            },
        })
        .await
        .expect_err("a historical Ready revision cannot replace the current Profile revision");
    assert_eq!(
        historical_error.code(),
        ProviderErrorCode::ActivationPreconditionFailed.as_str()
    );

    let request = ActivateProfileRequest {
        operation_id: OperationId::new(),
        precondition: ActivationPrecondition {
            profile_id: candidate.profile_id(),
            revision: candidate.revision(),
            validation_id,
            validation_digest,
            expected_activation_revision: Some(previous_active.activation_revision()),
        },
    };
    let (first, second) =
        tokio::join!(service.activate(request.clone()), service.activate(request),);
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let active = repository
        .active()
        .await
        .expect("read active singleton")
        .expect("exactly one successful activation retains a singleton");
    assert_eq!(active.profile_revision(), candidate.revision());
    assert_eq!(
        active.activation_revision(),
        previous_active.activation_revision() + 1
    );
}
