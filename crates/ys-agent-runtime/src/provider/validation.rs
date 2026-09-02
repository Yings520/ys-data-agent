//! Staged Provider Profile validation.
//!
//! This layer deliberately has no network, Vault, model-discovery, or client-factory dependency.
//! Compatibility probing is a later concern; callers must not start it when this validator returns
//! a field violation.

use serde_json::json;
use ys_agent_core::{
    AgentAction, AssistantToolCall, CompatibilityEvidence, ContextManifest, CoreError,
    CredentialGeneration, CredentialViewStatus, ModelMessage, ModelProvider, ModelRequest,
    ModelRole, OAuthConnectionStatus, ParameterApplicability, ProfileId, ProfileName,
    ProfileRevision, ProviderErrorCode, ProviderField, ProviderId, ProviderManagementError,
    ProviderModelId, ProviderParameterKey, ProviderParameters, ProviderRemediation, ProviderResult,
    Sensitivity, SideEffect, ToolRisk, ToolSpec, ValidationVersions,
};

use super::catalog::{GovernedProviderCatalog, ProviderCatalogEntry};
use super::evidence::GOVERNED_LITER_LLM_VERSION;

/// The highest retry count accepted by the fixed `LiterProviderFactory` transport policy.
const MAX_RETRY_COUNT: u32 = 2;
const MIN_TEMPERATURE: f32 = 0.0;
const MAX_TEMPERATURE: f32 = 2.0;

/// The fixed protocol revision for the synthetic compatibility probe. Changing it invalidates
/// persisted evidence through `ValidationVersions`.
pub const COMPATIBILITY_PROBE_SCHEMA_VERSION: &str = "provider-compatibility-probe-v1";
const COMPATIBILITY_PROBE_TOOL: &str = "ysda_compatibility_probe";
const COMPATIBILITY_PROBE_TOOL_VERSION: &str = "v1";
const COMPATIBILITY_CONTEXT_TOKEN_BUDGET: u32 = 128;

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

/// Non-secret inputs to the model-level half of staged compatibility validation.
///
/// `observed_context_limit` must come from model-level directory, response, or probe evidence.
/// A catalog declaration or client capability hint cannot fill in an unknown value.
#[derive(Debug, Clone)]
pub struct CompatibilityProbeRequest<'a> {
    pub revision: &'a ProfileRevision,
    pub local_validation: &'a LocalProfileValidation,
    pub oauth_status: Option<OAuthConnectionStatus>,
    pub observed_context_limit: Option<ModelContextLimit>,
    /// Version of the adapter's protocol codec. It participates in the persisted evidence digest.
    pub codec_version: &'a str,
}

/// A context limit observed at the model level. The source-specific constructors deliberately do
/// not admit a Provider catalog declaration or a generic client capability hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelContextLimit(u32);

impl ModelContextLimit {
    pub const fn from_directory(tokens: u32) -> Self {
        Self(tokens)
    }

    pub const fn from_probe_response(tokens: u32) -> Self {
        Self(tokens)
    }

    pub const fn from_approved_evidence(tokens: u32) -> Self {
        Self(tokens)
    }

    const fn tokens(self) -> u32 {
        self.0
    }
}

/// The safe, immutable outcome of a successful compatibility probe.
///
/// It deliberately excludes model responses, provider call IDs, and request content. Only the
/// closed core evidence digest, its input versions, and the verified numeric context limit leave
/// this boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeEvidence {
    compatibility: CompatibilityEvidence,
    context_limit: u32,
    versions: ValidationVersions,
}

impl ProbeEvidence {
    pub fn compatibility(&self) -> &CompatibilityEvidence {
        &self.compatibility
    }

    pub const fn context_limit(&self) -> u32 {
        self.context_limit
    }

    pub fn versions(&self) -> &ValidationVersions {
        &self.versions
    }
}

/// The networked half of staged Profile validation.
///
/// It owns a governed catalog solely to bind its digest into evidence. It never uses catalog
/// capability declarations as a substitute for a verified model result.
#[derive(Debug, Clone)]
pub struct CompatibilityValidator {
    catalog: GovernedProviderCatalog,
}

impl CompatibilityValidator {
    pub fn new(catalog: GovernedProviderCatalog) -> Self {
        Self { catalog }
    }

