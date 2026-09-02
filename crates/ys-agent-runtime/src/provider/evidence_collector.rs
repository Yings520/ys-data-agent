//! Sanitized, explicitly approved live-compatibility evidence collection.
//!
//! The collector has no environment-variable, configuration-file, Vault, or HTTP dependency.
//! An approved caller supplies a narrow probe that can report only capability booleans. That
//! prevents tokens, raw prompts, raw responses, account identity, and customer data from ever
//! becoming evidence-schema values.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ys_agent_core::{CredentialKind, ProviderId, ProviderModelId};

use super::evidence::EvidenceKind;

pub const LIVE_EVIDENCE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceEnvironment {
    ApprovedProduction,
    ApprovedEquivalent,
}

/// Non-secret version inputs that bind a collected result to the exact catalog/codec baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCollectionBaseline {
    pub catalog_digest: String,
    pub probe_schema_version: String,
    pub codec_digest: String,
    pub liter_llm_version: String,
}

impl EvidenceCollectionBaseline {
    pub fn is_complete(&self) -> bool {
        is_sha256_hex(&self.catalog_digest)
            && !self.probe_schema_version.trim().is_empty()
            && is_sha256_hex(&self.codec_digest)
            && !self.liter_llm_version.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCollectionTarget {
    pub provider: ProviderId,
    pub representative_model: ProviderModelId,
}

impl EvidenceCollectionTarget {
    pub fn new(provider: ProviderId, representative_model: ProviderModelId) -> Self {
        Self {
            provider,
            representative_model,
        }
    }
}

/// The complete but non-sensitive outcome of one real or approved-equivalent probe. The adapter
/// performing a live call must map its raw transport data into this type before returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceProbeObservation {
    pub authentication: bool,
    pub tool_call: bool,
    pub tool_call_id_round_trip: bool,
    pub multi_turn_tool_result: bool,
    pub context: bool,
    pub parameters: bool,
    pub error_behavior: bool,
    pub chatgpt_oauth: bool,
    pub fixed_responses_backend: bool,
}

#[async_trait]
pub trait LiveEvidenceProbe: Send + Sync {
    /// Executes the caller-authorized live check and returns only sanitized capability outcomes.
    async fn probe(
        &self,
        target: &EvidenceCollectionTarget,
    ) -> EvidenceCollectionResult<EvidenceProbeObservation>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanitizedProviderEvidenceDocument {
    pub schema_version: u16,
    pub environment: EvidenceEnvironment,
    pub provider: ProviderId,
    pub representative_model: ProviderModelId,
    pub credential_kind: CredentialKind,
    pub collected_at: DateTime<Utc>,
    pub baseline: EvidenceCollectionBaseline,
    pub observed_evidence: BTreeSet<EvidenceKind>,
    pub tool_call_id_round_trip: bool,
    pub multi_turn_tool_result: bool,
    pub context: bool,
    pub chatgpt_oauth: bool,
    pub fixed_responses_backend: bool,
}

impl SanitizedProviderEvidenceDocument {
    pub fn digest_sha256(&self) -> String {
        let canonical = serde_json::to_vec(self)
            .expect("sanitized evidence document serialization is infallible");
        hex::encode(Sha256::digest(canonical))
    }

    /// Produces the four immutable category hashes consumed by the support registry. The hashes
    /// bind each required category to this already sanitized document; neither raw probes nor a
    /// credential can become part of the release manifest.
    pub fn evidence_hashes(&self) -> BTreeMap<EvidenceKind, String> {
        EvidenceKind::ALL
            .into_iter()
            .map(|kind| {
                let canonical = serde_json::to_vec(&(self, kind))
                    .expect("sanitized evidence category serialization is infallible");
                (kind, hex::encode(Sha256::digest(canonical)))
            })
            .collect()
    }

    pub fn to_json(&self) -> EvidenceCollectionResult<String> {
        serde_json::to_string(self).map_err(|_| EvidenceCollectionError::InvalidDocument)
    }

    pub fn from_json(
        source: &str,
        expected_baseline: &EvidenceCollectionBaseline,
        expected_target: &EvidenceCollectionTarget,
        maximum_age: Duration,
        now: DateTime<Utc>,
    ) -> EvidenceCollectionResult<Self> {
        let document: Self =
            serde_json::from_str(source).map_err(|_| EvidenceCollectionError::InvalidDocument)?;
        document.validate(expected_baseline, expected_target, maximum_age, now)?;
        Ok(document)
    }

