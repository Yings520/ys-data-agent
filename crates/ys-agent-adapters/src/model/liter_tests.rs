use std::time::Duration;

use liter_llm::auth::Credential;
use liter_llm::error::LiterLlmError;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use ys_agent_core::{
    CredentialGeneration, CredentialLease, ProfileId, ProviderClientBinding, ProviderClientFactory,
    ProviderErrorCategory, ProviderErrorCode, ProviderField, ProviderId, ProviderModelId,
    ProviderParameterKey, ProviderParameters, ProviderRemediation, ProviderRetryability,
    SecretValue,
};

use super::{ClientPlan, LiterProviderFactory, ProviderErrorNormalizer};

const FIXTURE_ACCESS_TOKEN: &str = "fixture-access-token";
const FIXTURE_ACCOUNT_ID: &str = "fixture-account";

fn binding(provider: ProviderId, parameters: ProviderParameters) -> ProviderClientBinding {
    let profile_id = ProfileId::new();
    ProviderClientBinding {
        profile_id,
        profile_revision: 1,
        provider,
        model: ProviderModelId::new(
            provider,
            format!("{}fixture-model", provider.model_prefix()),
        )
        .expect("prefixed fixture model"),
        parameters,
        credential_generation: CredentialGeneration::new(
            profile_id,
            1,
            provider.required_credential_kind(),
        )
        .expect("credential generation"),
    }
}

fn api_key_lease() -> CredentialLease {
    CredentialLease::new(SecretValue::from_utf8("fixture-api-key".to_owned()))
}

fn oauth_lease() -> CredentialLease {
    let header = base64_url_encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64_url_encode(
        serde_json::to_string(&json!({
            "iss": "https://auth.openai.com",
            "aud": "app_EMoamEEZ73f0CkXaXp7hrann",
            "sub": "fixture-subject",
            "exp": 4_102_444_800_u64,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": FIXTURE_ACCOUNT_ID
            }
        }))
        .expect("JWT claims serialize")
        .as_bytes(),
    );
    CredentialLease::new(SecretValue::from_utf8(
        json!({
            "schema_version": 1,
            "access_token": FIXTURE_ACCESS_TOKEN,
            "refresh_token": "fixture-refresh-token",
            "id_token": format!("{header}.{payload}.fixture-signature"),
            "account_id": FIXTURE_ACCOUNT_ID,
            "subject": "fixture-subject",
            "expires_at_epoch_seconds": 4_102_444_800_u64
        })
        .to_string(),
    ))
}

#[test]
fn all_selectable_bindings_create_only_fixed_client_plans() {
    let factory = LiterProviderFactory::new();

    for provider in ProviderId::ALL {
        let binding = binding(provider, ProviderParameters::default());
        let credential = if provider == ProviderId::ChatGptSubscription {
            oauth_lease()
        } else {
            api_key_lease()
        };
        let plan = factory
            .build_plan(binding, credential)
            .expect("allowlisted client plan");

        match (provider, plan) {
            (ProviderId::ChatGptSubscription, ClientPlan::Responses { config, .. }) => {
                assert_eq!(
                    config.base_url.as_deref(),
                    Some("https://chatgpt.com/backend-api/codex")
                );
                assert_eq!(config.api_key.expose_secret(), FIXTURE_ACCESS_TOKEN);
                assert_eq!(
                    config.headers(),
                    [
                        (
                            "ChatGPT-Account-ID".to_owned(),
                            FIXTURE_ACCOUNT_ID.to_owned()
                        ),
                        ("originator".to_owned(), "codex_cli_rs".to_owned()),
                        ("user-agent".to_owned(), "codex_cli_rs/0.152.1".to_owned()),
                        ("accept".to_owned(), "text/event-stream".to_owned()),
                    ]
                );
                assert!(!config.load_env);
                assert_eq!(config.timeout, Duration::from_secs(30));
                assert_eq!(config.max_retries, 0);
            }
            (
                expected,
                ClientPlan::Chat {
                    model_hint, config, ..
                },
            ) => {
                let expected_base_url = match expected {
                    ProviderId::OpenAi => "https://api.openai.com/v1",
                    ProviderId::DeepSeek => "https://api.deepseek.com",
                    ProviderId::Anthropic | ProviderId::ClaudeSubscription => {
                        "https://api.anthropic.com/v1"
                    }
                    ProviderId::Kimi => "https://api.moonshot.cn/v1",
                    ProviderId::Qwen => "https://dashscope.aliyuncs.com/compatible-mode/v1",
                    ProviderId::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
                    ProviderId::MiniMax => "https://api.minimaxi.com/v1",
                    ProviderId::Glm => "https://open.bigmodel.cn/api/paas/v4",
                    ProviderId::OpenRouter => "https://openrouter.ai/api/v1",
                    ProviderId::OpenCodeGo => "https://opencode.ai/zen/go/v1",
                    ProviderId::AlibabaCoding => {
                        "https://coding-intl.dashscope.aliyuncs.com/apps/anthropic/v1"
                    }
                    ProviderId::BigModelCoding => "https://open.bigmodel.cn/api/anthropic/v1",
                    ProviderId::ZaiCoding => "https://api.z.ai/api/anthropic/v1",
                    ProviderId::MiniMaxCoding => "https://api.minimaxi.com/anthropic/v1",
                    ProviderId::KimiCoding => "https://api.kimi.com/coding/v1",
                    ProviderId::ChatGptSubscription
                    | ProviderId::OpenCodeZen
                    | ProviderId::Xai
                    | ProviderId::Zai => unreachable!("not a selectable Chat binding"),
                };
                assert_eq!(
                    model_hint,
                    format!("{}fixture-model", expected.model_prefix())
                );
                assert_eq!(config.base_url.as_deref(), Some(expected_base_url));
                if expected == ProviderId::ClaudeSubscription {
                    assert!(!config.headers().is_empty());
                } else {
                    assert!(config.headers().is_empty());
                }
                assert_eq!(config.timeout, Duration::from_secs(30));
                assert_eq!(config.max_retries, 0);
                assert!(!config.load_env);
            }
            _ => panic!("factory selected the wrong protocol for {provider:?}"),
        }
    }
}

