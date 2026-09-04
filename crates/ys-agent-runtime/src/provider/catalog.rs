use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sha2::{Digest, Sha256};
use ys_agent_core::{
    CredentialKind, ParameterApplicability, ProviderId, ProviderParameterKey, ProviderPlanId,
    SelectionTarget,
};

use super::evidence::EvidenceKind;

/// A closed adapter-owned endpoint selector. Profiles cannot carry URLs or construct new keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEndpointKey {
    OpenAi,
    Kimi,
    Qwen,
    Gemini,
    Glm,
    ChatGptBackend,
    ClaudeSubscription,
    OpenCodeGo,
    AlibabaCoding,
    BigModelCoding,
    ZaiCoding,
    MiniMaxCoding,
    KimiCoding,
    OpenCodeZen,
    DeepSeek,
    Xai,
    Zai,
    OpenRouter,
    MiniMax,
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    Chat,
    Responses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelDiscoveryKind {
    FixedBackend,
    ProviderCatalog,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCatalogEntry {
    id: ProviderId,
    display_name: &'static str,
    model_prefix: &'static str,
    credential_kind: CredentialKind,
    endpoint_key: ProviderEndpointKey,
    protocol: ProviderProtocol,
    discovery: ModelDiscoveryKind,
    parameter_rules: BTreeMap<ProviderParameterKey, ParameterApplicability>,
    required_evidence: BTreeSet<EvidenceKind>,
}

impl ProviderCatalogEntry {
    pub const fn id(&self) -> ProviderId {
        self.id
    }

    pub const fn display_name(&self) -> &'static str {
        self.display_name
    }

    pub const fn model_prefix(&self) -> &'static str {
        self.model_prefix
    }

    pub const fn credential_kind(&self) -> CredentialKind {
        self.credential_kind
    }

    pub const fn endpoint_key(&self) -> ProviderEndpointKey {
        self.endpoint_key
    }

    pub const fn protocol(&self) -> ProviderProtocol {
        self.protocol
    }

    pub const fn discovery(&self) -> ModelDiscoveryKind {
        self.discovery
    }

    pub fn parameter_rule(&self, key: &ProviderParameterKey) -> Option<ParameterApplicability> {
        self.parameter_rules.get(key).copied()
    }

    pub fn parameter_rules(&self) -> &BTreeMap<ProviderParameterKey, ParameterApplicability> {
        &self.parameter_rules
    }

    pub fn required_evidence(&self) -> &BTreeSet<EvidenceKind> {
        &self.required_evidence
    }

    pub fn selection_target(&self) -> SelectionTarget {
        match self.id {
            provider if provider.is_plan() => SelectionTarget::Plan {
                provider,
                plan: ProviderPlanId::new(plan_id(provider))
                    .expect("governed plan ID is a valid static token"),
            },
            provider => SelectionTarget::Provider(provider),
        }
    }
}

/// The product allowlist. It is constructed entirely from code and never from a transport
/// registry, environment variable, Profile, or remote response.
#[derive(Debug, Clone)]
pub struct GovernedProviderCatalog {
    entries: Vec<ProviderCatalogEntry>,
    legacy_entries: Vec<ProviderCatalogEntry>,
    digest: String,
}

impl Default for GovernedProviderCatalog {
    fn default() -> Self {
        let entries = ProviderId::ALL
            .into_iter()
            .map(entry_for)
            .collect::<Vec<_>>();
        let canonical = serde_json::json!({
            "schema_version": 1,
            "entries": entries,
        });
        let canonical = serde_json::to_vec(&canonical)
            .expect("governed Provider catalog serialization is infallible");
        let digest = hex::encode(Sha256::digest(canonical));
        let legacy_entries = ProviderId::LEGACY
            .into_iter()
            .map(entry_for)
            .collect::<Vec<_>>();
        Self {
            entries,
            legacy_entries,
            digest,
        }
    }
}

impl GovernedProviderCatalog {
    pub fn entries(&self) -> &[ProviderCatalogEntry] {
        &self.entries
    }

