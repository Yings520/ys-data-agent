use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tempfile::TempDir;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, CompatibilityEvidence, CredentialGeneration,
    DeviceAuthorizationView, OAuthConnectionService, OAuthConnectionStatus, OAuthConnectionView,
    OperationId, ProfileId, ProfileName, ProfileRevision, ProfileState, ProviderErrorCode,
    ProviderId, ProviderManagementError, ProviderRemediation, ProviderResult,
    RemoteRevocationOutcome, RevisionPrecondition, SaveProfileRevision, ValidationCommit,
    ValidationCommitPrecondition, ValidationVersions,
};
use ys_agent_runtime::provider::service::ProviderManagementService;
use ys_agent_store::SqliteRuntimeStore;

#[derive(Default)]
struct FakeOAuth {
    operations: Mutex<HashMap<OperationId, ProfileId>>,
    statuses: Mutex<HashMap<ProfileId, OAuthConnectionStatus>>,
    residual_risk: Mutex<bool>,
}

impl FakeOAuth {
    fn status(&self, profile_id: ProfileId, status: OAuthConnectionStatus) {
        self.statuses
            .lock()
            .expect("fake OAuth state")
            .insert(profile_id, status);
    }

    fn view_for(&self, profile_id: ProfileId) -> OAuthConnectionView {
        OAuthConnectionView {
            profile_id,
            status: self
                .statuses
                .lock()
                .expect("fake OAuth state")
                .get(&profile_id)
                .copied()
                .unwrap_or(OAuthConnectionStatus::Revoked),
            remediation: Some(ProviderRemediation::Reauthorize),
        }
    }
}

#[async_trait::async_trait]
impl OAuthConnectionService for FakeOAuth {
    async fn view(&self, profile_id: ProfileId) -> ProviderResult<OAuthConnectionView> {
        Ok(self.view_for(profile_id))
    }

    async fn restore(
        &self,
        profile_id: ProfileId,
        _generation: CredentialGeneration,
    ) -> ProviderResult<OAuthConnectionView> {
        Ok(self.view_for(profile_id))
    }

    async fn start(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.operations
            .lock()
            .expect("fake OAuth state")
            .insert(operation_id, profile_id);
        self.status(profile_id, OAuthConnectionStatus::Pending);
        Ok(DeviceAuthorizationView {
            verification_uri: "https://example.invalid/device".to_owned(),
            user_code: "safe-code".to_owned(),
            expires_in_seconds: 60,
        })
    }

    async fn complete(&self, operation_id: OperationId) -> ProviderResult<OAuthConnectionView> {
        let profile_id = self
            .operations
            .lock()
            .expect("fake OAuth state")
            .remove(&operation_id)
            .ok_or_else(oauth_error)?;
        self.status(profile_id, OAuthConnectionStatus::Connected);
        Ok(self.view_for(profile_id))
    }

    async fn refresh(
        &self,
        profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        self.status(profile_id, OAuthConnectionStatus::Connected);
        Ok(self.view_for(profile_id))
    }

    async fn reauthorize(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.start(profile_id, operation_id).await
    }

    async fn logout(
        &self,
        profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<RemoteRevocationOutcome> {
        self.status(profile_id, OAuthConnectionStatus::Revoked);
        if *self.residual_risk.lock().expect("fake OAuth state") {
            Ok(RemoteRevocationOutcome::ResidualRisk {
                remediation: ProviderRemediation::ContactSupport,
            })
        } else {
            Ok(RemoteRevocationOutcome::Revoked)
        }
    }
}

fn oauth_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OAuthNotConnected,
        Some(ys_agent_core::ProviderField::OAuth),
        ProviderRemediation::Reauthorize,
    )
}

async fn service_with_chatgpt_draft() -> (
    ProviderManagementService,
    ys_agent_store::SqliteProviderRepository,
    ProfileId,
    Arc<FakeOAuth>,
) {
    let directory = TempDir::new().expect("temporary database directory");
    let database = directory.keep().join("runtime.db");
    let store = SqliteRuntimeStore::open(database)
        .await
        .expect("open database");
    let repository = store.provider_repository();
    let profile_id = ProfileId::new();
    repository
        .save_revision(SaveProfileRevision {
            precondition: RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name: ProfileName::new("ChatGPT subscription").expect("valid name"),
            revision: ProfileRevision::draft(
                profile_id,
                1,
                ProviderId::ChatGptSubscription,
                ys_agent_core::ProviderModelId::new(
                    ProviderId::ChatGptSubscription,
                    "chatgpt/test-model",
                )
                .expect("governed model"),
                ys_agent_core::ProviderParameters::default(),
                None,
            )
            .expect("valid draft"),
        })
        .await
        .expect("save ChatGPT draft");
    let oauth = Arc::new(FakeOAuth::default());
    (
        ProviderManagementService::with_oauth(Arc::new(repository.clone()), oauth.clone()),
        repository,
        profile_id,
        oauth,
    )
}

