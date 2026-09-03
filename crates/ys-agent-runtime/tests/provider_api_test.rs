use std::{collections::VecDeque, sync::Arc};

use ys_agent_adapters::{
    credential::keyring::InMemoryCredentialVault, model::liter::LiterProviderFactory,
};
use ys_agent_core::{
    CredentialLease, CredentialVault, CredentialViewStatus, DiscoverModelsRequest, DiscoveredModel,
    ListModelCandidatesRequest, ModelCandidateStatus, ModelDiscovery, ProfileName,
    ProtectedCredentialWrite, ProviderCatalogView, ProviderCredentialReference, ProviderErrorCode,
    ProviderField, ProviderId, ProviderManagementApi, ProviderManagementError,
    ProviderProfileRepository, ProviderRemediation, ProviderResult, ProviderSupportStatus,
    RunProviderBindingRepository, SecretValue, SelectionAvailability, SelectionCurrentStatus,
    SelectionTarget,
};
use ys_agent_runtime::provider::{
    api::InProcessProviderManagementApi,
    catalog::GovernedProviderCatalog,
    service::{CredentialService, ProviderManagementService},
};
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

struct ScriptedDiscovery {
    responses: tokio::sync::Mutex<VecDeque<ProviderResult<Vec<DiscoveredModel>>>>,
}

#[async_trait::async_trait]
impl ModelDiscovery for ScriptedDiscovery {
    async fn discover(
        &self,
        _request: DiscoverModelsRequest,
        _credential: CredentialLease,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        self.responses
            .lock()
            .await
            .pop_front()
            .expect("scripted discovery response")
    }
}

fn catalog_views() -> Vec<ProviderCatalogView> {
    ProviderId::ALL
        .into_iter()
        .map(|provider| ProviderCatalogView {
            provider,
            display_name: format!("{provider:?}"),
            credential_kind: provider.required_credential_kind(),
            support_status: ProviderSupportStatus::Candidate,
            evidence_gaps: vec!["evidence_pending".to_owned()],
        })
        .collect()
}