    pub fn validate(
        &self,
        expected_baseline: &EvidenceCollectionBaseline,
        expected_target: &EvidenceCollectionTarget,
        maximum_age: Duration,
        now: DateTime<Utc>,
    ) -> EvidenceCollectionResult<()> {
        if self.schema_version != LIVE_EVIDENCE_SCHEMA_VERSION || !self.baseline.is_complete() {
            return Err(EvidenceCollectionError::InvalidDocument);
        }
        if &self.baseline != expected_baseline {
            return Err(EvidenceCollectionError::BaselineMismatch);
        }
        if self.provider != expected_target.provider
            || self.representative_model != expected_target.representative_model
            || self.representative_model.provider() != self.provider
            || self.credential_kind != self.provider.required_credential_kind()
        {
            return Err(EvidenceCollectionError::TargetMismatch);
        }
        if self.collected_at > now
            || now
                .signed_duration_since(self.collected_at)
                .to_std()
                .map_or(true, |age| age > maximum_age)
        {
            return Err(EvidenceCollectionError::Expired);
        }
        if self.observed_evidence != EvidenceKind::ALL.into_iter().collect()
            || !self.tool_call_id_round_trip
            || !self.multi_turn_tool_result
            || !self.context
        {
            return Err(EvidenceCollectionError::Incomplete);
        }
        let chatgpt = self.provider == ProviderId::ChatGptSubscription;
        if (chatgpt && (!self.chatgpt_oauth || !self.fixed_responses_backend))
            || (!chatgpt && (self.chatgpt_oauth || self.fixed_responses_backend))
        {
            return Err(EvidenceCollectionError::ProtocolMismatch);
        }
        Ok(())
    }
}

/// Explicit approval is a construction-time capability. Ordinary CI can still test this type with
/// a fixture probe, but the collector itself never discovers or loads personal credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedEvidenceCollection {
    environment: EvidenceEnvironment,
}

impl ApprovedEvidenceCollection {
    pub const fn production() -> Self {
        Self {
            environment: EvidenceEnvironment::ApprovedProduction,
        }
    }

    pub const fn equivalent() -> Self {
        Self {
            environment: EvidenceEnvironment::ApprovedEquivalent,
        }
    }
}

pub struct LiveEvidenceCollector {
    approval: ApprovedEvidenceCollection,
    baseline: EvidenceCollectionBaseline,
}

impl LiveEvidenceCollector {
    pub fn new(
        approval: ApprovedEvidenceCollection,
        baseline: EvidenceCollectionBaseline,
    ) -> EvidenceCollectionResult<Self> {
        if !baseline.is_complete() {
            return Err(EvidenceCollectionError::InvalidDocument);
        }
        Ok(Self { approval, baseline })
    }

    pub async fn collect(
        &self,
        target: EvidenceCollectionTarget,
        probe: &dyn LiveEvidenceProbe,
        now: DateTime<Utc>,
    ) -> EvidenceCollectionResult<SanitizedProviderEvidenceDocument> {
        if target.representative_model.provider() != target.provider {
            return Err(EvidenceCollectionError::TargetMismatch);
        }
        let observation = probe.probe(&target).await?;
        let observed_evidence = [
            (EvidenceKind::Authentication, observation.authentication),
            (
                EvidenceKind::Protocol,
                observation.tool_call
                    && observation.tool_call_id_round_trip
                    && observation.multi_turn_tool_result,
            ),
            (EvidenceKind::Parameters, observation.parameters),
            (EvidenceKind::ErrorBehavior, observation.error_behavior),
        ]
        .into_iter()
        .filter_map(|(kind, passed)| passed.then_some(kind))
        .collect();
        let document = SanitizedProviderEvidenceDocument {
            schema_version: LIVE_EVIDENCE_SCHEMA_VERSION,
            environment: self.approval.environment,
            provider: target.provider,
            representative_model: target.representative_model.clone(),
            credential_kind: target.provider.required_credential_kind(),
            collected_at: now,
            baseline: self.baseline.clone(),
            observed_evidence,
            tool_call_id_round_trip: observation.tool_call_id_round_trip,
            multi_turn_tool_result: observation.multi_turn_tool_result,
            context: observation.context,
            chatgpt_oauth: observation.chatgpt_oauth,
            fixed_responses_backend: observation.fixed_responses_backend,
        };
        document.validate(&self.baseline, &target, Duration::ZERO, now)?;
        Ok(document)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceCollectionError {
    InvalidDocument,
    BaselineMismatch,
    TargetMismatch,
    Incomplete,
    ProtocolMismatch,
    Expired,
}

impl EvidenceCollectionError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidDocument => "provider.evidence.invalid_document",
            Self::BaselineMismatch => "provider.evidence.baseline_mismatch",
            Self::TargetMismatch => "provider.evidence.target_mismatch",
            Self::Incomplete => "provider.evidence.incomplete",
            Self::ProtocolMismatch => "provider.evidence.protocol_mismatch",
            Self::Expired => "provider.evidence.expired",
        }
    }
}

pub type EvidenceCollectionResult<T> = Result<T, EvidenceCollectionError>;

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
