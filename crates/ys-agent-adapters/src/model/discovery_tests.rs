use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use liter_llm::types::{ModelObject, ModelsListResponse};
use ys_agent_core::{
    CredentialGeneration, CredentialKind, CredentialLease, DiscoverModelsRequest, ModelDiscovery,
    OperationId, ProfileId, ProviderErrorCode, ProviderField, ProviderId, ProviderRemediation,
    SecretValue,
};

use super::{
    ChatGptDirectoryModel, DiscoveryTransport, LiterModelDiscovery, TransportFailure,
    chatgpt_codex_client_version, chatgpt_codex_directory_url, chatgpt_codex_user_agent,
    fixed_provider_hint,
};

const ONLINE_MODEL_PROVIDERS: [ProviderId; 9] = [
    ProviderId::OpenAi,
    ProviderId::DeepSeek,
    ProviderId::Anthropic,
    ProviderId::Kimi,
    ProviderId::Qwen,
    ProviderId::Gemini,
    ProviderId::MiniMax,
    ProviderId::Glm,
    ProviderId::OpenRouter,
];

#[derive(Clone)]
struct FixtureTransport {
    result: Arc<Mutex<Result<ModelsListResponse, TransportFailure>>>,
    chatgpt_models: Arc<Mutex<Result<Vec<ChatGptDirectoryModel>, TransportFailure>>>,
    calls: Arc<AtomicUsize>,
    saw_secret: Arc<AtomicBool>,
}

impl FixtureTransport {
    fn success(ids: &[&str]) -> Self {
        Self::from_result(Ok(ModelsListResponse {
            object: "list".to_owned(),
            data: ids
                .iter()
                .map(|id| ModelObject {
                    id: (*id).to_owned(),
                    ..ModelObject::default()
                })
                .collect(),
        }))
    }

    fn failure(failure: TransportFailure) -> Self {
        Self::from_result(Err(failure))
    }

    fn from_result(result: Result<ModelsListResponse, TransportFailure>) -> Self {
        Self {
            result: Arc::new(Mutex::new(result)),
            chatgpt_models: Arc::new(Mutex::new(Ok(Vec::new()))),
            calls: Arc::new(AtomicUsize::new(0)),
            saw_secret: Arc::new(AtomicBool::new(false)),
        }
    }

    fn with_chatgpt_models(self, models: Vec<ChatGptDirectoryModel>) -> Self {
        *self.chatgpt_models.lock().expect("fixture ChatGPT models") = Ok(models);
        self
    }
}

#[async_trait]
impl DiscoveryTransport for FixtureTransport {
    async fn list_models(
        &self,
        _provider: ProviderId,
        credential: CredentialLease,
    ) -> Result<ModelsListResponse, TransportFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        credential.with_secret(|secret| {
            secret.with_exposed(|value| {
                self.saw_secret
                    .store(value == "fixture-secret", Ordering::SeqCst);
            });
        });
        self.result.lock().expect("fixture result").clone()
    }

    async fn list_chatgpt_models(
        &self,
        credential: CredentialLease,
    ) -> Result<Vec<ChatGptDirectoryModel>, TransportFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        credential.with_secret(|secret| {
            secret.with_exposed(|value| {
                self.saw_secret
                    .store(value == "fixture-secret", Ordering::SeqCst);
            });
        });
        self.chatgpt_models
            .lock()
            .expect("fixture ChatGPT models")
            .clone()
    }
}

fn request(provider: ProviderId) -> DiscoverModelsRequest {
    let profile_id = ProfileId::new();
    DiscoverModelsRequest {
        operation_id: OperationId::new(),
        profile_id,
        profile_revision: 1,
        provider,
        credential_generation: CredentialGeneration::new(
            profile_id,
            1,
            provider.required_credential_kind(),
        )
        .expect("credential generation"),
    }
}

fn credential() -> CredentialLease {
    CredentialLease::new(SecretValue::from_utf8("fixture-secret".to_owned()))
}

