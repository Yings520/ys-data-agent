//! Pure Provider Profile validation (task 4.1).
//!
//! This layer deliberately has no network, Vault, model-discovery, or client-factory dependency.
//! Compatibility probing is a later concern; callers must not start it when this validator returns
//! a field violation.

use ys_agent_core::{
    CredentialGeneration, CredentialViewStatus, ParameterApplicability, ProfileId, ProfileName,
    ProviderErrorCode, ProviderField, ProviderId, ProviderManagementError, ProviderModelId,
    ProviderParameterKey, ProviderParameters, ProviderRemediation,
};

use super::catalog::{GovernedProviderCatalog, ProviderCatalogEntry};

/// The highest retry count accepted by the fixed `LiterProviderFactory` transport policy.
const MAX_RETRY_COUNT: u32 = 2;
const MIN_TEMPERATURE: f32 = 0.0;
const MAX_TEMPERATURE: f32 = 2.0;

/// Local, non-secret inputs which a service assembles before any Vault access or probe.
///
/// Endpoint, authentication-origin, and redirect fields are intentionally absent: those values
/// are fixed by the catalog and OAuth adapter, so a Profile cannot override them.
#[derive(Debug, Clone)]
pub struct LocalProfileValidationRequest<'a> {
    pub profile_id: ProfileId,
    pub name: &'a ProfileName,
    pub provider: ProviderId,
    pub model: &'a ProviderModelId,
    pub parameters: &'a ProviderParameters,
    pub credential_status: CredentialViewStatus,
    pub credential_generation: Option<CredentialGeneration>,
    /// Known persisted names. The current Profile may appear and is ignored by its matching ID.
    pub existing_names: &'a [(ProfileId, ProfileName)],
}

/// One stable, field-addressable reason why local validation cannot advance to a network probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldViolation {
    error: ProviderManagementError,
}

impl FieldViolation {
    fn new(
        code: ProviderErrorCode,
        field: ProviderField,
        remediation: ProviderRemediation,
    ) -> Self {
        Self {
            error: ProviderManagementError::new(code, Some(field), remediation),
        }
    }

    pub fn error(&self) -> &ProviderManagementError {
        &self.error
    }
}

/// The complete local validation result. Multiple malformed fields remain visible together so a
/// user can repair a Draft without a network round trip for each one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalProfileValidation {
    violations: Vec<FieldViolation>,
}

impl LocalProfileValidation {
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn violations(&self) -> &[FieldViolation] {
        &self.violations
    }
}

/// Validates generic parameter ranges and catalog applicability without attempting a transport
/// call. Conditional parameters remain field errors until task 4.2 has model-level evidence.
#[derive(Debug, Default, Clone, Copy)]
struct ParameterValidator;

impl ParameterValidator {
    fn validate(
        &self,
        entry: &ProviderCatalogEntry,
        parameters: &ProviderParameters,
        violations: &mut Vec<FieldViolation>,
    ) {
        let mut temperature_is_in_range = true;
        if let Some(temperature) = parameters.temperature() {
            temperature_is_in_range = temperature.is_finite()
                && (MIN_TEMPERATURE..=MAX_TEMPERATURE).contains(&temperature);
            if !temperature_is_in_range {
                violations.push(parameter_violation(
                    ProviderParameterKey::Temperature,
                    ProviderRemediation::ReturnToEdit,
                ));
            }
        }

        let mut max_tokens_is_in_range = true;
        if parameters.max_tokens() == Some(0) {
            max_tokens_is_in_range = false;
            violations.push(parameter_violation(
                ProviderParameterKey::MaxTokens,
                ProviderRemediation::ReturnToEdit,
            ));
        }

        let timeout_is_in_range = parameters.timeout_seconds() > 0;
        if !timeout_is_in_range {
            violations.push(parameter_violation(
                ProviderParameterKey::Timeout,
                ProviderRemediation::ReturnToEdit,
            ));
        }

        let retry_is_in_range = parameters.retry_count() <= MAX_RETRY_COUNT;
        if !retry_is_in_range {
            violations.push(parameter_violation(
                ProviderParameterKey::Retry,
                ProviderRemediation::ReturnToEdit,
            ));
        }

        if parameters.temperature().is_some() && temperature_is_in_range {
            validate_applicability(entry, ProviderParameterKey::Temperature, violations);
        }
        if parameters.max_tokens().is_some() && max_tokens_is_in_range {
            validate_applicability(entry, ProviderParameterKey::MaxTokens, violations);
        }
        if timeout_is_in_range {
            validate_applicability(entry, ProviderParameterKey::Timeout, violations);
        }
        if retry_is_in_range {
            validate_applicability(entry, ProviderParameterKey::Retry, violations);
        }
        for key in parameters.provider_specific().keys() {
            validate_applicability(
                entry,
                ProviderParameterKey::ProviderSpecific(key.clone()),
                violations,
            );
        }
    }
}

