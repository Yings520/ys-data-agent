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

use super::{DiscoveryTransport, LiterModelDiscovery, TransportFailure, fixed_provider_hint};

const API_KEY_PROVIDERS: [ProviderId; 8] = [
    ProviderId::OpenCodeGo,
    ProviderId::OpenCodeZen,
    ProviderId::DeepSeek,
    ProviderId::Xai,
    ProviderId::Zai,
    ProviderId::OpenRouter,
    ProviderId::MiniMax,
    ProviderId::Anthropic,
];

#[derive(Clone)]
struct FixtureTransport {
    result: Arc<Mutex<Result<ModelsListResponse, TransportFailure>>>,
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
            calls: Arc::new(AtomicUsize::new(0)),
            saw_secret: Arc::new(AtomicBool::new(false)),
        }
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

#[tokio::test]
async fn eight_allowlisted_providers_use_fixed_hints_and_return_prefixed_models() {
    for provider in API_KEY_PROVIDERS {
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

    let chatgpt_error = discovery
        .discover(request(ProviderId::ChatGptSubscription), credential())
        .await
        .expect_err("ChatGPT uses its fixed backend list");
    assert_eq!(
        chatgpt_error.code(),
        ProviderErrorCode::ProtocolIncompatible.as_str()
    );

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