    /// Performs exactly one fixed synthetic tool-call round trip and one continuation. No user
    /// query, customer context, artifact, clipboard value, schema, or history is accepted here.
    pub async fn probe_model(
        &self,
        request: CompatibilityProbeRequest<'_>,
        client: &dyn ModelProvider,
    ) -> ProviderResult<ProbeEvidence> {
        if let Some(violation) = request.local_validation.violations().first() {
            return Err(violation.error().clone());
        }

        if request.revision.provider() == ProviderId::ChatGptSubscription
            && request.oauth_status != Some(OAuthConnectionStatus::Connected)
        {
            return Err(provider_error(
                ProviderErrorCode::OAuthNotConnected,
                ProviderField::OAuth,
                ProviderRemediation::Reauthorize,
            ));
        }

        let context_limit = request
            .observed_context_limit
            .map(ModelContextLimit::tokens)
            .filter(|limit| *limit > 0)
            .ok_or_else(|| {
                provider_error(
                    ProviderErrorCode::ModelIncompatible,
                    ProviderField::Model,
                    ProviderRemediation::ValidateProfile,
                )
            })?;

        let capabilities = client.capabilities();
        if !capabilities.tool_calling || !capabilities.structured_outputs {
            return Err(provider_error(
                ProviderErrorCode::ModelIncompatible,
                ProviderField::Model,
                ProviderRemediation::ValidateProfile,
            ));
        }
        if request.codec_version.trim().is_empty() {
            return Err(provider_error(
                ProviderErrorCode::ProtocolIncompatible,
                ProviderField::Validation,
                ProviderRemediation::ValidateProfile,
            ));
        }

        let first_response = client
            .complete(initial_probe_request(request.revision))
            .await
            .map_err(normalize_probe_error)?;
        let provider_call_id = required_probe_call_id(first_response.action)?;

        let continuation_response = client
            .complete(continuation_probe_request(
                request.revision,
                provider_call_id.as_str(),
            ))
            .await
            .map_err(normalize_probe_error)?;
        if !matches!(continuation_response.action, AgentAction::Respond { .. }) {
            return Err(provider_error(
                ProviderErrorCode::ProtocolInvalidResponse,
                ProviderField::Validation,
                ProviderRemediation::ValidateProfile,
            ));
        }

        let versions = ValidationVersions::new(
            self.catalog.digest(),
            COMPATIBILITY_PROBE_SCHEMA_VERSION,
            GOVERNED_LITER_LLM_VERSION,
            request.codec_version,
        );
        Ok(ProbeEvidence {
            compatibility: CompatibilityEvidence::passing(
                request.revision.validation_inputs(versions.clone()),
            ),
            context_limit,
            versions,
        })
    }
}

impl Default for CompatibilityValidator {
    fn default() -> Self {
        Self::new(GovernedProviderCatalog::default())
    }
}

fn initial_probe_request(revision: &ProfileRevision) -> ModelRequest {
    ModelRequest {
        model: revision.model().as_str().to_owned(),
        messages: vec![synthetic_system_message(), synthetic_user_message()],
        tools: vec![synthetic_probe_tool()],
        context_manifest: synthetic_context_manifest(),
        temperature: None,
    }
}

fn continuation_probe_request(revision: &ProfileRevision, provider_call_id: &str) -> ModelRequest {
    ModelRequest {
        model: revision.model().as_str().to_owned(),
        messages: vec![
            synthetic_system_message(),
            ModelMessage {
                role: ModelRole::Assistant,
                content: String::new(),
                tool_call_id: None,
                name: None,
                assistant_tool_call: Some(AssistantToolCall {
                    provider_call_id: provider_call_id.to_owned(),
                    name: COMPATIBILITY_PROBE_TOOL.to_owned(),
                    arguments: json!({}),
                }),
            },
            ModelMessage {
                role: ModelRole::Tool,
                content: r#"{"status":"ok"}"#.to_owned(),
                tool_call_id: Some(provider_call_id.to_owned()),
                name: Some(COMPATIBILITY_PROBE_TOOL.to_owned()),
                assistant_tool_call: None,
            },
        ],
        tools: vec![synthetic_probe_tool()],
        context_manifest: synthetic_context_manifest(),
        temperature: None,
    }
}

fn synthetic_system_message() -> ModelMessage {
    ModelMessage {
        role: ModelRole::System,
        content: "Perform the fixed compatibility probe tool call only.".to_owned(),
        tool_call_id: None,
        name: None,
        assistant_tool_call: None,
    }
}

fn synthetic_user_message() -> ModelMessage {
    ModelMessage {
        role: ModelRole::User,
        content: "Call ysda_compatibility_probe with an empty object.".to_owned(),
        tool_call_id: None,
        name: None,
        assistant_tool_call: None,
    }
}