/// The local half of staged compatibility validation. Its ownership is intentionally limited to
/// deterministic catalog and Draft data; a valid result is only a candidate for the later probe.
#[derive(Debug, Clone)]
pub struct LocalProfileValidator {
    catalog: GovernedProviderCatalog,
    parameters: ParameterValidator,
}

impl LocalProfileValidator {
    pub fn new(catalog: GovernedProviderCatalog) -> Self {
        Self {
            catalog,
            parameters: ParameterValidator,
        }
    }

    pub fn validate_local(
        &self,
        request: LocalProfileValidationRequest<'_>,
    ) -> LocalProfileValidation {
        let mut violations = Vec::new();
        if request
            .existing_names
            .iter()
            .any(|(profile_id, name)| *profile_id != request.profile_id && name == request.name)
        {
            violations.push(FieldViolation::new(
                ProviderErrorCode::ProfileNameConflict,
                ProviderField::ProfileName,
                ProviderRemediation::ReturnToEdit,
            ));
        }

        if request.model.provider() != request.provider {
            violations.push(FieldViolation::new(
                ProviderErrorCode::InvalidModelPrefix,
                ProviderField::Model,
                ProviderRemediation::ReturnToEdit,
            ));
        }

        validate_credential(&request, &mut violations);
        self.parameters.validate(
            self.catalog.entry(request.provider),
            request.parameters,
            &mut violations,
        );
        LocalProfileValidation { violations }
    }
}

impl Default for LocalProfileValidator {
    fn default() -> Self {
        Self::new(GovernedProviderCatalog::default())
    }
}

fn validate_credential(
    request: &LocalProfileValidationRequest<'_>,
    violations: &mut Vec<FieldViolation>,
) {
    match request.credential_status {
        CredentialViewStatus::Saved => {
            if request.credential_generation.is_none() {
                violations.push(FieldViolation::new(
                    ProviderErrorCode::CredentialMissing,
                    ProviderField::Credential,
                    ProviderRemediation::ConfigureCredentialStore,
                ));
            }
        }
        CredentialViewStatus::Missing => violations.push(FieldViolation::new(
            ProviderErrorCode::CredentialMissing,
            ProviderField::Credential,
            ProviderRemediation::ConfigureCredentialStore,
        )),
        CredentialViewStatus::Expired | CredentialViewStatus::Revoked
            if request.provider == ProviderId::ChatGptSubscription =>
        {
            violations.push(FieldViolation::new(
                ProviderErrorCode::OAuthNotConnected,
                ProviderField::OAuth,
                ProviderRemediation::Reauthorize,
            ));
        }
        CredentialViewStatus::Expired | CredentialViewStatus::Revoked => {
            violations.push(FieldViolation::new(
                ProviderErrorCode::AuthenticationInvalid,
                ProviderField::Credential,
                ProviderRemediation::ReturnToEdit,
            ));
        }
        CredentialViewStatus::ProtectionUnavailable => violations.push(FieldViolation::new(
            ProviderErrorCode::CredentialProtectionUnavailable,
            ProviderField::Credential,
            ProviderRemediation::ConfigureCredentialStore,
        )),
        CredentialViewStatus::ReconciliationRequired => violations.push(FieldViolation::new(
            ProviderErrorCode::CredentialProtectionUnavailable,
            ProviderField::Credential,
            ProviderRemediation::ContactSupport,
        )),
    }

    if let Some(generation) = request.credential_generation
        && (generation.profile_id() != request.profile_id
            || generation.kind() != request.provider.required_credential_kind())
    {
        violations.push(FieldViolation::new(
            ProviderErrorCode::AuthenticationInvalid,
            ProviderField::Credential,
            ProviderRemediation::ReturnToEdit,
        ));
    }
}

fn validate_applicability(
    entry: &ProviderCatalogEntry,
    key: ProviderParameterKey,
    violations: &mut Vec<FieldViolation>,
) {
    match entry.parameter_rule(&key) {
        Some(ParameterApplicability::Supported) => {}
        Some(ParameterApplicability::Conditional) => violations.push(parameter_violation(
            key,
            ProviderRemediation::ValidateProfile,
        )),
        Some(ParameterApplicability::Unsupported) | None => {
            violations.push(parameter_violation(key, ProviderRemediation::ReturnToEdit))
        }
    }
}

fn parameter_violation(
    key: ProviderParameterKey,
    remediation: ProviderRemediation,
) -> FieldViolation {
    FieldViolation::new(
        ProviderErrorCode::ModelIncompatible,
        ProviderField::Parameter(key),
        remediation,
    )
}
