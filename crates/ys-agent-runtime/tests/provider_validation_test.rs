use serde_json::json;
use ys_agent_core::{
    CredentialGeneration, CredentialViewStatus, ProfileId, ProfileName, ProviderErrorCode,
    ProviderField, ProviderId, ProviderModelId, ProviderParameterKey, ProviderParameters,
    ProviderRemediation,
};
use ys_agent_runtime::provider::{
    catalog::GovernedProviderCatalog,
    validation::{LocalProfileValidation, LocalProfileValidationRequest, LocalProfileValidator},
};

fn parameters(value: serde_json::Value) -> ProviderParameters {
    serde_json::from_value(value).expect("parameter fixture has the core shape")
}

fn credential(profile_id: ProfileId, provider: ProviderId) -> CredentialGeneration {
    CredentialGeneration::new(profile_id, 1, provider.required_credential_kind())
        .expect("credential belongs to the profile and provider")
}

fn request<'a>(
    profile_id: ProfileId,
    name: &'a ProfileName,
    provider: ProviderId,
    model: &'a ProviderModelId,
    parameters: &'a ProviderParameters,
    credential: (CredentialViewStatus, Option<CredentialGeneration>),
    existing_names: &'a [(ProfileId, ProfileName)],
) -> LocalProfileValidationRequest<'a> {
    LocalProfileValidationRequest {
        profile_id,
        name,
        provider,
        model,
        parameters,
        credential_status: credential.0,
        credential_generation: credential.1,
        existing_names,
    }
}

fn has(
    result: &LocalProfileValidation,
    code: ProviderErrorCode,
    field: Option<&ProviderField>,
    remediation: ProviderRemediation,
) -> bool {
    result.violations().iter().any(|violation| {
        let error = violation.error();
        error.code() == code.as_str()
            && error.field() == field
            && error.remediation() == remediation
    })
}

#[test]
fn local_validation_accepts_each_governed_provider_without_network_inputs() {
    let validator = LocalProfileValidator::new(GovernedProviderCatalog::default());

    for provider in ProviderId::ALL {
        let profile_id = ProfileId::new();
        let name = ProfileName::new(format!("{provider:?} local")).expect("valid name");
        let model = ProviderModelId::new(
            provider,
            format!("{}representative-model", provider.model_prefix()),
        )
        .expect("governed prefix");
        let parameters = ProviderParameters::default();
        let result = validator.validate_local(request(
            profile_id,
            &name,
            provider,
            &model,
            &parameters,
            (
                CredentialViewStatus::Saved,
                Some(credential(profile_id, provider)),
            ),
            &[],
        ));

        assert!(result.is_valid(), "{provider:?}: {result:?}");
        assert!(result.violations().is_empty());
    }
}

