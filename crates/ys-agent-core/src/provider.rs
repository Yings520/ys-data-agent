use std::{collections::BTreeMap, fmt, num::NonZeroU64, sync::Arc};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{CoreError, CoreResult, ModelProvider, OperationId, ProfileId, RunId, ValidationId};

/// Provider and subscription-plan identities exposed by the model picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    OpenAi,
    Kimi,
    Qwen,
    Gemini,
    Glm,
    ChatGptSubscription,
    ClaudeSubscription,
    OpenCodeGo,
    AlibabaCoding,
    BigModelCoding,
    ZaiCoding,
    MiniMaxCoding,
    KimiCoding,
    // Retained for backwards-compatible deserialization of profiles created before the
    // The current catalog replaced the original provider list. They are not selectable.
    OpenCodeZen,
    DeepSeek,
    Xai,
    Zai,
    OpenRouter,
    MiniMax,
    Anthropic,
}

impl ProviderId {
    pub const PROVIDERS: [Self; 9] = [
        Self::OpenAi,
        Self::DeepSeek,
        Self::Anthropic,
        Self::Kimi,
        Self::Qwen,
        Self::Gemini,
        Self::MiniMax,
        Self::Glm,
        Self::OpenRouter,
    ];

    pub const PLANS: [Self; 8] = [
        Self::ChatGptSubscription,
        Self::ClaudeSubscription,
        Self::OpenCodeGo,
        Self::AlibabaCoding,
        Self::BigModelCoding,
        Self::ZaiCoding,
        Self::MiniMaxCoding,
        Self::KimiCoding,
    ];

    pub const ALL: [Self; 17] = [
        Self::OpenAi,
        Self::DeepSeek,
        Self::Anthropic,
        Self::Kimi,
        Self::Qwen,
        Self::Gemini,
        Self::MiniMax,
        Self::Glm,
        Self::OpenRouter,
        Self::ChatGptSubscription,
        Self::ClaudeSubscription,
        Self::OpenCodeGo,
        Self::AlibabaCoding,
        Self::BigModelCoding,
        Self::ZaiCoding,
        Self::MiniMaxCoding,
        Self::KimiCoding,
    ];

    pub const LEGACY: [Self; 3] = [Self::OpenCodeZen, Self::Xai, Self::Zai];

    pub const fn is_plan(self) -> bool {
        matches!(
            self,
            Self::ChatGptSubscription
                | Self::ClaudeSubscription
                | Self::OpenCodeGo
                | Self::AlibabaCoding
                | Self::BigModelCoding
                | Self::ZaiCoding
                | Self::MiniMaxCoding
                | Self::KimiCoding
        )
    }

    pub const fn model_prefix(self) -> &'static str {
        match self {
            Self::OpenAi => "openai/",
            Self::Kimi => "moonshot/",
            Self::Qwen => "dashscope/",
            Self::Gemini => "gemini/",
            Self::Glm => "zai/",
            Self::ChatGptSubscription => "chatgpt/",
            Self::ClaudeSubscription => "anthropic/",
            Self::OpenCodeGo => "opencode-go/",
            Self::AlibabaCoding
            | Self::BigModelCoding
            | Self::ZaiCoding
            | Self::MiniMaxCoding
            | Self::KimiCoding => "anthropic/",
            Self::OpenCodeZen => "opencode/",
            Self::DeepSeek => "deepseek/",
            Self::Xai => "xai/",
            Self::Zai => "zai/",
            Self::OpenRouter => "openrouter/",
            Self::MiniMax => "minimax/",
            Self::Anthropic => "anthropic/",
        }
    }

    pub const fn required_credential_kind(self) -> CredentialKind {
        match self {
            Self::ChatGptSubscription => CredentialKind::OAuthConnection,
            _ => CredentialKind::ApiKey,
        }
    }
}

/// A model identifier whose provider prefix has already been verified.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProviderModelId {
    provider: ProviderId,
    value: String,
}

impl ProviderModelId {
    pub const SETUP_PENDING_SUFFIX: &'static str = "__setup_pending__";