    pub fn entry(&self, provider: ProviderId) -> &ProviderCatalogEntry {
        self.entries
            .iter()
            .chain(&self.legacy_entries)
            .find(|entry| entry.id == provider)
            .expect("every core Provider belongs to the governed catalog")
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

fn entry_for(id: ProviderId) -> ProviderCatalogEntry {
    let (display_name, endpoint_key, protocol, discovery) = match id {
        ProviderId::OpenAi => (
            "openai",
            ProviderEndpointKey::OpenAi,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Kimi => (
            "kimi",
            ProviderEndpointKey::Kimi,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Qwen => (
            "qwen",
            ProviderEndpointKey::Qwen,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Gemini => (
            "gemini",
            ProviderEndpointKey::Gemini,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Glm => (
            "glm",
            ProviderEndpointKey::Glm,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::ChatGptSubscription => (
            "codex",
            ProviderEndpointKey::ChatGptBackend,
            ProviderProtocol::Responses,
            ModelDiscoveryKind::FixedBackend,
        ),
        ProviderId::ClaudeSubscription => (
            "claude code",
            ProviderEndpointKey::ClaudeSubscription,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::OpenCodeGo => (
            "OpenCode Go",
            ProviderEndpointKey::OpenCodeGo,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::AlibabaCoding => (
            "alibaba coding",
            ProviderEndpointKey::AlibabaCoding,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::BigModelCoding => (
            "bigmodel coding",
            ProviderEndpointKey::BigModelCoding,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::ZaiCoding => (
            "zai coding",
            ProviderEndpointKey::ZaiCoding,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::MiniMaxCoding => (
            "minimax coding",
            ProviderEndpointKey::MiniMaxCoding,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::KimiCoding => (
            "kimi coding",
            ProviderEndpointKey::KimiCoding,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::OpenCodeZen => (
            "OpenCode Zen",
            ProviderEndpointKey::OpenCodeZen,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::DeepSeek => (
            "deepseek",
            ProviderEndpointKey::DeepSeek,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Xai => (
            "xAI",
            ProviderEndpointKey::Xai,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Zai => (
            "Z.AI",
            ProviderEndpointKey::Zai,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::OpenRouter => (
            "openrouter",
            ProviderEndpointKey::OpenRouter,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::MiniMax => (
            "minimax",
            ProviderEndpointKey::MiniMax,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
        ProviderId::Anthropic => (
            "claude",
            ProviderEndpointKey::Anthropic,
            ProviderProtocol::Chat,
            ModelDiscoveryKind::ProviderCatalog,
        ),
    };
    ProviderCatalogEntry {
        id,
        display_name,
        model_prefix: id.model_prefix(),
        credential_kind: id.required_credential_kind(),
        endpoint_key,
        protocol,
        discovery,
        parameter_rules: common_parameter_rules(),
        required_evidence: EvidenceKind::ALL.into_iter().collect(),
    }
}

const fn plan_id(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::ChatGptSubscription => "codex",
        ProviderId::ClaudeSubscription => "claude_subscription",
        ProviderId::OpenCodeGo => "opencode_go",
        ProviderId::AlibabaCoding => "alibaba_coding",
        ProviderId::BigModelCoding => "bigmodel_coding",
        ProviderId::ZaiCoding => "zai_coding",
        ProviderId::MiniMaxCoding => "minimax_coding",
        ProviderId::KimiCoding => "kimi_coding",
        _ => "provider",
    }
}

fn common_parameter_rules() -> BTreeMap<ProviderParameterKey, ParameterApplicability> {
    [
        (
            ProviderParameterKey::Temperature,
            ParameterApplicability::Conditional,
        ),
        (
            ProviderParameterKey::MaxTokens,
            ParameterApplicability::Conditional,
        ),
        (
            ProviderParameterKey::Timeout,
            ParameterApplicability::Supported,
        ),
        (
            ProviderParameterKey::Retry,
            ParameterApplicability::Supported,
        ),
    ]
    .into_iter()
    .collect()
}