#[test]
fn local_validation_collects_field_errors_before_any_probe_can_start() {
    let validator = LocalProfileValidator::new(GovernedProviderCatalog::default());
    let profile_id = ProfileId::new();
    let conflicting_profile_id = ProfileId::new();
    let name = ProfileName::new("duplicate").expect("valid name");
    let model = ProviderModelId::new(ProviderId::Anthropic, "anthropic/model")
        .expect("valid foreign model");
    let parameters = parameters(json!({
        "temperature": 3.0,
        "max_tokens": 0,
        "timeout_seconds": 0,
        "retry_count": 3,
        "provider_specific": { "base_url": true }
    }));
    let existing_names = [(conflicting_profile_id, name.clone())];

    let result = validator.validate_local(request(
        profile_id,
        &name,
        ProviderId::DeepSeek,
        &model,
        &parameters,
        (CredentialViewStatus::Missing, None),
        &existing_names,
    ));

    assert!(!result.is_valid());
    assert!(has(
        &result,
        ProviderErrorCode::ProfileNameConflict,
        Some(&ProviderField::ProfileName),
        ProviderRemediation::ReturnToEdit,
    ));
    assert!(has(
        &result,
        ProviderErrorCode::InvalidModelPrefix,
        Some(&ProviderField::Model),
        ProviderRemediation::ReturnToEdit,
    ));
    assert!(has(
        &result,
        ProviderErrorCode::CredentialMissing,
        Some(&ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    ));
    for key in [
        ProviderParameterKey::Temperature,
        ProviderParameterKey::MaxTokens,
        ProviderParameterKey::Timeout,
        ProviderParameterKey::Retry,
        ProviderParameterKey::ProviderSpecific("base_url".to_owned()),
    ] {
        assert!(has(
            &result,
            ProviderErrorCode::ModelIncompatible,
            Some(&ProviderField::Parameter(key)),
            ProviderRemediation::ReturnToEdit,
        ));
    }
}

#[test]
fn conditional_parameters_remain_field_errors_until_model_evidence_exists() {
    let validator = LocalProfileValidator::new(GovernedProviderCatalog::default());
    let profile_id = ProfileId::new();
    let name = ProfileName::new("conditional parameters").expect("valid name");
    let model = ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model").expect("valid model");
    let parameters = parameters(json!({
        "temperature": 0.5,
        "max_tokens": 100,
        "timeout_seconds": 30,
        "retry_count": 2,
        "provider_specific": {}
    }));

    let result = validator.validate_local(request(
        profile_id,
        &name,
        ProviderId::DeepSeek,
        &model,
        &parameters,
        (
            CredentialViewStatus::Saved,
            Some(credential(profile_id, ProviderId::DeepSeek)),
        ),
        &[],
    ));

    for key in [
        ProviderParameterKey::Temperature,
        ProviderParameterKey::MaxTokens,
    ] {
        assert!(has(
            &result,
            ProviderErrorCode::ModelIncompatible,
            Some(&ProviderField::Parameter(key)),
            ProviderRemediation::ValidateProfile,
        ));
    }
    assert!(!has(
        &result,
        ProviderErrorCode::ModelIncompatible,
        Some(&ProviderField::Parameter(ProviderParameterKey::Retry)),
        ProviderRemediation::ReturnToEdit,
    ));
}

#[test]
fn expired_chatgpt_connection_is_a_reauthorizable_local_blocker() {
    let validator = LocalProfileValidator::new(GovernedProviderCatalog::default());
    let profile_id = ProfileId::new();
    let name = ProfileName::new("subscription").expect("valid name");
    let model = ProviderModelId::new(ProviderId::ChatGptSubscription, "chatgpt/model")
        .expect("valid model");
    let parameters = ProviderParameters::default();
    let result = validator.validate_local(request(
        profile_id,
        &name,
        ProviderId::ChatGptSubscription,
        &model,
        &parameters,
        (
            CredentialViewStatus::Expired,
            Some(credential(profile_id, ProviderId::ChatGptSubscription)),
        ),
        &[],
    ));

    assert!(has(
        &result,
        ProviderErrorCode::OAuthNotConnected,
        Some(&ProviderField::OAuth),
        ProviderRemediation::Reauthorize,
    ));
}

#[test]
fn credential_generation_must_belong_to_the_exact_profile_and_authentication_kind() {
    let validator = LocalProfileValidator::default();
    let profile_id = ProfileId::new();
    let name = ProfileName::new("credential binding").expect("valid name");
    let model = ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model").expect("valid model");
    let parameters = ProviderParameters::default();
    let foreign_generation = credential(ProfileId::new(), ProviderId::DeepSeek);
    let result = validator.validate_local(request(
        profile_id,
        &name,
        ProviderId::DeepSeek,
        &model,
        &parameters,
        (CredentialViewStatus::Saved, Some(foreign_generation)),
        &[],
    ));

    assert!(has(
        &result,
        ProviderErrorCode::AuthenticationInvalid,
        Some(&ProviderField::Credential),
        ProviderRemediation::ReturnToEdit,
    ));
}
