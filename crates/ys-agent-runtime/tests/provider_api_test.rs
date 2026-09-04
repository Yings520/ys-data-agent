use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ys_agent_adapters::{
    credential::memory::InMemoryCredentialVault, model::liter::LiterProviderFactory,
};
use ys_agent_core::{
    AgentAction, CoreError, CoreResult, CredentialLease, CredentialVault, CredentialViewStatus,
    DiscoverModelsRequest, DiscoveredModel, ListModelCandidatesRequest, ModelCandidateKey,
    ModelCandidateStatus, ModelCapabilities, ModelDiscovery, ModelProvider, ModelRequest,
    ModelResponse, ProfileName, ProtectedCredentialWrite, ProviderCatalogView,
    ProviderClientBinding, ProviderClientFactory, ProviderCredentialReference, ProviderErrorCode,
    ProviderField, ProviderId, ProviderManagementApi, ProviderManagementError,
    ProviderProfileRepository, ProviderRemediation, ProviderResult, ProviderSupportStatus,
    RunProviderBindingRepository, SecretValue, SelectionAvailability, SelectionCurrentStatus,
    SelectionTarget, SwitchModelRequest, ToolCall, ToolCallId,
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
    delay_ms: AtomicUsize,
}

#[async_trait::async_trait]
impl ModelDiscovery for ScriptedDiscovery {
    async fn discover(
        &self,
        _request: DiscoverModelsRequest,
        _credential: CredentialLease,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        let response = self
            .responses
            .lock()
            .await
            .pop_front()
            .expect("scripted discovery response");
        let delay_ms = self.delay_ms.load(Ordering::SeqCst);
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
        }
        response
    }
}

struct ScriptedProvider {
    responses: tokio::sync::Mutex<VecDeque<CoreResult<ModelResponse>>>,
}

#[async_trait::async_trait]
impl ModelProvider for ScriptedProvider {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            tool_calling: true,
            structured_outputs: true,
            max_context_tokens: 0,
            parallel_tool_calls: false,
            streaming: false,
        }
    }

    async fn complete(&self, _request: ModelRequest) -> CoreResult<ModelResponse> {
        self.responses
            .lock()
            .await
            .pop_front()
            .expect("scripted Provider response")
    }
}

struct ScriptedProviderFactory {
    scripts: tokio::sync::Mutex<VecDeque<VecDeque<CoreResult<ModelResponse>>>>,
}

#[async_trait::async_trait]
impl ProviderClientFactory for ScriptedProviderFactory {
    async fn build(
        &self,
        _binding: ProviderClientBinding,
        credential: CredentialLease,
    ) -> ProviderResult<Arc<dyn ModelProvider>> {
        credential.with_secret(|_| ());
        let responses = self
            .scripts
            .lock()
            .await
            .pop_front()
            .expect("scripted Provider instance");
        Ok(Arc::new(ScriptedProvider {
            responses: tokio::sync::Mutex::new(responses),
        }))
    }
}