#[tokio::test]
async fn claude_subscription_uses_setup_token_bearer_auth_and_required_headers() {
    let factory = LiterProviderFactory::new();
    let plan = factory
        .build_plan(
            binding(
                ProviderId::ClaudeSubscription,
                ProviderParameters::default(),
            ),
            CredentialLease::new(SecretValue::from_utf8(
                "sk-ant-oat01-fixture-setup-token".to_owned(),
            )),
        )
        .expect("Claude subscription setup token builds a client plan");

    let ClientPlan::Chat { config, .. } = plan else {
        panic!("Claude subscription uses the Anthropic Messages protocol")
    };
    assert!(
        config.api_key.expose_secret().is_empty(),
        "a setup token must never be configured as Anthropic x-api-key auth"
    );
    let credential = config
        .credential_provider
        .as_ref()
        .expect("Claude subscription must resolve a Bearer credential")
        .resolve()
        .await
        .expect("static setup token resolves");
    let Credential::BearerToken(token) = credential else {
        panic!("Claude setup token must resolve as Bearer auth")
    };
    assert_eq!(
        token.expose_secret(),
        SecretString::from("sk-ant-oat01-fixture-setup-token".to_owned()).expose_secret()
    );
    for (name, value) in [
        (
            "anthropic-beta",
            "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,web-fetch-2025-09-10",
        ),
        ("user-agent", "claude-cli/2.1.75 (external, cli)"),
        ("x-app", "cli"),
        ("anthropic-dangerous-direct-browser-access", "true"),
    ] {
        assert!(
            config
                .headers()
                .iter()
                .any(|(header, actual)| header == name && actual == value),
            "missing Claude subscription header {name}"
        );
    }
    assert!(!config.load_env);
}

#[tokio::test]
async fn factory_constructs_one_non_routing_default_client_for_each_allowlisted_provider() {
    let factory = LiterProviderFactory::new();

    for provider in ProviderId::ALL {
        let credential = if provider == ProviderId::ChatGptSubscription {
            oauth_lease()
        } else {
            api_key_lease()
        };
        let client = factory
            .build(binding(provider, ProviderParameters::default()), credential)
            .await
            .expect("fixed DefaultClient construction");
        let capabilities = client.capabilities();
        assert!(capabilities.tool_calling);
        assert!(!capabilities.parallel_tool_calls);
        assert!(!capabilities.streaming);
    }
}

#[test]
fn binding_timeout_and_bounded_retry_are_forwarded_without_changing_provider_or_model() {
    let factory = LiterProviderFactory::new();
    let parameters: ProviderParameters = serde_json::from_value(json!({
        "temperature": null,
        "max_tokens": null,
        "timeout_seconds": 7,
        "retry_count": 2,
        "provider_specific": {}
    }))
    .expect("valid bounded parameter DTO");

    for provider in [ProviderId::DeepSeek, ProviderId::ChatGptSubscription] {
        let credential = if provider == ProviderId::ChatGptSubscription {
            oauth_lease()
        } else {
            api_key_lease()
        };
        let plan = factory
            .build_plan(binding(provider, parameters.clone()), credential)
            .expect("bound timeout and retry plan");
        let config = match plan {
            ClientPlan::Chat { config, .. } | ClientPlan::Responses { config, .. } => config,
        };
        assert_eq!(config.timeout, Duration::from_secs(7));
        assert_eq!(config.max_retries, 2);
        assert!(!config.load_env);
    }
}