fn synthetic_probe_tool() -> ToolSpec {
    ToolSpec {
        name: COMPATIBILITY_PROBE_TOOL.to_owned(),
        description: "Fixed provider compatibility probe; returns a static result.".to_owned(),
        input_schema: json!({"type": "object", "additionalProperties": false}),
        output_schema: json!({
            "type": "object",
            "properties": {"status": {"const": "ok"}},
            "required": ["status"],
            "additionalProperties": false
        }),
        risk: ToolRisk::Low,
        side_effect: SideEffect::None,
        idempotent: true,
        timeout_ms: 1_000,
        max_output_bytes: 64,
        required_permissions: Vec::new(),
        input_sensitivity: Sensitivity::Internal,
        output_sensitivity: Sensitivity::Internal,
        version: COMPATIBILITY_PROBE_TOOL_VERSION.to_owned(),
    }
}

fn synthetic_context_manifest() -> ContextManifest {
    let mut manifest = ContextManifest::empty(COMPATIBILITY_CONTEXT_TOKEN_BUDGET);
    manifest.tool_view_version = COMPATIBILITY_PROBE_SCHEMA_VERSION.to_owned();
    manifest
}

fn required_probe_call_id(action: AgentAction) -> ProviderResult<String> {
    let AgentAction::CallTool { call } = action else {
        return Err(provider_error(
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ));
    };
    if call.name != COMPATIBILITY_PROBE_TOOL
        || call.version != COMPATIBILITY_PROBE_TOOL_VERSION
        || call.arguments != json!({})
    {
        return Err(provider_error(
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ));
    }
    let Some(provider_call_id) = call.provider_call_id else {
        return Err(provider_error(
            ProviderErrorCode::ProtocolInvalidToolCallId,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ));
    };
    if provider_call_id.trim().is_empty() {
        return Err(provider_error(
            ProviderErrorCode::ProtocolInvalidToolCallId,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ));
    }
    Ok(provider_call_id)
}

fn normalize_probe_error(error: CoreError) -> ProviderManagementError {
    let (code, field, remediation) = match error.code() {
        "provider.auth.invalid" => (
            ProviderErrorCode::AuthenticationInvalid,
            ProviderField::Credential,
            ProviderRemediation::ReturnToEdit,
        ),
        "provider.model.not_found" => (
            ProviderErrorCode::ModelNotFound,
            ProviderField::Model,
            ProviderRemediation::ReturnToEdit,
        ),
        "provider.model.incompatible" => (
            ProviderErrorCode::ModelIncompatible,
            ProviderField::Model,
            ProviderRemediation::ValidateProfile,
        ),
        "provider.rate_limited" => (
            ProviderErrorCode::RateLimited,
            ProviderField::Validation,
            ProviderRemediation::Retry,
        ),
        "provider.timeout" => (
            ProviderErrorCode::Timeout,
            ProviderField::Validation,
            ProviderRemediation::Retry,
        ),
        "provider.network" => (
            ProviderErrorCode::Network,
            ProviderField::Validation,
            ProviderRemediation::Retry,
        ),
        "provider.server" => (
            ProviderErrorCode::Server,
            ProviderField::Validation,
            ProviderRemediation::Retry,
        ),
        "provider.oauth.not_connected" => (
            ProviderErrorCode::OAuthNotConnected,
            ProviderField::OAuth,
            ProviderRemediation::Reauthorize,
        ),
        "provider.operation.cancelled" => (
            ProviderErrorCode::OperationCancelled,
            ProviderField::Validation,
            ProviderRemediation::ReturnToEdit,
        ),
        "provider.protocol.invalid_tool_call_id" => (
            ProviderErrorCode::ProtocolInvalidToolCallId,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ),
        "provider.protocol.invalid_response" => (
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ),
        "provider.protocol.incompatible" => (
            ProviderErrorCode::ProtocolIncompatible,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ),
        "provider.internal" => (
            ProviderErrorCode::Internal,
            ProviderField::Validation,
            ProviderRemediation::ContactSupport,
        ),
        _ => (
            ProviderErrorCode::ProtocolInvalidResponse,
            ProviderField::Validation,
            ProviderRemediation::ValidateProfile,
        ),
    };
    provider_error(code, field, remediation)
}

fn provider_error(
    code: ProviderErrorCode,
    field: ProviderField,
    remediation: ProviderRemediation,
) -> ProviderManagementError {
    ProviderManagementError::new(code, Some(field), remediation)
}