#[test]
fn chatgpt_directory_uses_the_supported_codex_protocol_version_not_the_app_version() {
    assert_eq!(chatgpt_codex_client_version(), "0.152.1");
    assert_eq!(
        chatgpt_codex_directory_url(),
        "https://chatgpt.com/backend-api/codex/models?client_version=0.152.1"
    );
    assert_eq!(chatgpt_codex_user_agent(), "codex_cli_rs/0.152.1");
    assert_ne!(chatgpt_codex_client_version(), env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn online_providers_use_fixed_hints_and_return_prefixed_models() {
    for provider in ONLINE_MODEL_PROVIDERS {
        assert_eq!(fixed_provider_hint(provider), Ok(provider.model_prefix()));

        let transport = FixtureTransport::success(&["model-z", "model-a", "model-a"]);
        let discovery = LiterModelDiscovery::with_transport(Arc::new(transport.clone()));
        let models = discovery
            .discover(request(provider), credential())
            .await
            .expect("discover models");

        assert_eq!(
            models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            vec![
                format!("{}model-a", provider.model_prefix()),
                format!("{}model-z", provider.model_prefix()),
            ]
        );
        assert!(models.iter().all(|model| model.context_limit.is_none()));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert!(transport.saw_secret.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn already_prefixed_results_survive_and_wrong_outer_prefixes_are_filtered() {
    let provider = ProviderId::DeepSeek;
    let transport = FixtureTransport::success(&[
        "deepseek/model-a",
        "xai/foreign-model",
        "plain-model",
        " deepseek/space",
        "",
    ]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

    let models = discovery
        .discover(request(provider), credential())
        .await
        .expect("valid models remain");

    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["deepseek/model-a", "deepseek/plain-model"]
    );
}

#[tokio::test]
async fn online_catalog_omits_non_chat_models_before_they_become_selectable() {
    let transport = FixtureTransport::success(&["gpt-5.4", "text-embedding-3-small"]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

    let models = discovery
        .discover(request(ProviderId::OpenAi), credential())
        .await
        .expect("a supported chat model remains");

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model, "openai/gpt-5.4");
    assert_eq!(models[0].context_limit, Some(1_050_000));
}

#[tokio::test]
async fn opencode_go_intersects_the_online_catalog_with_the_product_allowlist() {
    let transport = FixtureTransport::success(&[
        "deepseek-v4-pro",
        "kimi-k3",
        "claude-sonnet-4-6",
        "unapproved-model",
    ]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

    let models = discovery
        .discover(request(ProviderId::OpenCodeGo), credential())
        .await
        .expect("supported OpenCode Go models remain selectable");

    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        ["opencode-go/deepseek-v4-pro", "opencode-go/kimi-k3"]
    );
}

#[tokio::test]
async fn every_opencode_go_model_has_catalog_context_evidence() {
    let transport = FixtureTransport::success(&[
        "deepseek-v4-pro",
        "deepseek-v4-flash",
        "kimi-k2.7-code",
        "kimi-k3",
        "kimi-k2.6",
        "glm-5.2",
        "glm-5.1",
        "grok-4.5",
        "mimo-v2.5-pro",
        "mimo-v2.5",
    ]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

    let models = discovery
        .discover(request(ProviderId::OpenCodeGo), credential())
        .await
        .expect("the OpenCode Go catalog is selectable");

    assert_eq!(models.len(), 10);
    assert!(
        models
            .iter()
            .all(|model| model.context_limit.is_some_and(|limit| limit > 0)),
        "every displayed OpenCode Go model must carry activation context evidence: {models:?}"
    );
}

#[tokio::test]
async fn opencode_go_rejects_an_online_catalog_without_supported_models() {
    let transport = FixtureTransport::success(&["claude-sonnet-4-6", "unapproved-model"]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

    let error = discovery
        .discover(request(ProviderId::OpenCodeGo), credential())
        .await
        .expect_err("an empty allowlist intersection cannot become a selectable catalog");

    assert_eq!(error.code(), ProviderErrorCode::DiscoveryFailed.as_str());
    assert_eq!(error.field(), Some(&ProviderField::Model));
}

#[tokio::test]
async fn empty_or_fully_polluted_catalog_is_a_recoverable_discovery_failure() {
    for ids in [&[][..], &["xai/foreign-model", "   "][..]] {
        let transport = FixtureTransport::success(ids);
        let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));

        let error = discovery
            .discover(request(ProviderId::DeepSeek), credential())
            .await
            .expect_err("manual entry remains available");

        assert_eq!(error.code(), ProviderErrorCode::DiscoveryFailed.as_str());
        assert_eq!(error.field(), Some(&ProviderField::Model));
        assert_eq!(error.remediation(), ProviderRemediation::ReturnToEdit);
    }
}

#[tokio::test]
async fn invalid_request_never_reaches_the_transport() {
    let transport = FixtureTransport::success(&["model-a"]);
    let discovery = LiterModelDiscovery::with_transport(Arc::new(transport.clone()));

    let mut mismatched = request(ProviderId::Anthropic);
    mismatched.credential_generation =
        CredentialGeneration::new(mismatched.profile_id, 1, CredentialKind::OAuthConnection)
            .expect("mismatched generation");
    let credential_error = discovery
        .discover(mismatched, credential())
        .await
        .expect_err("API-key provider cannot use OAuth credential");
    assert_eq!(
        credential_error.code(),
        ProviderErrorCode::AuthenticationInvalid.as_str()
    );
    assert_eq!(credential_error.field(), Some(&ProviderField::Credential));

    let mut wrong_profile = request(ProviderId::DeepSeek);
    wrong_profile.credential_generation =
        CredentialGeneration::new(ProfileId::new(), 1, CredentialKind::ApiKey)
            .expect("foreign generation");
    let profile_error = discovery
        .discover(wrong_profile, credential())
        .await
        .expect_err("credential generation belongs to another profile");
    assert_eq!(
        profile_error.code(),
        ProviderErrorCode::AuthenticationInvalid.as_str()
    );

    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn chatgpt_subscription_uses_its_account_model_directory_and_other_plans_stay_curated() {
    let transport = FixtureTransport::success(&["must-not-be-used"]);
    let discovery =
        LiterModelDiscovery::with_transport(Arc::new(transport.clone().with_chatgpt_models(vec![
            ChatGptDirectoryModel::listed("gpt-5.6-terra", 272_000, 7),
            ChatGptDirectoryModel::hidden("gpt-reserve", 272_000, 3),
            ChatGptDirectoryModel::listed("gpt-5.6-sol", 272_000, 6),
            ChatGptDirectoryModel::listed("gpt-5.6-sol", 272_000, 9),
            ChatGptDirectoryModel::listed("missing-context", 0, 10),
        ])));

    let codex = discovery
        .discover(request(ProviderId::ChatGptSubscription), credential())
        .await
        .expect("connected Codex subscription has its account-visible models");
    assert_eq!(
        codex
            .iter()
            .map(|model| (model.model.as_str(), model.context_limit))
            .collect::<Vec<_>>(),
        vec![
            ("chatgpt/gpt-5.6-sol", Some(272_000)),
            ("chatgpt/gpt-5.6-terra", Some(272_000)),
        ]
    );

    let alibaba = discovery
        .discover(request(ProviderId::AlibabaCoding), credential())
        .await
        .expect("Alibaba coding plan models");
    assert!(
        alibaba
            .iter()
            .any(|model| model.model == "anthropic/qwen3-coder-plus")
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transport_failures_map_to_stable_sanitized_errors() {
    let cases = [
        (
            TransportFailure::Authentication,
            ProviderErrorCode::AuthenticationInvalid,
            ProviderField::Credential,
            ProviderRemediation::ReturnToEdit,
        ),
        (
            TransportFailure::RateLimited,
            ProviderErrorCode::RateLimited,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        (
            TransportFailure::Timeout,
            ProviderErrorCode::Timeout,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        (
            TransportFailure::Network,
            ProviderErrorCode::Network,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        (
            TransportFailure::Server,
            ProviderErrorCode::Server,
            ProviderField::Model,
            ProviderRemediation::Retry,
        ),
        (
            TransportFailure::InvalidResponse,
            ProviderErrorCode::DiscoveryFailed,
            ProviderField::Model,
            ProviderRemediation::ReturnToEdit,
        ),
    ];

    for (failure, expected_code, expected_field, expected_remediation) in cases {
        let transport = FixtureTransport::failure(failure);
        let discovery = LiterModelDiscovery::with_transport(Arc::new(transport));
        let error = discovery
            .discover(request(ProviderId::OpenRouter), credential())
            .await
            .expect_err("stable discovery failure");

        assert_eq!(error.code(), expected_code.as_str());
        assert_eq!(error.field(), Some(&expected_field));
        assert_eq!(error.remediation(), expected_remediation);
        assert!(!error.to_string().contains("fixture-secret"));
    }
}