#[test]
fn unproven_or_invalid_binding_parameters_are_rejected_not_silently_dropped() {
    let factory = LiterProviderFactory::new();

    for (parameter, field) in [
        (
            json!({
                "temperature": 0.2,
                "max_tokens": null,
                "timeout_seconds": 30,
                "retry_count": 0,
                "provider_specific": {}
            }),
            ProviderParameterKey::Temperature,
        ),
        (
            json!({
                "temperature": null,
                "max_tokens": 100,
                "timeout_seconds": 30,
                "retry_count": 0,
                "provider_specific": {}
            }),
            ProviderParameterKey::MaxTokens,
        ),
        (
            json!({
                "temperature": null,
                "max_tokens": null,
                "timeout_seconds": 0,
                "retry_count": 0,
                "provider_specific": {}
            }),
            ProviderParameterKey::Timeout,
        ),
        (
            json!({
                "temperature": null,
                "max_tokens": null,
                "timeout_seconds": 30,
                "retry_count": 3,
                "provider_specific": {}
            }),
            ProviderParameterKey::Retry,
        ),
    ] {
        let parameters = serde_json::from_value(parameter).expect("valid parameter DTO");
        let error =
            match factory.build_plan(binding(ProviderId::DeepSeek, parameters), api_key_lease()) {
                Err(error) => error,
                Ok(_) => panic!("unproven or unbounded parameter must fail closed"),
            };
        assert_eq!(error.code(), ProviderErrorCode::ModelIncompatible.as_str());
        assert_eq!(error.field(), Some(&ProviderField::Parameter(field)));
    }
}