#[tokio::test]
async fn oauth_completion_reauthorization_and_refresh_rotate_generations_without_moving_active() {
    let (service, repository, profile_id, _oauth) = service_with_chatgpt_draft().await;
    let connect = OperationId::new();
    service
        .start_oauth(profile_id, connect)
        .await
        .expect("start returns a safe device view");
    assert_eq!(
        service
            .oauth_connection(profile_id)
            .await
            .expect("pending is a safe connection view")
            .status,
        OAuthConnectionStatus::Pending
    );

    service
        .complete_oauth(connect)
        .await
        .expect("complete persists the connected OAuth generation");
    let connected = service
        .load_profile(profile_id)
        .await
        .expect("load connected profile");
    assert_eq!(connected.revision, 2);
    assert_eq!(
        connected
            .credential_generation
            .expect("OAuth generation")
            .number(),
        1
    );
    assert_eq!(connected.summary.state, ProfileState::Draft);
    let revision = repository
        .load_current_revision(profile_id)
        .await
        .expect("connected revision");
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
    let activation = ActivateProfileRequest {
        operation_id: OperationId::new(),
        precondition: ActivationPrecondition {
            profile_id,
            revision: revision.revision(),
            validation_id: evidence.id(),
            validation_digest: evidence.digest(),
            expected_activation_revision: None,
        },
    };
    service
        .commit_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id,
                revision: revision.revision(),
                credential_generation: revision.credential_generation().expect("OAuth generation"),
                validation_digest: evidence.digest(),
            },
            evidence,
            versions,
        })
        .await
        .expect("fresh OAuth generation can be validated before activation");
    service
        .activate(activation)
        .await
        .expect("explicitly activate the validated prior generation");

    let reauthorize = OperationId::new();
    service
        .reauthorize_oauth(profile_id, reauthorize)
        .await
        .expect("reauthorization starts another safe device flow");
    service
        .complete_oauth(reauthorize)
        .await
        .expect("reauthorization rotates into a newer Draft");
    let reauthorized = service
        .load_profile(profile_id)
        .await
        .expect("load reauthorized profile");
    assert_eq!(reauthorized.revision, 3);
    assert_eq!(
        reauthorized
            .credential_generation
            .expect("reauthorized generation")
            .number(),
        2
    );
    assert_eq!(
        repository
            .active()
            .await
            .expect("active snapshot")
            .expect("prior active remains")
            .profile_revision(),
        2
    );

    service
        .refresh_oauth(profile_id, OperationId::new())
        .await
        .expect("refresh rotates into a newer Draft that must be validated explicitly");
    let refreshed = service
        .load_profile(profile_id)
        .await
        .expect("load refreshed profile");
    assert_eq!(refreshed.revision, 4);
    assert_eq!(
        refreshed
            .credential_generation
            .expect("rotated generation")
            .number(),
        3
    );
    assert_eq!(refreshed.summary.state, ProfileState::Draft);
    assert_eq!(
        repository
            .active()
            .await
            .expect("active snapshot")
            .expect("old active remains until explicit activation")
            .profile_revision(),
        2
    );
}

#[tokio::test]
async fn oauth_cancellation_and_disconnected_status_fail_closed_for_activation_and_logout_reports_residual_risk()
 {
    let (service, repository, profile_id, oauth) = service_with_chatgpt_draft().await;
    let cancelled = OperationId::new();
    service
        .cancel_operation(cancelled)
        .expect("cancel operation");
    let cancelled_error = service
        .start_oauth(profile_id, cancelled)
        .await
        .expect_err("cancelled OAuth must not start");
    assert_eq!(
        cancelled_error.code(),
        ProviderErrorCode::OperationCancelled.as_str()
    );

    let connect = OperationId::new();
    service
        .start_oauth(profile_id, connect)
        .await
        .expect("start OAuth");
    service
        .complete_oauth(connect)
        .await
        .expect("connect OAuth");
    let current = repository
        .load_current_revision(profile_id)
        .await
        .expect("connected current revision");
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(current.validation_inputs(versions.clone()));
    let request = ActivateProfileRequest {
        operation_id: OperationId::new(),
        precondition: ActivationPrecondition {
            profile_id,
            revision: current.revision(),
            validation_id: evidence.id(),
            validation_digest: evidence.digest(),
            expected_activation_revision: None,
        },
    };
    service
        .commit_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id,
                revision: current.revision(),
                credential_generation: current.credential_generation().expect("OAuth generation"),
                validation_digest: evidence.digest(),
            },
            evidence,
            versions,
        })
        .await
        .expect("prepare Ready OAuth revision");
    for status in [
        OAuthConnectionStatus::Expired,
        OAuthConnectionStatus::Failed,
        OAuthConnectionStatus::Revoked,
    ] {
        oauth.status(profile_id, status);
        let activation_error = service
            .activate(request.clone())
            .await
            .expect_err("a disconnected OAuth status cannot activate");
        assert_eq!(
            activation_error.code(),
            ProviderErrorCode::OAuthNotConnected.as_str()
        );
    }

    *oauth.residual_risk.lock().expect("fake OAuth state") = true;
    assert!(matches!(
        service.logout_oauth(profile_id, OperationId::new()).await,
        Ok(RemoteRevocationOutcome::ResidualRisk {
            remediation: ProviderRemediation::ContactSupport
        })
    ));
}
