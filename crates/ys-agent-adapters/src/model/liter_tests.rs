use std::time::Duration;

use secrecy::ExposeSecret;
use serde_json::json;
use ys_agent_core::{
    CredentialGeneration, CredentialLease, ProfileId, ProviderClientBinding, ProviderClientFactory,
    ProviderErrorCode, ProviderField, ProviderId, ProviderModelId, ProviderParameterKey,
    ProviderParameters, SecretValue,
};

use super::{ClientPlan, LiterProviderFactory};

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
fn all_nine_allowlisted_bindings_create_only_fixed_client_plans() {
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
                assert_eq!(
                    model_hint,
                    format!("{}fixture-model", expected.model_prefix())
                );
                assert_eq!(config.base_url, None);
                assert!(config.headers().is_empty());
                assert_eq!(config.timeout, Duration::from_secs(30));
                assert_eq!(config.max_retries, 0);
                assert!(!config.load_env);
            }
            _ => panic!("factory selected the wrong protocol for {provider:?}"),
        }
    }
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