#[test]
fn credential_kind_and_profile_generation_mismatches_cannot_construct_a_client() {
    let factory = LiterProviderFactory::new();
    let mut wrong_kind = binding(ProviderId::DeepSeek, ProviderParameters::default());
    wrong_kind.credential_generation = CredentialGeneration::new(
        wrong_kind.profile_id,
        1,
        ys_agent_core::CredentialKind::OAuthConnection,
    )
    .expect("wrong-kind fixture generation");

    let error = match factory.build_plan(wrong_kind, api_key_lease()) {
        Err(error) => error,
        Ok(_) => panic!("API-key provider must reject OAuth generation"),
    };
    assert_eq!(
        error.code(),
        ProviderErrorCode::AuthenticationInvalid.as_str()
    );
    assert_eq!(error.field(), Some(&ProviderField::Credential));

    let mut wrong_model = binding(ProviderId::DeepSeek, ProviderParameters::default());
    wrong_model.model = ProviderModelId::new(ProviderId::Xai, "xai/fixture-model")
        .expect("valid foreign model fixture");
    let error = match factory.build_plan(wrong_model, api_key_lease()) {
        Err(error) => error,
        Ok(_) => panic!("binding provider and model prefix must agree"),
    };
    assert_eq!(error.code(), ProviderErrorCode::InvalidModelPrefix.as_str());
    assert_eq!(error.field(), Some(&ProviderField::Model));

    let error = match factory.build_plan(
        binding(
            ProviderId::ChatGptSubscription,
            ProviderParameters::default(),
        ),
        api_key_lease(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("ChatGPT requires a connected OAuth bundle"),
    };
    assert_eq!(error.code(), ProviderErrorCode::OAuthNotConnected.as_str());
}

#[test]
fn provider_error_normalizer_classifies_known_liter_failures_without_echoing_canaries() {
    const CANARY: &str = "provider-echo-canary-must-not-leak";
    let cases = [
        (
            LiterLlmError::Authentication {
                message: CANARY.to_owned(),
                status: 401,
            },
            ProviderErrorCode::AuthenticationInvalid,
            ProviderErrorCategory::Authentication,
            ProviderRetryability::Never,
            Some(ProviderField::Credential),
            ProviderRemediation::ReturnToEdit,
        ),
        (
            LiterLlmError::NotFound {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::ModelNotFound,
            ProviderErrorCategory::Model,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ReturnToEdit,
        ),
        (
            LiterLlmError::RateLimited {
                message: CANARY.to_owned(),
                retry_after: None,
            },
            ProviderErrorCode::RateLimited,
            ProviderErrorCategory::RateLimit,
            ProviderRetryability::Bounded,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        (
            LiterLlmError::Timeout,
            ProviderErrorCode::Timeout,
            ProviderErrorCategory::Timeout,
            ProviderRetryability::Bounded,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        (
            LiterLlmError::ServerError {
                message: CANARY.to_owned(),
                status: 500,
            },
            ProviderErrorCode::Server,
            ProviderErrorCategory::Server,
            ProviderRetryability::Bounded,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        (
            LiterLlmError::ServiceUnavailable {
                message: CANARY.to_owned(),
                status: 503,
            },
            ProviderErrorCode::Server,
            ProviderErrorCategory::Server,
            ProviderRetryability::Bounded,
            Some(ProviderField::Model),
            ProviderRemediation::Retry,
        ),
        (
            LiterLlmError::BadRequest {
                message: CANARY.to_owned(),
                status: 422,
            },
            ProviderErrorCode::ModelIncompatible,
            ProviderErrorCategory::Capability,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::ContextWindowExceeded {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::ModelIncompatible,
            ProviderErrorCategory::Capability,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::ContentPolicy {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::ModelIncompatible,
            ProviderErrorCategory::Capability,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::EndpointNotSupported {
                endpoint: CANARY.to_owned(),
                provider: CANARY.to_owned(),
            },
            ProviderErrorCode::ModelIncompatible,
            ProviderErrorCategory::Capability,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::BudgetExceeded {
                message: CANARY.to_owned(),
                model: Some(CANARY.to_owned()),
            },
            ProviderErrorCode::ModelIncompatible,
            ProviderErrorCategory::Capability,
            ProviderRetryability::Never,
            Some(ProviderField::Model),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::Streaming {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::InvalidHeader {
                name: CANARY.to_owned(),
                reason: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::HookRejected {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::InternalError {
                message: CANARY.to_owned(),
            },
            ProviderErrorCode::Internal,
            ProviderErrorCategory::Internal,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ContactSupport,
        ),
        (
            LiterLlmError::OutboundForbidden {
                url: CANARY.to_owned(),
                reason: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::IdempotencyConflict {
                key: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
        (
            LiterLlmError::IdempotencyInFlight {
                key: CANARY.to_owned(),
            },
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderErrorCategory::Protocol,
            ProviderRetryability::Never,
            Some(ProviderField::Validation),
            ProviderRemediation::ValidateProfile,
        ),
    ];

    for (source, code, category, retryability, field, remediation) in cases {
        let normalized = ProviderErrorNormalizer::from_liter(source);
        assert_eq!(normalized.code(), code.as_str());
        assert_eq!(normalized.category(), category);
        assert_eq!(normalized.retryability(), retryability);
        assert_eq!(normalized.field(), field.as_ref());
        assert_eq!(normalized.remediation(), remediation);
        assert!(!format!("{normalized:?}").contains(CANARY));

        let core = ProviderErrorNormalizer::into_core(normalized);
        assert_eq!(core.code(), code.as_str());
        assert!(!format!("{core:?}").contains(CANARY));
        assert!(!core.to_string().contains(CANARY));
    }
}

#[test]
fn provider_error_normalizer_drops_network_and_serialization_details() {
    const CANARY: &str = "network-and-serialization-canary-must-not-leak";
    let network = reqwest::Client::new()
        .get("http://[::1")
        .build()
        .expect_err("malformed URL must not build");
    let serialization =
        serde_json::from_str::<serde_json::Value>("{").expect_err("malformed JSON fixture");
    let cases = [
        (LiterLlmError::Network(network), ProviderErrorCode::Network),
        (
            LiterLlmError::Serialization(serialization),
            ProviderErrorCode::ProtocolInvalidResponse,
        ),
    ];

    for (source, code) in cases {
        let normalized = ProviderErrorNormalizer::from_liter(source);
        assert_eq!(normalized.code(), code.as_str());
        assert!(!format!("{normalized:?}").contains(CANARY));
    }
}

#[test]
fn provider_error_normalizer_cancellation_is_non_retryable_and_never_succeeds() {
    let cancelled = ProviderErrorNormalizer::cancelled();

    assert_eq!(
        cancelled.code(),
        ProviderErrorCode::OperationCancelled.as_str()
    );
    assert_eq!(cancelled.category(), ProviderErrorCategory::Operation);
    assert_eq!(cancelled.retryability(), ProviderRetryability::Never);
    assert_eq!(cancelled.field(), None);
    assert_eq!(cancelled.remediation(), ProviderRemediation::ReturnToEdit);
    assert_eq!(
        ProviderErrorNormalizer::into_core(cancelled).code(),
        ProviderErrorCode::OperationCancelled.as_str()
    );
}

fn base64_url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        }
    }
    output
}