fn successful_probe_responses() -> VecDeque<CoreResult<ModelResponse>> {
    VecDeque::from([
        Ok(ModelResponse {
            action: AgentAction::CallTool {
                call: ToolCall {
                    id: ToolCallId::new(),
                    provider_call_id: Some("switch-model-probe".to_owned()),
                    name: "ysda_compatibility_probe".to_owned(),
                    arguments: serde_json::json!({"probe": "ysda-v2"}),
                    version: "v1".to_owned(),
                },
            },
            raw_content: None,
            usage: None,
        }),
        Ok(ModelResponse {
            action: AgentAction::Respond {
                message: "probe complete".to_owned(),
            },
            raw_content: None,
            usage: None,
        }),
    ])
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
            Ok(vec![DiscoveredModel {
                model: "deepseek/new-model".to_owned(),
                context_limit: Some(64),
            }]),
            Ok(vec![DiscoveredModel {
                model: "deepseek/timeout-model".to_owned(),
                context_limit: Some(64),
            }]),
            Err(ProviderManagementError::new(
                ProviderErrorCode::DiscoveryFailed,
                Some(ProviderField::Model),
                ProviderRemediation::Retry,
            )),
        ])),
        delay_ms: AtomicUsize::new(0),
    });
    let factory = Arc::new(ScriptedProviderFactory {
        scripts: tokio::sync::Mutex::new(VecDeque::from([
            successful_probe_responses(),
            VecDeque::from([Err(CoreError::validation(
                "provider.timeout",
                "injected compatibility timeout",
            ))]),
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
        vault.clone(),
        run_bindings,
        lifecycle,
        credentials,
        discovery.clone(),
        factory,
    )
    .with_model_discovery_timeout(Duration::from_millis(20));

    assert_eq!(
        api.catalog().await.expect("offline catalog").len(),
        ProviderId::ALL.len()
    );
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
    assert_eq!(snapshot.targets().len(), ProviderId::ALL.len());
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
    assert!(first.candidates().iter().all(|candidate| {
        !candidate
            .model_display_name()
            .starts_with(ProviderId::DeepSeek.model_prefix())
    }));
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
            && candidate.status() == ModelCandidateStatus::Unavailable
    }));
    let ready_key = first
        .candidates()
        .iter()
        .find(|candidate| candidate.current() == SelectionCurrentStatus::Current)
        .expect("current Ready candidate")
        .key()
        .clone();
    assert!(
        !serde_json::to_string(&first)
            .expect("serialize candidates")
            .contains("anthropic/polluted")
    );

    discovery.delay_ms.store(100, Ordering::SeqCst);
    let second = api
        .list_model_candidates(request)
        .await
        .expect("discovery timeout retains saved candidates");
    discovery.delay_ms.store(0, Ordering::SeqCst);
    assert_eq!(second.candidates().len(), 2);
    assert!(
        second
            .candidates()
            .iter()
            .all(|candidate| candidate.key().model().as_str() == "deepseek/test-model")
    );

    let unavailable_key = ModelCandidateKey::new(
        copied.summary.profile_id,
        copied.revision,
        Some(reactivated.activation_revision),
        copied.summary.provider,
        copied.model.clone(),
    )
    .expect("unavailable candidate key");
    let error = api
        .switch_model(SwitchModelRequest::new(
            ys_agent_core::OperationId::new(),
            unavailable_key,
        ))
        .await
        .expect_err("a candidate without a protected Credential must fail closed");
    assert_eq!(error.code(), "provider.credential.missing");
    assert_eq!(
        api.active_provider()
            .await
            .expect("active readback after missing Credential")
            .expect("active Provider"),
        reactivated
    );

    let switched = api
        .switch_model(SwitchModelRequest::new(
            ys_agent_core::OperationId::new(),
            ready_key.clone(),
        ))
        .await
        .expect("atomically reactivate a Ready candidate");
    assert_eq!(switched.profile_id, active.profile_id());
    assert_eq!(switched.model.as_str(), "deepseek/test-model");
    assert_eq!(
        api.active_provider()
            .await
            .expect("authoritative readback")
            .expect("active Provider"),
        switched
    );

    let error = api
        .switch_model(SwitchModelRequest::new(
            ys_agent_core::OperationId::new(),
            ready_key,
        ))
        .await
        .expect_err("a candidate with a stale activation revision must fail closed");
    assert_eq!(error.code(), "provider.activation.precondition_failed");
    assert_eq!(
        api.active_provider()
            .await
            .expect("active readback after conflict")
            .expect("active Provider"),
        switched
    );

    let cancelled_operation = ys_agent_core::OperationId::new();
    api.cancel_operation(cancelled_operation)
        .await
        .expect("record cancellation");
    let fresh_key = ModelCandidateKey::new(
        switched.profile_id,
        switched.profile_revision,
        Some(switched.activation_revision),
        switched.provider,
        switched.model.clone(),
    )
    .expect("fresh candidate key");
    let error = api
        .switch_model(SwitchModelRequest::new(cancelled_operation, fresh_key))
        .await
        .expect_err("a cancelled switch must fail closed");
    assert_eq!(error.code(), "provider.operation.cancelled");
    assert_eq!(
        api.active_provider()
            .await
            .expect("active readback after cancellation")
            .expect("active Provider"),
        switched
    );

    let first_use_key = ModelCandidateKey::new(
        switched.profile_id,
        switched.profile_revision,
        Some(switched.activation_revision),
        switched.provider,
        ys_agent_core::ProviderModelId::new(ProviderId::DeepSeek, "deepseek/new-model")
            .expect("governed model"),
    )
    .expect("first-use candidate key");
    let first_used = api
        .switch_model(SwitchModelRequest::new(
            ys_agent_core::OperationId::new(),
            first_use_key,
        ))
        .await
        .expect("discover, persist, validate, and activate a first-use model");
    assert_eq!(first_used.model.as_str(), "deepseek/new-model");
    assert_eq!(first_used.profile_revision, switched.profile_revision + 1);
    assert_eq!(
        api.active_provider()
            .await
            .expect("first-use authoritative readback")
            .expect("active Provider"),
        first_used
    );

    let timeout_key = ModelCandidateKey::new(
        first_used.profile_id,
        first_used.profile_revision,
        Some(first_used.activation_revision),
        first_used.provider,
        ys_agent_core::ProviderModelId::new(ProviderId::DeepSeek, "deepseek/timeout-model")
            .expect("governed model"),
    )
    .expect("timeout candidate key");
    let error = api
        .switch_model(SwitchModelRequest::new(
            ys_agent_core::OperationId::new(),
            timeout_key,
        ))
        .await
        .expect_err("a compatibility timeout must not replace the active model");
    assert_eq!(error.code(), "provider.timeout");
    assert_eq!(
        api.active_provider()
            .await
            .expect("active readback after timeout")
            .expect("active Provider"),
        first_used
    );
    let after_timeout = api
        .list_model_candidates(ListModelCandidatesRequest {
            target: SelectionTarget::Provider(ProviderId::DeepSeek),
        })
        .await
        .expect("reload candidates after a failed first-use validation");
    let current = after_timeout
        .candidates()
        .iter()
        .filter(|candidate| candidate.current().is_current())
        .collect::<Vec<_>>();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].key().model().as_str(), "deepseek/new-model");

    let active_detail = api
        .load_profile(first_used.profile_id)
        .await
        .expect("load active Profile before credential loss");
    let credential_generation = active_detail
        .credential_generation
        .expect("active Profile has a credential generation");
    vault
        .delete_generation(ProviderCredentialReference {
            profile_id: first_used.profile_id,
            generation: credential_generation,
        })
        .await
        .expect("simulate protected credential loss");

    assert_eq!(
        api.active_provider()
            .await
            .expect("durable active pointer remains available for CAS"),
        Some(first_used.clone())
    );
    assert_eq!(
        api.usable_active_provider()
            .await
            .expect("credential loss is a renderable active-model state"),
        None,
        "a missing Credential must not appear usable in the Header or Query gate"
    );

    let snapshot = api
        .model_selection_snapshot()
        .await
        .expect("credential loss remains a renderable selection state");
    let target = snapshot
        .targets()
        .iter()
        .find(|target| target.target().provider() == ProviderId::DeepSeek)
        .expect("DeepSeek target remains visible");
    assert_eq!(target.current(), SelectionCurrentStatus::Current);
    assert_eq!(target.availability(), SelectionAvailability::NeedsSetup);

    let candidates = api
        .list_model_candidates(ListModelCandidatesRequest {
            target: SelectionTarget::Provider(ProviderId::DeepSeek),
        })
        .await
        .expect("credential loss remains a renderable candidate state");
    let current = candidates
        .candidates()
        .iter()
        .find(|candidate| candidate.current().is_current())
        .expect("persisted current model remains marked");
    assert_eq!(current.status(), ModelCandidateStatus::Unavailable);
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
            delay_ms: AtomicUsize::new(0),
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