    pub fn new(provider: ProviderId, value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        let expected_prefix = provider.model_prefix();
        if !value.starts_with(expected_prefix) || value.len() == expected_prefix.len() {
            return Err(CoreError::validation(
                "provider_model_prefix_mismatch",
                format!("model must use the {} prefix", expected_prefix),
            ));
        }
        if value.chars().any(char::is_whitespace) {
            return Err(CoreError::validation(
                "invalid_provider_model",
                "provider model must not contain whitespace",
            ));
        }
        Ok(Self { provider, value })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn setup_pending(provider: ProviderId) -> Self {
        Self::new(
            provider,
            format!("{}{}", provider.model_prefix(), Self::SETUP_PENDING_SUFFIX),
        )
        .expect("the internal setup-pending model uses the provider prefix")
    }

    pub fn is_setup_pending(&self) -> bool {
        self.value
            .strip_prefix(self.provider.model_prefix())
            .is_some_and(|value| value == Self::SETUP_PENDING_SUFFIX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        if value.trim().is_empty() || value != value.trim() {
            return Err(CoreError::validation(
                "invalid_profile_name",
                "profile name must be non-empty and must not have surrounding whitespace",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    OAuthConnection,
}

/// A non-sensitive reference to one immutable credential generation. It deliberately has no
/// vault locator and cannot carry a secret value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialGeneration {
    profile_id: ProfileId,
    generation: NonZeroU64,
    kind: CredentialKind,
}

impl CredentialGeneration {
    pub fn new(profile_id: ProfileId, generation: u64, kind: CredentialKind) -> CoreResult<Self> {
        let generation = NonZeroU64::new(generation).ok_or_else(|| {
            CoreError::validation(
                "invalid_credential_generation",
                "credential generation starts at 1",
            )
        })?;
        Ok(Self {
            profile_id,
            generation,
            kind,
        })
    }

    pub fn profile_id(self) -> ProfileId {
        self.profile_id
    }

    pub fn number(self) -> u64 {
        self.generation.get()
    }

    pub fn kind(self) -> CredentialKind {
        self.kind
    }
}

/// Short-lived sensitive input. This type intentionally implements neither Debug, Clone, nor
/// serde serialization; it is not a field of any snapshot or display type.
///
/// ```compile_fail
/// let secret = ys_agent_core::SecretValue::from_utf8("not-for-debug".to_owned());
/// let _ = format!("{secret:?}");
/// ```
///
/// ```compile_fail
/// let secret = ys_agent_core::SecretValue::from_utf8("not-for-serialization".to_owned());
/// let _ = serde_json::to_string(&secret);
/// ```
///
/// ```compile_fail
/// let secret = ys_agent_core::SecretValue::from_utf8("not-for-cloning".to_owned());
/// let _ = secret.clone();
/// ```
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn from_utf8(value: String) -> Self {
        Self(value.into_bytes())
    }

    pub fn with_exposed<T>(&self, use_secret: impl FnOnce(&str) -> T) -> T {
        // Secrets originate from text input and are only exposed within the caller's closure.
        use_secret(std::str::from_utf8(&self.0).expect("SecretValue is constructed from UTF-8"))
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterValue {
    Boolean(bool),
    Integer(i64),
}

impl From<bool> for ParameterValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<i64> for ParameterValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

/// Whether a parameter may be sent to a particular Provider/model combination. The catalog owns
/// the actual per-provider rules; this type prevents adapters from silently treating every field
/// as universally applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterApplicability {
    Supported,
    Unsupported,
    Conditional,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderParameterKey {
    Temperature,
    MaxTokens,
    Timeout,
    Retry,
    ProviderSpecific(String),
}

/// Only non-sensitive, provider-request parameters may enter a configuration snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderParameters {
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    timeout_seconds: u32,
    retry_count: u32,
    provider_specific: BTreeMap<String, ParameterValue>,
}

impl Default for ProviderParameters {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            timeout_seconds: 30,
            retry_count: 0,
            provider_specific: BTreeMap::new(),
        }
    }
}

impl ProviderParameters {
    pub fn with_provider_specific(provider_specific: BTreeMap<String, ParameterValue>) -> Self {
        Self {
            provider_specific,
            ..Self::default()
        }
    }

    pub fn set_temperature(&mut self, temperature: Option<f32>) -> CoreResult<()> {
        if temperature.is_some_and(|value| !value.is_finite()) {
            return Err(CoreError::validation(
                "invalid_provider_parameter",
                "temperature must be finite",
            ));
        }
        self.temperature = temperature;
        Ok(())
    }

    /// Returns the non-sensitive temperature captured by the immutable binding.
    pub const fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    /// Returns the non-sensitive maximum output token request captured by the binding.
    pub const fn max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }

    /// Returns the validated request timeout value captured by the binding.
    pub const fn timeout_seconds(&self) -> u32 {
        self.timeout_seconds
    }

    /// Returns the bounded retry count captured by the binding.
    pub const fn retry_count(&self) -> u32 {
        self.retry_count
    }

    /// Returns explicitly scoped provider parameters. The factory must reject entries that are not
    /// part of the approved model evidence; it must never silently forward arbitrary values.
    pub fn provider_specific(&self) -> &BTreeMap<String, ParameterValue> {
        &self.provider_specific
    }

    pub fn validate_applicability(
        &self,
        rules: &BTreeMap<ProviderParameterKey, ParameterApplicability>,
    ) -> CoreResult<()> {
        for key in self.configured_keys() {
            match rules.get(&key) {
                Some(ParameterApplicability::Supported) => {}
                Some(ParameterApplicability::Conditional) => {
                    return Err(CoreError::validation(
                        "provider_parameter_conditional",
                        format!("parameter {key:?} requires model-level compatibility evidence"),
                    ));
                }
                Some(ParameterApplicability::Unsupported) => {
                    return Err(CoreError::validation(
                        "provider_parameter_unsupported",
                        format!("parameter {key:?} is not supported by this provider/model"),
                    ));
                }
                None => {
                    return Err(CoreError::validation(
                        "provider_parameter_unclassified",
                        format!("parameter {key:?} has no approved provider/model rule"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn configured_keys(&self) -> Vec<ProviderParameterKey> {
        let mut keys = Vec::new();
        if self.temperature.is_some() {
            keys.push(ProviderParameterKey::Temperature);
        }
        if self.max_tokens.is_some() {
            keys.push(ProviderParameterKey::MaxTokens);
        }
        keys.push(ProviderParameterKey::Timeout);
        keys.push(ProviderParameterKey::Retry);
        keys.extend(
            self.provider_specific
                .keys()
                .cloned()
                .map(ProviderParameterKey::ProviderSpecific),
        );
        keys
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileState {
    Draft,
    Ready,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ValidationDigest(String);

impl ValidationDigest {
    fn from_canonical_json(canonical_json: &str) -> Self {
        Self(hex::encode(Sha256::digest(canonical_json.as_bytes())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationInputs {
    profile_id: ProfileId,
    profile_revision: u64,
    provider: ProviderId,
    model: ProviderModelId,
    parameters: ProviderParameters,
    credential_generation: Option<CredentialGeneration>,
    versions: ValidationVersions,
}

/// Versioned, non-sensitive inputs that make a compatibility result reproducible. Any change
/// produces a new digest rather than mutating a previous validation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationVersions {
    catalog_digest: String,
    probe_schema_version: String,
    liter_llm_version: String,
    codec_version: String,
}

impl ValidationVersions {
    pub fn new(
        catalog_digest: impl Into<String>,
        probe_schema_version: impl Into<String>,
        liter_llm_version: impl Into<String>,
        codec_version: impl Into<String>,
    ) -> Self {
        Self {
            catalog_digest: catalog_digest.into(),
            probe_schema_version: probe_schema_version.into(),
            liter_llm_version: liter_llm_version.into(),
            codec_version: codec_version.into(),
        }
    }
}

impl ValidationInputs {
    fn digest(&self) -> ValidationDigest {
        ValidationDigest::from_canonical_json(
            &serde_json::to_string(self).expect("validation input serialization is infallible"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompatibilityEvidence {
    id: ValidationId,
    digest: ValidationDigest,
    passed: bool,
}

impl CompatibilityEvidence {
    pub fn passing(inputs: ValidationInputs) -> Self {
        Self {
            id: ValidationId::new(),
            digest: inputs.digest(),
            passed: true,
        }
    }

    pub fn failing(inputs: ValidationInputs) -> Self {
        Self {
            id: ValidationId::new(),
            digest: inputs.digest(),
            passed: false,
        }
    }

    pub fn matches(&self, inputs: &ValidationInputs) -> bool {
        self.digest == inputs.digest()
    }

    pub fn id(&self) -> ValidationId {
        self.id
    }

    pub fn digest(&self) -> ValidationDigest {
        self.digest.clone()
    }

    pub const fn passed(&self) -> bool {
        self.passed
    }

    pub fn persisted(&self) -> PersistedCompatibilityEvidence {
        PersistedCompatibilityEvidence::from_evidence(self)
    }
}

/// A validated compatibility record read from durable storage. It carries no request, response,
/// credential, or customer data. Repositories must construct it only from the validation row
/// joined to the exact persisted Profile revision and credential generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedCompatibilityEvidence {
    id: ValidationId,
    digest: ValidationDigest,
    passed: bool,
}

/// A non-sensitive revision row assembled by a persistence adapter. `ProfileRevision::hydrate`
/// validates its lifecycle shape before exposing it to the rest of the application.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedProfileRevision {
    pub profile_id: ProfileId,
    pub revision: u64,
    pub provider: ProviderId,
    pub model: ProviderModelId,
    pub parameters: ProviderParameters,
    pub credential_generation: Option<CredentialGeneration>,
    pub state: ProfileState,
    pub validation: Option<PersistedCompatibilityEvidence>,
}

impl PersistedCompatibilityEvidence {
    pub fn new(id: ValidationId, digest: impl Into<String>, passed: bool) -> CoreResult<Self> {
        let digest = digest.into();
        if digest.len() != 64
            || digest
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(CoreError::validation(
                "invalid_validation_digest",
                "persisted validation digests must be lowercase SHA-256 hex",
            ));
        }
        Ok(Self {
            id,
            digest: ValidationDigest(digest),
            passed,
        })
    }

    pub fn from_evidence(evidence: &CompatibilityEvidence) -> Self {
        Self {
            id: evidence.id,
            digest: evidence.digest.clone(),
            passed: evidence.passed,
        }
    }

    fn into_evidence(self) -> CompatibilityEvidence {
        CompatibilityEvidence {
            id: self.id,
            digest: self.digest,
            passed: self.passed,
        }
    }
}

/// An immutable configuration snapshot. State changes are limited to attaching a matching
/// validation result; any configuration or credential change requires a new revision.
/// This snapshot is intentionally serialization-only. Persistence adapters must use a validated
/// DTO and reconstruct it through `draft` plus a matching validation transition, so untrusted
/// state cannot bypass prefix, credential-ownership, or lifecycle checks.
///
/// ```compile_fail
/// let _ = serde_json::from_str::<ys_agent_core::ProfileRevision>("{}");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileRevision {
    profile_id: ProfileId,
    revision: u64,
    provider: ProviderId,
    model: ProviderModelId,
    parameters: ProviderParameters,
    credential_generation: Option<CredentialGeneration>,
    state: ProfileState,
    validation: Option<CompatibilityEvidence>,
}

impl ProfileRevision {
    pub fn draft(
        profile_id: ProfileId,
        revision: u64,
        provider: ProviderId,
        model: ProviderModelId,
        parameters: ProviderParameters,
        credential_generation: Option<CredentialGeneration>,
    ) -> CoreResult<Self> {
        if revision == 0 {
            return Err(CoreError::validation(
                "invalid_profile_revision",
                "profile revisions start at 1",
            ));
        }
        if model.provider() != provider {
            return Err(CoreError::validation(
                "provider_model_owner_mismatch",
                "model belongs to another provider",
            ));
        }
        if let Some(generation) = credential_generation {
            if generation.profile_id() != profile_id {
                return Err(CoreError::validation(
                    "credential_profile_mismatch",
                    "credential generation belongs to another profile",
                ));
            }
            if generation.kind() != provider.required_credential_kind() {
                return Err(CoreError::validation(
                    "credential_kind_mismatch",
                    "credential kind does not match the provider authentication contract",
                ));
            }
        }
        Ok(Self {
            profile_id,
            revision,
            provider,
            model,
            parameters,
            credential_generation,
            state: ProfileState::Draft,
            validation: None,
        })
    }

    /// Rebuilds a revision that was previously accepted by the repository's persistence query.
    /// This is intentionally separate from `accept_validation`: a load must preserve the original
    /// validation identifier and digest without treating stored evidence as a new validation.
    pub fn hydrate(persisted: PersistedProfileRevision) -> CoreResult<Self> {
        let mut revision = Self::draft(
            persisted.profile_id,
            persisted.revision,
            persisted.provider,
            persisted.model,
            persisted.parameters,
            persisted.credential_generation,
        )?;

        match (persisted.state, persisted.validation) {
            (ProfileState::Draft, None) => Ok(revision),
            (ProfileState::Ready, Some(validation)) if validation.passed => {
                if revision.credential_generation.is_none() {
                    return Err(CoreError::validation(
                        "credential_missing",
                        "a persisted ready revision requires a credential generation",
                    ));
                }
                revision.state = ProfileState::Ready;
                revision.validation = Some(validation.into_evidence());
                Ok(revision)
            }
            (ProfileState::Invalid, Some(validation)) if !validation.passed => {
                revision.state = ProfileState::Invalid;
                revision.validation = Some(validation.into_evidence());
                Ok(revision)
            }
            _ => Err(CoreError::validation(
                "invalid_persisted_profile_revision",
                "persisted Profile state and validation outcome must match",
            )),
        }
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn state(&self) -> ProfileState {
        self.state
    }

    pub fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn model(&self) -> &ProviderModelId {
        &self.model
    }

    pub fn parameters(&self) -> &ProviderParameters {
        &self.parameters
    }

    pub fn credential_generation(&self) -> Option<CredentialGeneration> {
        self.credential_generation
    }

    pub fn validation(&self) -> Option<&CompatibilityEvidence> {
        self.validation.as_ref()
    }

    pub fn validation_inputs(&self, versions: ValidationVersions) -> ValidationInputs {
        ValidationInputs {
            profile_id: self.profile_id,
            profile_revision: self.revision,
            provider: self.provider,
            model: self.model.clone(),
            parameters: self.parameters.clone(),
            credential_generation: self.credential_generation,
            versions,
        }
    }

    pub fn accept_validation(
        &mut self,
        evidence: CompatibilityEvidence,
        versions: ValidationVersions,
    ) -> CoreResult<()> {
        if !evidence.passed {
            return Err(CoreError::validation(
                "validation_not_passing",
                "a failed validation cannot make a revision ready",
            ));
        }
        if self.credential_generation.is_none() {
            return Err(CoreError::validation(
                "credential_missing",
                "a revision without credentials cannot become ready",
            ));
        }
        if !evidence.matches(&self.validation_inputs(versions)) {
            return Err(CoreError::validation(
                "validation_digest_stale",
                "validation evidence does not match this revision's current inputs",
            ));
        }
        match self.state {
            ProfileState::Draft | ProfileState::Invalid => {
                self.validation = Some(evidence);
                self.state = ProfileState::Ready;
                Ok(())
            }
            ProfileState::Ready => Err(CoreError::invalid_transition(
                "profile_revision",
                "Ready",
                "Ready",
            )),
        }
    }

    pub fn reject_validation(
        &mut self,
        evidence: CompatibilityEvidence,
        versions: ValidationVersions,
    ) -> CoreResult<()> {
        if evidence.passed {
            return Err(CoreError::validation(
                "validation_not_failing",
                "a passing validation cannot make a revision invalid",
            ));
        }
        if self.state != ProfileState::Draft {
            return Err(CoreError::invalid_transition(
                "profile_revision",
                format!("{:?}", self.state),
                "Invalid",
            ));
        }
        if !evidence.matches(&self.validation_inputs(versions)) {
            return Err(CoreError::validation(
                "validation_digest_stale",
                "validation evidence does not match this revision's current inputs",
            ));
        }
        self.validation = Some(evidence);
        self.state = ProfileState::Invalid;
        Ok(())
    }

    fn ready_evidence(&self) -> CoreResult<&CompatibilityEvidence> {
        match (self.state, self.validation.as_ref()) {
            (ProfileState::Ready, Some(evidence)) if evidence.passed => Ok(evidence),
            _ => Err(CoreError::validation(
                "profile_revision_not_ready",
                "only a ready revision with passing evidence can be activated or bound",
            )),
        }
    }
}

/// Per-profile append-only revision guard. Durable repositories must enforce the same rule.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileHistory {
    profile_id: ProfileId,
    name: ProfileName,
    revisions: Vec<ProfileRevision>,
}

/// The non-sensitive identity aggregate for one locally managed Provider profile.
pub type ProviderProfile = ProfileHistory;

/// Compatibility name used by repository and service contracts.
pub type ProviderProfileRevision = ProfileRevision;

impl ProfileHistory {
    pub fn new(profile_id: ProfileId, name: ProfileName) -> Self {
        Self {
            profile_id,
            name,
            revisions: Vec::new(),
        }
    }

    pub fn append(&mut self, revision: ProfileRevision) -> CoreResult<()> {
        if revision.profile_id != self.profile_id {
            return Err(CoreError::validation(
                "profile_revision_owner_mismatch",
                "revision belongs to another profile",
            ));
        }
        let expected = self.revisions.len() as u64 + 1;
        if revision.revision != expected {
            return Err(CoreError::validation(
                "provider_revision_overwrite_rejected",
                "revisions must append exactly once in increasing order",
            ));
        }
        self.revisions.push(revision);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveProviderSnapshot {
    activation_revision: u64,
    profile_id: ProfileId,
    profile_revision: u64,
    provider: ProviderId,
    model: ProviderModelId,
    parameters: ProviderParameters,
    credential_generation: CredentialGeneration,
    validation_id: ValidationId,
    validation_digest: ValidationDigest,
}

impl ActiveProviderSnapshot {
    pub fn from_ready(revision: &ProfileRevision, activation_revision: u64) -> CoreResult<Self> {
        let evidence = revision.ready_evidence()?;
        let credential_generation = revision.credential_generation.ok_or_else(|| {
            CoreError::validation(
                "credential_missing",
                "a ready revision requires a credential generation",
            )
        })?;
        Ok(Self {
            activation_revision,
            profile_id: revision.profile_id,
            profile_revision: revision.revision,
            provider: revision.provider,
            model: revision.model.clone(),
            parameters: revision.parameters.clone(),
            credential_generation,
            validation_id: evidence.id(),
            validation_digest: evidence.digest.clone(),
        })
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    pub fn activation_revision(&self) -> u64 {
        self.activation_revision
    }

    pub fn profile_revision(&self) -> u64 {
        self.profile_revision
    }

    pub fn validation_id(&self) -> ValidationId {
        self.validation_id
    }

    /// Non-sensitive evidence identity retained by the committed active snapshot. Doctor uses it
    /// to prove the exact active revision was validated without reading a Provider response.
    pub fn validation_digest(&self) -> &ValidationDigest {
        &self.validation_digest
    }

    /// The active snapshot references this immutable generation only; it never exposes a Vault
    /// locator or Credential material.
    pub fn credential_generation(&self) -> CredentialGeneration {
        self.credential_generation
    }
}

/// A singleton active pointer. `None` is the explicit no-active management state.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ActiveProviderSlot {
    current: Option<ActiveProviderSnapshot>,
    activation_revision: u64,
}

impl ActiveProviderSlot {
    pub const fn empty() -> Self {
        Self {
            current: None,
            activation_revision: 0,
        }
    }

    pub fn activate(&mut self, revision: &ProfileRevision) -> CoreResult<()> {
        let activation_revision = self.activation_revision.checked_add(1).ok_or_else(|| {
            CoreError::validation(
                "activation_revision_overflow",
                "activation revision cannot advance",
            )
        })?;
        self.current = Some(ActiveProviderSnapshot::from_ready(
            revision,
            activation_revision,
        )?);
        self.activation_revision = activation_revision;
        Ok(())
    }

    pub fn current(&self) -> Option<&ActiveProviderSnapshot> {
        self.current.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct FingerprintFields<'a> {
    schema_version: u32,
    profile_id: ProfileId,
    profile_revision: u64,
    provider: ProviderId,
    model: &'a ProviderModelId,
    parameters: FingerprintParameters,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct FingerprintParameters {
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    timeout_seconds: u32,
    retry_count: u32,
}

impl From<&ProviderParameters> for FingerprintParameters {
    fn from(parameters: &ProviderParameters) -> Self {
        Self {
            temperature: parameters.temperature,
            max_tokens: parameters.max_tokens,
            timeout_seconds: parameters.timeout_seconds,
            retry_count: parameters.retry_count,
        }
    }
}

/// Canonical, non-sensitive identity for the Provider configuration selected by a Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFingerprint {
    profile_id: ProfileId,
    profile_revision: u64,
    provider: ProviderId,
    model: ProviderModelId,
    canonical_json: String,
    digest: String,
}

impl ProviderFingerprint {
    pub fn from_revision(revision: &ProfileRevision) -> CoreResult<Self> {
        revision.ready_evidence()?;
        let canonical_json = serde_json::to_string(&FingerprintFields {
            schema_version: 1,
            profile_id: revision.profile_id,
            profile_revision: revision.revision,
            provider: revision.provider,
            model: &revision.model,
            parameters: FingerprintParameters::from(&revision.parameters),
        })
        .expect("fingerprint serialization is infallible");
        let digest = hex::encode(Sha256::digest(canonical_json.as_bytes()));
        Ok(Self {
            profile_id: revision.profile_id,
            profile_revision: revision.revision,
            provider: revision.provider,
            model: revision.model.clone(),
            canonical_json,
            digest,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Intended for tests and non-persistent diagnostics. Its fields are the complete whitelist.
    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }
}

/// Immutable Provider selection carried by one Run. It contains a generation reference but no
/// secret bytes or vault locator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunProviderBinding {
    run_id: RunId,
    active: ActiveProviderSnapshot,
    fingerprint: ProviderFingerprint,
}

impl RunProviderBinding {
    pub fn from_active(run_id: RunId, active: ActiveProviderSnapshot) -> CoreResult<Self> {
        let revision = ProfileRevision {
            profile_id: active.profile_id,
            revision: active.profile_revision,
            provider: active.provider,
            model: active.model.clone(),
            parameters: active.parameters.clone(),
            credential_generation: Some(active.credential_generation),
            state: ProfileState::Ready,
            validation: Some(CompatibilityEvidence {
                id: active.validation_id,
                digest: active.validation_digest.clone(),
                passed: true,
            }),
        };
        Ok(Self {
            run_id,
            fingerprint: ProviderFingerprint::from_revision(&revision)?,
            active,
        })
    }

    pub fn profile_id(&self) -> ProfileId {
        self.active.profile_id()
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn fingerprint(&self) -> &ProviderFingerprint {
        &self.fingerprint
    }

    pub fn profile_revision(&self) -> u64 {
        self.active.profile_revision()
    }

    pub fn validation_id(&self) -> ValidationId {
        self.active.validation_id()
    }

    pub fn activation_revision(&self) -> u64 {
        self.active.activation_revision()
    }

    pub fn provider(&self) -> ProviderId {
        self.active.provider
    }

    pub fn model(&self) -> &ProviderModelId {
        &self.active.model
    }

    pub fn parameters(&self) -> &ProviderParameters {
        &self.active.parameters
    }

    pub fn credential_generation(&self) -> CredentialGeneration {
        self.active.credential_generation
    }

    pub fn validation_digest(&self) -> &ValidationDigest {
        &self.active.validation_digest
    }
}

impl fmt::Display for ValidationDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The only credential information permitted to cross into a service view, TUI, or Doctor
/// report. It deliberately describes availability rather than a locator or a secret value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialViewStatus {
    Missing,
    Saved,
    Expired,
    Revoked,
    ProtectionUnavailable,
    ReconciliationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthConnectionStatus {
    Pending,
    Connected,
    Expired,
    Revoked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSupportStatus {
    Supported,
    Candidate,
    Blocked,
}

/// Stable, non-sensitive user-facing fields. No variant may carry a credential locator, secret,
/// request payload, or provider response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderField {
    ProfileName,
    Provider,
    Model,
    Credential,
    Parameter(ProviderParameterKey),
    Validation,
    Activation,
    OAuth,
}

/// Stable remediation instructions which callers can render without parsing an adapter error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRemediation {
    ReturnToEdit,
    Retry,
    ConfigureCredentialStore,
    Reauthorize,
    ValidateProfile,
    ActivateAnotherProfile,
    EnterNoActiveProvider,
    WaitForCurrentOperation,
    ContactSupport,
}

/// Stable broad classification for Provider failures. Callers render this alongside the error
/// code instead of interpreting adapter-specific messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCategory {
    Authentication,
    Model,
    Capability,
    RateLimit,
    Timeout,
    Network,
    Server,
    Protocol,
    Credential,
    OAuth,
    Operation,
    Storage,
    Internal,
}

impl ProviderErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Model => "model",
            Self::Capability => "capability",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Server => "server",
            Self::Protocol => "protocol",
            Self::Credential => "credential",
            Self::OAuth => "oauth",
            Self::Operation => "operation",
            Self::Storage => "storage",
            Self::Internal => "internal",
        }
    }
}

/// Retry policy that is safe for a normalized Provider failure. `Bounded` always means the
/// profile's configured retry bound; it never permits model or Provider fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRetryability {
    Never,
    Bounded,
}

impl ProviderRetryability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Bounded => "bounded",
        }
    }
}

/// Closed set of Provider-management failure codes. Adapter implementations map external errors
/// into this type before they leave their boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorCode {
    ProfileNameConflict,
    InvalidModelPrefix,
    AuthenticationInvalid,
    ModelNotFound,
    ModelIncompatible,
    RateLimited,
    Timeout,
    Network,
    Server,
    ProtocolInvalidResponse,
    ProtocolInvalidToolCallId,
    CredentialMissing,
    CredentialProtectionUnavailable,
    OAuthNotConnected,
    ValidationStale,
    ActivationPreconditionFailed,
    NoActiveProfile,
    OperationCancelled,
    OperationStale,
    DiscoveryFailed,
    ProtocolIncompatible,
    RemoteRevokeFailed,
    StorageConflict,
    Internal,
}

impl ProviderErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProfileNameConflict => "provider.profile.name_conflict",
            Self::InvalidModelPrefix => "provider.model.prefix_mismatch",
            Self::AuthenticationInvalid => "provider.auth.invalid",
            Self::ModelNotFound => "provider.model.not_found",
            Self::ModelIncompatible => "provider.model.incompatible",
            Self::RateLimited => "provider.rate_limited",
            Self::Timeout => "provider.timeout",
            Self::Network => "provider.network",
            Self::Server => "provider.server",
            Self::ProtocolInvalidResponse => "provider.protocol.invalid_response",
            Self::ProtocolInvalidToolCallId => "provider.protocol.invalid_tool_call_id",
            Self::CredentialMissing => "provider.credential.missing",
            Self::CredentialProtectionUnavailable => "provider.credential.protection_unavailable",
            Self::OAuthNotConnected => "provider.oauth.not_connected",
            Self::ValidationStale => "provider.validation.stale",
            Self::ActivationPreconditionFailed => "provider.activation.precondition_failed",
            Self::NoActiveProfile => "provider.no_active_profile",
            Self::OperationCancelled => "provider.operation.cancelled",
            Self::OperationStale => "provider.operation.stale",
            Self::DiscoveryFailed => "provider.discovery.failed",
            Self::ProtocolIncompatible => "provider.protocol.incompatible",
            Self::RemoteRevokeFailed => "provider.oauth.remote_revoke_failed",
            Self::StorageConflict => "provider.storage.conflict",
            Self::Internal => "provider.internal",
        }
    }

    pub const fn category(self) -> ProviderErrorCategory {
        match self {
            Self::ProfileNameConflict | Self::InvalidModelPrefix | Self::ModelNotFound => {
                ProviderErrorCategory::Model
            }
            Self::AuthenticationInvalid => ProviderErrorCategory::Authentication,
            Self::ModelIncompatible | Self::ProtocolIncompatible => {
                ProviderErrorCategory::Capability
            }
            Self::RateLimited => ProviderErrorCategory::RateLimit,
            Self::Timeout => ProviderErrorCategory::Timeout,
            Self::Network | Self::DiscoveryFailed => ProviderErrorCategory::Network,
            Self::Server => ProviderErrorCategory::Server,
            Self::ProtocolInvalidResponse | Self::ProtocolInvalidToolCallId => {
                ProviderErrorCategory::Protocol
            }
            Self::CredentialMissing | Self::CredentialProtectionUnavailable => {
                ProviderErrorCategory::Credential
            }
            Self::OAuthNotConnected | Self::RemoteRevokeFailed => ProviderErrorCategory::OAuth,
            Self::ValidationStale
            | Self::ActivationPreconditionFailed
            | Self::NoActiveProfile
            | Self::OperationCancelled
            | Self::OperationStale => ProviderErrorCategory::Operation,
            Self::StorageConflict => ProviderErrorCategory::Storage,
            Self::Internal => ProviderErrorCategory::Internal,
        }
    }

    pub const fn retryability(self) -> ProviderRetryability {
        match self {
            Self::RateLimited | Self::Timeout | Self::Network | Self::Server => {
                ProviderRetryability::Bounded
            }
            _ => ProviderRetryability::Never,
        }
    }
}

impl Serialize for ProviderErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProviderErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let code = match value.as_str() {
            "provider.profile.name_conflict" => Self::ProfileNameConflict,
            "provider.model.prefix_mismatch" => Self::InvalidModelPrefix,
            "provider.auth.invalid" => Self::AuthenticationInvalid,
            "provider.model.not_found" => Self::ModelNotFound,
            "provider.model.incompatible" => Self::ModelIncompatible,
            "provider.rate_limited" => Self::RateLimited,
            "provider.timeout" => Self::Timeout,
            "provider.network" => Self::Network,
            "provider.server" => Self::Server,
            "provider.protocol.invalid_response" => Self::ProtocolInvalidResponse,
            "provider.protocol.invalid_tool_call_id" => Self::ProtocolInvalidToolCallId,
            "provider.credential.missing" => Self::CredentialMissing,
            "provider.credential.protection_unavailable" => Self::CredentialProtectionUnavailable,
            "provider.oauth.not_connected" => Self::OAuthNotConnected,
            "provider.validation.stale" => Self::ValidationStale,
            "provider.activation.precondition_failed" => Self::ActivationPreconditionFailed,
            "provider.no_active_profile" => Self::NoActiveProfile,
            "provider.operation.cancelled" => Self::OperationCancelled,
            "provider.operation.stale" => Self::OperationStale,
            "provider.discovery.failed" => Self::DiscoveryFailed,
            "provider.protocol.incompatible" => Self::ProtocolIncompatible,
            "provider.oauth.remote_revoke_failed" => Self::RemoteRevokeFailed,
            "provider.storage.conflict" => Self::StorageConflict,
            "provider.internal" => Self::Internal,
            _ => return Err(serde::de::Error::custom("unknown Provider error code")),
        };
        Ok(code)
    }
}

/// The portable error surface of every Provider port. It excludes arbitrary strings so raw
/// keyring, OAuth, and HTTP errors cannot leak through the core boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderManagementError {
    code: ProviderErrorCode,
    field: Option<ProviderField>,
    remediation: ProviderRemediation,
}

impl ProviderManagementError {
    pub const fn new(
        code: ProviderErrorCode,
        field: Option<ProviderField>,
        remediation: ProviderRemediation,
    ) -> Self {
        Self {
            code,
            field,
            remediation,
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code.as_str()
    }

    pub const fn category(&self) -> ProviderErrorCategory {
        self.code.category()
    }

    pub const fn retryability(&self) -> ProviderRetryability {
        self.code.retryability()
    }

    pub fn field(&self) -> Option<&ProviderField> {
        self.field.as_ref()
    }

    pub const fn remediation(&self) -> ProviderRemediation {
        self.remediation
    }
}

impl fmt::Display for ProviderManagementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ProviderManagementError {}

pub type ProviderResult<T> = Result<T, ProviderManagementError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub profile_id: ProfileId,
    pub name: String,
    pub provider: ProviderId,
    pub state: ProfileState,
    pub credential_status: CredentialViewStatus,
    pub is_active: bool,
}

/// Safe Profile data for TUI and Doctor use. The credential is represented only by its state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileDetail {
    pub summary: ProfileSummary,
    pub revision: u64,
    pub credential_generation: Option<CredentialGeneration>,
    pub model: ProviderModelId,
    pub parameters: ProviderParameters,
    pub validation_id: Option<ValidationId>,
    pub oauth_status: Option<OAuthConnectionStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCatalogView {
    pub provider: ProviderId,
    pub display_name: String,
    pub credential_kind: CredentialKind,
    pub support_status: ProviderSupportStatus,
    pub evidence_gaps: Vec<String>,
}

/// Stable identifier for a governed commercial or subscription plan. It is display-safe and
/// carries no entitlement, credential, URL, or provider request data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProviderPlanId(String);

impl ProviderPlanId {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        if value.trim().is_empty()
            || value != value.trim()
            || value.chars().any(char::is_whitespace)
        {
            return Err(CoreError::validation(
                "invalid_provider_plan_id",
                "provider plan id must be a non-empty token without whitespace",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Catalog-owned classification used by the two top-level model-selection tabs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SelectionTarget {
    Provider(ProviderId),
    Plan {
        provider: ProviderId,
        plan: ProviderPlanId,
    },
}

impl SelectionTarget {
    pub const fn provider(&self) -> ProviderId {
        match self {
            Self::Provider(provider) | Self::Plan { provider, .. } => *provider,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionAvailability {
    Configured,
    NeedsSetup,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCurrentStatus {
    Current,
    NotCurrent,
}

impl SelectionCurrentStatus {
    pub const fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }
}

/// One safe Provider or Plan row returned by the governed selection Catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionTargetView {
    target: SelectionTarget,
    display_name: String,
    availability: SelectionAvailability,
    current: SelectionCurrentStatus,
}

impl SelectionTargetView {
    pub fn new(
        target: SelectionTarget,
        display_name: impl Into<String>,
        availability: SelectionAvailability,
        current: SelectionCurrentStatus,
    ) -> CoreResult<Self> {
        let display_name = display_name.into();
        validate_selection_label(&display_name, "selection target display name")?;
        Ok(Self {
            target,
            display_name,
            availability,
            current,
        })
    }

    pub fn target(&self) -> &SelectionTarget {
        &self.target
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn availability(&self) -> SelectionAvailability {
        self.availability
    }

    pub const fn current(&self) -> SelectionCurrentStatus {
        self.current
    }
}

/// A complete top-level snapshot. Construction enforces the single global Current marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSelectionSnapshot {
    targets: Vec<SelectionTargetView>,
}

impl ModelSelectionSnapshot {
    pub fn new(targets: Vec<SelectionTargetView>) -> CoreResult<Self> {
        ensure_unique_current(
            targets
                .iter()
                .filter(|target| target.current().is_current())
                .count(),
            "model selection snapshot",
        )?;
        Ok(Self { targets })
    }

    pub fn targets(&self) -> &[SelectionTargetView] {
        &self.targets
    }
}

/// Stable candidate identity and optimistic-concurrency preconditions. All fields are private so
/// callers cannot construct a partial key or pair a model with the wrong Provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ModelCandidateKey {
    profile_id: ProfileId,
    expected_profile_revision: u64,
    expected_activation_revision: Option<u64>,
    provider: ProviderId,
    model: ProviderModelId,
}

impl ModelCandidateKey {
    pub fn new(
        profile_id: ProfileId,
        expected_profile_revision: u64,
        expected_activation_revision: Option<u64>,
        provider: ProviderId,
        model: ProviderModelId,
    ) -> CoreResult<Self> {
        if expected_profile_revision == 0 {
            return Err(CoreError::validation(
                "invalid_model_candidate_profile_revision",
                "model candidate profile revision starts at 1",
            ));
        }
        if expected_activation_revision == Some(0) {
            return Err(CoreError::validation(
                "invalid_model_candidate_activation_revision",
                "model candidate activation revision starts at 1",
            ));
        }
        if model.provider() != provider {
            return Err(CoreError::validation(
                "model_candidate_provider_mismatch",
                "model candidate Provider must own the model identifier",
            ));
        }
        Ok(Self {
            profile_id,
            expected_profile_revision,
            expected_activation_revision,
            provider,
            model,
        })
    }

    pub const fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    pub const fn expected_profile_revision(&self) -> u64 {
        self.expected_profile_revision
    }

    pub const fn expected_activation_revision(&self) -> Option<u64> {
        self.expected_activation_revision
    }

    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn model(&self) -> &ProviderModelId {
        &self.model
    }

    pub fn ensure_fresh(
        &self,
        current_profile_revision: u64,
        current_activation_revision: Option<u64>,
    ) -> ProviderResult<()> {
        if self.expected_profile_revision != current_profile_revision
            || self.expected_activation_revision != current_activation_revision
        {
            return Err(ProviderManagementError::new(
                ProviderErrorCode::ActivationPreconditionFailed,
                Some(ProviderField::Activation),
                ProviderRemediation::Retry,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCandidateStatus {
    Ready,
    NeedsValidation,
    ValidationExpired,
    CapabilityInsufficient,
    Unavailable,
}

/// Display-safe model candidate. It deliberately excludes Provider parameters, validation
/// digests, credentials, endpoints, request contents, and transport identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCandidateView {
    key: ModelCandidateKey,
    profile_display_name: String,
    model_display_name: String,
    status: ModelCandidateStatus,
    current: SelectionCurrentStatus,
}

impl ModelCandidateView {
    pub fn new(
        key: ModelCandidateKey,
        profile_display_name: impl Into<String>,
        model_display_name: impl Into<String>,
        status: ModelCandidateStatus,
        current: SelectionCurrentStatus,
    ) -> CoreResult<Self> {
        let profile_display_name = profile_display_name.into();
        let model_display_name = model_display_name.into();
        validate_selection_label(&profile_display_name, "profile display name")?;
        validate_selection_label(&model_display_name, "model display name")?;
        Ok(Self {
            key,
            profile_display_name,
            model_display_name,
            status,
            current,
        })
    }

    pub fn key(&self) -> &ModelCandidateKey {
        &self.key
    }

    pub fn profile_display_name(&self) -> &str {
        &self.profile_display_name
    }

    pub fn model_display_name(&self) -> &str {
        &self.model_display_name
    }

    pub const fn status(&self) -> ModelCandidateStatus {
        self.status
    }

    pub const fn current(&self) -> SelectionCurrentStatus {
        self.current
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCandidateBatch {
    target: SelectionTarget,
    candidates: Vec<ModelCandidateView>,
}

impl ModelCandidateBatch {
    pub fn new(target: SelectionTarget, candidates: Vec<ModelCandidateView>) -> CoreResult<Self> {
        ensure_unique_current(
            candidates
                .iter()
                .filter(|candidate| candidate.current().is_current())
                .count(),
            "model candidate batch",
        )?;
        if candidates
            .iter()
            .any(|candidate| candidate.key().provider() != target.provider())
        {
            return Err(CoreError::validation(
                "model_candidate_target_mismatch",
                "every model candidate must belong to the selected target Provider",
            ));
        }
        Ok(Self { target, candidates })
    }

    pub fn target(&self) -> &SelectionTarget {
        &self.target
    }

    pub fn candidates(&self) -> &[ModelCandidateView] {
        &self.candidates
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListModelCandidatesRequest {
    pub target: SelectionTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchModelRequest {
    operation_id: OperationId,
    candidate: ModelCandidateKey,
}

impl SwitchModelRequest {
    pub const fn new(operation_id: OperationId, candidate: ModelCandidateKey) -> Self {
        Self {
            operation_id,
            candidate,
        }
    }

    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    pub const fn candidate(&self) -> &ModelCandidateKey {
        &self.candidate
    }
}

fn validate_selection_label(value: &str, label: &str) -> CoreResult<()> {
    if value.trim().is_empty() || value != value.trim() || value.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "invalid_model_selection_label",
            format!("{label} must be non-empty, trimmed, and contain no control characters"),
        ));
    }
    Ok(())
}

fn ensure_unique_current(count: usize, scope: &str) -> CoreResult<()> {
    if count > 1 {
        return Err(CoreError::validation(
            "multiple_current_model_selections",
            format!("{scope} may contain at most one Current marker"),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub model: String,
    pub context_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityEvidenceView {
    pub validation_id: ValidationId,
    pub state: ProfileState,
    pub credential_status: CredentialViewStatus,
    pub error: Option<ProviderManagementError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiveProviderView {
    pub activation_revision: u64,
    pub profile_id: ProfileId,
    pub profile_revision: u64,
    pub provider: ProviderId,
    pub model: ProviderModelId,
    pub parameters: ProviderParameters,
}

/// Non-sensitive confirmation data rendered before changing the active singleton. It makes the
/// Run boundary explicit: existing Run bindings remain immutable and only later Runs can observe
/// the selected revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationConfirmation {
    pub profile_id: ProfileId,
    pub profile_revision: u64,
    pub affects_new_runs_only: bool,
}

impl From<&ActiveProviderSnapshot> for ActiveProviderView {
    fn from(snapshot: &ActiveProviderSnapshot) -> Self {
        Self {
            activation_revision: snapshot.activation_revision,
            profile_id: snapshot.profile_id,
            profile_revision: snapshot.profile_revision,
            provider: snapshot.provider,
            model: snapshot.model.clone(),
            parameters: snapshot.parameters.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderDoctorView {
    pub active: Option<ActiveProviderView>,
    pub credential_status: Option<CredentialViewStatus>,
    pub blockers: Vec<ProviderManagementError>,
    pub warnings: Vec<ProviderManagementError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionPrecondition {
    pub profile_id: ProfileId,
    pub expected_current_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveProfileRevision {
    pub precondition: RevisionPrecondition,
    pub name: ProfileName,
    pub revision: ProfileRevision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCommitPrecondition {
    pub operation_id: OperationId,
    pub profile_id: ProfileId,
    pub revision: u64,
    pub credential_generation: CredentialGeneration,
    pub validation_digest: ValidationDigest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationCommit {
    pub precondition: ValidationCommitPrecondition,
    pub evidence: CompatibilityEvidence,
    /// Versioned, non-sensitive inputs used to bind the evidence digest to the exact current
    /// Profile revision before persistence. They are verified but need not be retained after the
    /// resulting digest has been committed.
    pub versions: ValidationVersions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationPrecondition {
    pub profile_id: ProfileId,
    pub revision: u64,
    pub validation_id: ValidationId,
    pub validation_digest: ValidationDigest,
    pub expected_activation_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMutationIntent {
    operation_id: OperationId,
    profile_id: ProfileId,
    expected_revision: u64,
    expected_generation: Option<CredentialGeneration>,
    operation: CredentialMutationOperation,
    old_generation: Option<CredentialGeneration>,
    new_generation: Option<CredentialGeneration>,
    rollback_generation: Option<CredentialGeneration>,
}

impl CredentialMutationIntent {
    pub fn create(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        new_generation: CredentialGeneration,
    ) -> CoreResult<Self> {
        Self::new(
            operation_id,
            profile_id,
            expected_revision,
            CredentialMutationOperation::Create,
            None,
            Some(new_generation),
            None,
        )
    }

    pub fn replace(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        old_generation: CredentialGeneration,
        new_generation: CredentialGeneration,
    ) -> CoreResult<Self> {
        Self::new(
            operation_id,
            profile_id,
            expected_revision,
            CredentialMutationOperation::Replace,
            Some(old_generation),
            Some(new_generation),
            None,
        )
    }

    pub fn refresh(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        old_generation: CredentialGeneration,
        new_generation: CredentialGeneration,
    ) -> CoreResult<Self> {
        Self::new(
            operation_id,
            profile_id,
            expected_revision,
            CredentialMutationOperation::Refresh,
            Some(old_generation),
            Some(new_generation),
            None,
        )
    }

    pub fn delete(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        old_generation: CredentialGeneration,
        rollback_generation: CredentialGeneration,
    ) -> CoreResult<Self> {
        Self::new(
            operation_id,
            profile_id,
            expected_revision,
            CredentialMutationOperation::Delete,
            Some(old_generation),
            None,
            Some(rollback_generation),
        )
    }

    pub fn revoke(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        old_generation: CredentialGeneration,
        rollback_generation: CredentialGeneration,
    ) -> CoreResult<Self> {
        Self::new(
            operation_id,
            profile_id,
            expected_revision,
            CredentialMutationOperation::Revoke,
            Some(old_generation),
            None,
            Some(rollback_generation),
        )
    }

    fn new(
        operation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        operation: CredentialMutationOperation,
        old_generation: Option<CredentialGeneration>,
        new_generation: Option<CredentialGeneration>,
        rollback_generation: Option<CredentialGeneration>,
    ) -> CoreResult<Self> {
        if expected_revision == 0 {
            return Err(CoreError::validation(
                "invalid_credential_mutation_revision",
                "credential mutations require a persisted Profile revision",
            ));
        }
        for generation in [old_generation, new_generation, rollback_generation]
            .into_iter()
            .flatten()
        {
            if generation.profile_id() != profile_id {
                return Err(CoreError::validation(
                    "credential_profile_mismatch",
                    "credential journal generations belong to the mutation Profile",
                ));
            }
        }
        let expected_kind = old_generation
            .or(new_generation)
            .or(rollback_generation)
            .map(CredentialGeneration::kind);
        if [old_generation, new_generation, rollback_generation]
            .into_iter()
            .flatten()
            .any(|generation| Some(generation.kind()) != expected_kind)
        {
            return Err(CoreError::validation(
                "credential_kind_mismatch",
                "credential journal generations use one authentication kind",
            ));
        }
        let shape_is_valid = match operation {
            CredentialMutationOperation::Create => {
                old_generation.is_none()
                    && new_generation.is_some()
                    && rollback_generation.is_none()
            }
            CredentialMutationOperation::Replace | CredentialMutationOperation::Refresh => {
                matches!(
                    (old_generation, new_generation),
                    (Some(old), Some(new)) if new.number() > old.number()
                ) && rollback_generation.is_none()
            }
            CredentialMutationOperation::Delete | CredentialMutationOperation::Revoke => {
                matches!(
                    (old_generation, rollback_generation),
                    (Some(old), Some(rollback)) if rollback.number() > old.number()
                ) && new_generation.is_none()
            }
        };
        if !shape_is_valid {
            return Err(CoreError::validation(
                "invalid_credential_mutation_shape",
                "credential journal operation and generation references must agree",
            ));
        }
        Ok(Self {
            operation_id,
            profile_id,
            expected_revision,
            expected_generation: old_generation,
            operation,
            old_generation,
            new_generation,
            rollback_generation,
        })
    }

    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    pub fn expected_generation(&self) -> Option<CredentialGeneration> {
        self.expected_generation
    }

    pub fn operation(&self) -> CredentialMutationOperation {
        self.operation
    }

    pub fn old_generation(&self) -> Option<CredentialGeneration> {
        self.old_generation
    }

    pub fn new_generation(&self) -> Option<CredentialGeneration> {
        self.new_generation
    }

    pub fn rollback_generation(&self) -> Option<CredentialGeneration> {
        self.rollback_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMutationOperation {
    Create,
    Replace,
    Refresh,
    Delete,
    Revoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMutationPhase {
    IntentRecorded,
    VaultWritten,
    PointerCommitted,
    CleanupPending,
    RolledBack,
    Completed,
    Blocked,
}

impl CredentialMutationPhase {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::RolledBack | Self::Completed | Self::Blocked)
    }
}

/// Durable non-sensitive input for credential recovery. It names only immutable generation
/// references and stable errors; a Vault locator or secret can never enter this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedCredentialMutationRecord {
    pub operation_id: OperationId,
    pub profile_id: ProfileId,
    pub expected_revision: u64,
    pub operation: CredentialMutationOperation,
    pub old_generation: Option<CredentialGeneration>,
    pub new_generation: Option<CredentialGeneration>,
    pub rollback_generation: Option<CredentialGeneration>,
    pub phase: CredentialMutationPhase,
    pub error_code: Option<ProviderErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMutationRecord {
    intent: CredentialMutationIntent,
    phase: CredentialMutationPhase,
    error_code: Option<ProviderErrorCode>,
}

impl CredentialMutationRecord {
    pub fn intent_recorded(intent: CredentialMutationIntent) -> Self {
        Self {
            intent,
            phase: CredentialMutationPhase::IntentRecorded,
            error_code: None,
        }
    }

    pub fn hydrate(persisted: PersistedCredentialMutationRecord) -> CoreResult<Self> {
        validate_credential_mutation_error(persisted.phase, persisted.error_code)?;
        let intent = CredentialMutationIntent::new(
            persisted.operation_id,
            persisted.profile_id,
            persisted.expected_revision,
            persisted.operation,
            persisted.old_generation,
            persisted.new_generation,
            persisted.rollback_generation,
        )?;
        Ok(Self {
            intent,
            phase: persisted.phase,
            error_code: persisted.error_code,
        })
    }

    pub fn persisted(&self) -> PersistedCredentialMutationRecord {
        PersistedCredentialMutationRecord {
            operation_id: self.operation_id(),
            profile_id: self.profile_id(),
            expected_revision: self.expected_revision(),
            operation: self.operation(),
            old_generation: self.old_generation(),
            new_generation: self.new_generation(),
            rollback_generation: self.rollback_generation(),
            phase: self.phase(),
            error_code: self.error_code(),
        }
    }

    pub fn transition(
        mut self,
        phase: CredentialMutationPhase,
        error_code: Option<ProviderErrorCode>,
    ) -> CoreResult<Self> {
        validate_credential_mutation_error(phase, error_code)?;
        if !credential_mutation_transition_is_valid(self.phase, phase) {
            return Err(CoreError::validation(
                "invalid_credential_mutation_transition",
                "credential journal phases must advance through the recovery state machine",
            ));
        }
        self.phase = phase;
        self.error_code = error_code;
        Ok(self)
    }

    pub fn operation_id(&self) -> OperationId {
        self.intent.operation_id()
    }

    pub fn profile_id(&self) -> ProfileId {
        self.intent.profile_id()
    }

    pub fn expected_revision(&self) -> u64 {
        self.intent.expected_revision()
    }

    pub fn expected_generation(&self) -> Option<CredentialGeneration> {
        self.intent.expected_generation()
    }

    pub fn operation(&self) -> CredentialMutationOperation {
        self.intent.operation()
    }

    pub fn old_generation(&self) -> Option<CredentialGeneration> {
        self.intent.old_generation()
    }

    pub fn new_generation(&self) -> Option<CredentialGeneration> {
        self.intent.new_generation()
    }

    pub fn rollback_generation(&self) -> Option<CredentialGeneration> {
        self.intent.rollback_generation()
    }

    pub fn phase(&self) -> CredentialMutationPhase {
        self.phase
    }

    pub fn error_code(&self) -> Option<ProviderErrorCode> {
        self.error_code
    }

    pub fn requires_reconciliation(&self) -> bool {
        !self.phase.is_terminal()
    }

    pub fn blocks_profile_use(&self) -> bool {
        self.phase == CredentialMutationPhase::Blocked
    }
}

fn validate_credential_mutation_error(
    phase: CredentialMutationPhase,
    error_code: Option<ProviderErrorCode>,
) -> CoreResult<()> {
    if phase == CredentialMutationPhase::Blocked && error_code.is_none() {
        return Err(CoreError::validation(
            "credential_mutation_block_reason_required",
            "a blocked credential mutation requires a stable error code",
        ));
    }
    if phase != CredentialMutationPhase::Blocked && error_code.is_some() {
        return Err(CoreError::validation(
            "credential_mutation_unexpected_error_code",
            "only blocked credential mutations persist an error code",
        ));
    }
    Ok(())
}

fn credential_mutation_transition_is_valid(
    current: CredentialMutationPhase,
    next: CredentialMutationPhase,
) -> bool {
    matches!(
        (current, next),
        (
            CredentialMutationPhase::IntentRecorded,
            CredentialMutationPhase::VaultWritten
                | CredentialMutationPhase::RolledBack
                | CredentialMutationPhase::Blocked
        ) | (
            CredentialMutationPhase::VaultWritten,
            CredentialMutationPhase::PointerCommitted
                | CredentialMutationPhase::CleanupPending
                | CredentialMutationPhase::RolledBack
                | CredentialMutationPhase::Blocked
        ) | (
            CredentialMutationPhase::PointerCommitted,
            CredentialMutationPhase::CleanupPending
                | CredentialMutationPhase::Completed
                | CredentialMutationPhase::Blocked
        ) | (
            CredentialMutationPhase::CleanupPending,
            CredentialMutationPhase::RolledBack
                | CredentialMutationPhase::Completed
                | CredentialMutationPhase::Blocked
        )
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct CredentialPointerCommit {
    mutation_id: OperationId,
    profile_id: ProfileId,
    expected_revision: u64,
    replacement_revision: ProfileRevision,
}

impl CredentialPointerCommit {
    pub fn new(
        mutation_id: OperationId,
        profile_id: ProfileId,
        expected_revision: u64,
        replacement_revision: ProfileRevision,
    ) -> CoreResult<Self> {
        let next_revision = expected_revision.checked_add(1);
        if expected_revision == 0
            || replacement_revision.profile_id() != profile_id
            || Some(replacement_revision.revision()) != next_revision
            || replacement_revision.state() != ProfileState::Draft
            || replacement_revision.validation().is_some()
        {
            return Err(CoreError::validation(
                "invalid_credential_pointer_commit",
                "credential pointer commits append the next unvalidated revision for the same Profile",
            ));
        }
        Ok(Self {
            mutation_id,
            profile_id,
            expected_revision,
            replacement_revision,
        })
    }

    pub fn mutation_id(&self) -> OperationId {
        self.mutation_id
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    pub fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    pub fn replacement_revision(&self) -> &ProfileRevision {
        &self.replacement_revision
    }

    pub fn new_generation(&self) -> Option<CredentialGeneration> {
        self.replacement_revision.credential_generation()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialReference {
    pub profile_id: ProfileId,
    pub generation: CredentialGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProtectionStatus {
    ConfirmedNative,
    Unavailable,
    Unconfirmed,
}

/// An owned write request whose secret cannot be rendered, cloned, or serialized.
pub struct ProtectedCredentialWrite {
    pub reference: ProviderCredentialReference,
    pub secret: SecretValue,
}

/// A short-lived secret lease. Only an adapter can inspect it through the closure; it never
/// supplies a string to view types, repositories, or TUI state.
pub struct CredentialLease(SecretValue);

impl CredentialLease {
    pub fn new(secret: SecretValue) -> Self {
        Self(secret)
    }

    pub fn with_secret<T>(&self, use_secret: impl FnOnce(&SecretValue) -> T) -> T {
        use_secret(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderClientBinding {
    pub profile_id: ProfileId,
    pub profile_revision: u64,
    pub provider: ProviderId,
    pub model: ProviderModelId,
    pub parameters: ProviderParameters,
    pub credential_generation: CredentialGeneration,
}

impl ProviderClientBinding {
    pub fn from_revision(revision: &ProfileRevision) -> CoreResult<Self> {
        let credential_generation = revision.credential_generation().ok_or_else(|| {
            CoreError::validation(
                "credential_missing",
                "a Provider client requires a credential generation",
            )
        })?;
        Ok(Self {
            profile_id: revision.profile_id(),
            profile_revision: revision.revision(),
            provider: revision.provider(),
            model: revision.model().clone(),
            parameters: revision.parameters().clone(),
            credential_generation,
        })
    }

    pub fn from_run_binding(binding: &RunProviderBinding) -> Self {
        Self {
            profile_id: binding.active.profile_id,
            profile_revision: binding.active.profile_revision,
            provider: binding.active.provider,
            model: binding.active.model.clone(),
            parameters: binding.active.parameters.clone(),
            credential_generation: binding.active.credential_generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverModelsRequest {
    pub operation_id: OperationId,
    pub profile_id: ProfileId,
    pub profile_revision: u64,
    pub provider: ProviderId,
    pub credential_generation: CredentialGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateProfileRequest {
    pub operation_id: OperationId,
    pub profile_id: ProfileId,
    pub revision: u64,
    /// A model-level limit learned from discovery, a protocol probe, or approved evidence. A
    /// catalog-wide default is intentionally not sufficient to activate a Profile.
    pub observed_context_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateProfileRequest {
    pub operation_id: OperationId,
    pub precondition: ActivationPrecondition,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveProfileRequest {
    pub operation_id: OperationId,
    pub revision: SaveProfileRevision,
}

/// The only two Credential mutations supported by the service boundary. A deletion carries no
/// secret value, which makes it safe to issue from a masked TUI command.
pub enum CredentialMutation {
    Replace(ProtectedCredentialWrite),
    Delete,
}

pub struct CredentialMutationRequest {
    pub intent: CredentialMutationIntent,
    pub mutation: CredentialMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRevisionPrecondition {
    pub profile_id: ProfileId,
    pub revision: u64,
    pub activation_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteProfileRequest {
    pub operation_id: OperationId,
    pub profile_id: ProfileId,
    pub expected_revision: u64,
    pub expected_active: Option<ActiveRevisionPrecondition>,
    pub enter_no_active_provider: bool,
}

#[derive(Clone)]
pub struct ResolvedRunProvider {
    pub binding: RunProviderBinding,
    pub provider: Arc<dyn ModelProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAuthorizationView {
    pub verification_uri: String,
    pub user_code: String,
    pub expires_in_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthConnectionView {
    pub profile_id: ProfileId,
    pub status: OAuthConnectionStatus,
    pub remediation: Option<ProviderRemediation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRevocationOutcome {
    Revoked,
    ResidualRisk { remediation: ProviderRemediation },
}
