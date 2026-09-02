use std::sync::Arc;

use ys_agent_adapters::{
    credential::keyring::InMemoryCredentialVault,
    model::{discovery::LiterModelDiscovery, liter::LiterProviderFactory},
};
use ys_agent_core::{
    CredentialViewStatus, ProviderCatalogView, ProviderId, ProviderManagementApi,
    ProviderProfileRepository, ProviderSupportStatus, RunProviderBindingRepository,
};
use ys_agent_runtime::provider::{
    api::InProcessProviderManagementApi,
    catalog::GovernedProviderCatalog,
    service::{CredentialService, ProviderManagementService},
};
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

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
        Arc::new(LiterModelDiscovery::new()),
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
}