#[tokio::test]
async fn masked_provider_api_serves_offline_catalog_profiles_and_active_snapshot() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let store = Arc::new(
        SqliteRuntimeStore::open(directory.path().join("runtime.db"))
            .await
            .expect("open runtime store"),
    );
    let active = provider_fixture::persisted_test_active_provider(store.as_ref()).await;
    let profiles: Arc<dyn ProviderProfileRepository> = Arc::new(store.provider_repository());
    let run_bindings: Arc<dyn RunProviderBindingRepository> =
        Arc::new(store.run_binding_repository());
    let vault = Arc::new(InMemoryCredentialVault::new());
    vault
        .write_generation(ProtectedCredentialWrite {
            reference: ProviderCredentialReference {
                profile_id: active.profile_id(),
                generation: active.credential_generation(),
            },
            secret: SecretValue::from_utf8("provider-api-secret".to_owned()),
        })
        .await
        .expect("seed protected Credential");
    let discovery = Arc::new(ScriptedDiscovery {
        responses: tokio::sync::Mutex::new(VecDeque::from([
            Ok(vec![
                DiscoveredModel {
                    model: "deepseek/test-model".to_owned(),
                    context_limit: None,
                },
                DiscoveredModel {
                    model: "deepseek/new-model".to_owned(),
                    context_limit: None,
                },
                DiscoveredModel {
                    model: "anthropic/polluted".to_owned(),
                    context_limit: None,
                },
            ]),
            Err(ProviderManagementError::new(
                ProviderErrorCode::DiscoveryFailed,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            )),
        ])),
    });
    let lifecycle = Arc::new(ProviderManagementService::new(profiles.clone()));
    let credentials = Arc::new(CredentialService::new(
        profiles.clone(),
        run_bindings.clone(),
        vault.clone(),
    ));
    let api = InProcessProviderManagementApi::new(
        GovernedProviderCatalog::default(),
        catalog_views(),
        profiles,
        vault,
        run_bindings,
        lifecycle,
        credentials,
        discovery,
        Arc::new(LiterProviderFactory::new()),
    );

    assert_eq!(api.catalog().await.expect("offline catalog").len(), 9);
    let profiles = api.list_profiles().await.expect("masked profile list");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].credential_status, CredentialViewStatus::Saved);

    let active_view = api
        .active_provider()
        .await
        .expect("active snapshot")
        .expect("fixture active snapshot");
    assert_eq!(active_view.profile_id, active.profile_id());
    assert_eq!(active_view.profile_revision, active.profile_revision());
    assert!(!format!("{active_view:?}").contains("credential"));

    let reactivated = api
        .activate_current(active.profile_id(), ys_agent_core::OperationId::new())
        .await
        .expect("activation derives its CAS precondition from the committed revision");
    assert_eq!(reactivated.profile_id, active.profile_id());
    assert_eq!(reactivated.profile_revision, active.profile_revision());
    assert_eq!(
        reactivated.activation_revision,
        active.activation_revision() + 1,
        "the façade returns the committed Active snapshot rather than a TUI prediction"
    );

    let copied = api
        .copy_profile(
            active.profile_id(),
            ProfileName::new("Backup").expect("valid name"),
        )
        .await
        .expect("copy same-named model into another Profile");
    let snapshot = api
        .model_selection_snapshot()
        .await
        .expect("compose selection snapshot");
    assert_eq!(snapshot.targets().len(), 9);
    assert_eq!(
        snapshot
            .targets()
            .iter()
            .filter(|target| target.current() == SelectionCurrentStatus::Current)
            .count(),
        1
    );
    assert!(snapshot.targets().iter().any(|target| matches!(
        target.target(),
        SelectionTarget::Plan {
            provider: ProviderId::ChatGptSubscription,
            ..
        }
    )));
    assert_eq!(
        snapshot
            .targets()
            .iter()
            .find(|target| target.target().provider() == ProviderId::DeepSeek)
            .expect("DeepSeek target")
            .availability(),
        SelectionAvailability::Configured
    );
    assert_eq!(
        snapshot
            .targets()
            .iter()
            .find(|target| target.target().provider() == ProviderId::Anthropic)
            .expect("Anthropic target")
            .availability(),
        SelectionAvailability::NeedsSetup
    );

    let request = ListModelCandidatesRequest {
        target: SelectionTarget::Provider(ProviderId::DeepSeek),
    };
    let first = api
        .list_model_candidates(request.clone())
        .await
        .expect("merge saved and discovered candidates");
    assert_eq!(first.candidates().len(), 3);
    assert_eq!(
        first
            .candidates()
            .iter()
            .filter(|candidate| candidate.current() == SelectionCurrentStatus::Current)
            .count(),
        1
    );
    assert!(first.candidates().iter().any(|candidate| {
        candidate.key().profile_id() == copied.summary.profile_id
            && candidate.key().model().as_str() == "deepseek/test-model"
            && candidate.status() == ModelCandidateStatus::Unavailable
    }));
    assert!(first.candidates().iter().any(|candidate| {
        candidate.key().model().as_str() == "deepseek/new-model"
            && candidate.status() == ModelCandidateStatus::NeedsValidation
    }));
    assert!(
        !serde_json::to_string(&first)
            .expect("serialize candidates")
            .contains("anthropic/polluted")
    );

    let second = api
        .list_model_candidates(request)
        .await
        .expect("discovery failure retains saved candidates");
    assert_eq!(second.candidates().len(), 2);
    assert!(
        second
            .candidates()
            .iter()
            .all(|candidate| candidate.key().model().as_str() == "deepseek/test-model")
    );
}

#[tokio::test]
async fn empty_catalog_stays_empty_and_rejects_unprovable_candidates() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let store = Arc::new(
        SqliteRuntimeStore::open(directory.path().join("runtime.db"))
            .await
            .expect("open runtime store"),
    );
    let profiles: Arc<dyn ProviderProfileRepository> = Arc::new(store.provider_repository());
    let run_bindings: Arc<dyn RunProviderBindingRepository> =
        Arc::new(store.run_binding_repository());
    let vault = Arc::new(InMemoryCredentialVault::new());
    let lifecycle = Arc::new(ProviderManagementService::new(profiles.clone()));
    let credentials = Arc::new(CredentialService::new(
        profiles.clone(),
        run_bindings.clone(),
        vault.clone(),
    ));
    let api = InProcessProviderManagementApi::new(
        GovernedProviderCatalog::default(),
        Vec::new(),
        profiles,
        vault,
        run_bindings,
        lifecycle,
        credentials,
        Arc::new(ScriptedDiscovery {
            responses: tokio::sync::Mutex::new(VecDeque::new()),
        }),
        Arc::new(LiterProviderFactory::new()),
    );

    let snapshot = api
        .model_selection_snapshot()
        .await
        .expect("an empty governed projection is an explicit empty state");
    assert!(snapshot.targets().is_empty());

    let error = api
        .list_model_candidates(ListModelCandidatesRequest {
            target: SelectionTarget::Provider(ProviderId::DeepSeek),
        })
        .await
        .expect_err("an absent target cannot produce model candidates");
    assert_eq!(error.code(), "provider.protocol.incompatible");
}
