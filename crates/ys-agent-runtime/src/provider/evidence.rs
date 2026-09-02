use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ys_agent_core::{
    CredentialKind, ProviderCatalogView, ProviderId, ProviderModelId, ProviderSupportStatus,
};

use super::catalog::GovernedProviderCatalog;

pub const GOVERNED_LITER_LLM_VERSION: &str = "1.19.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Authentication,
    Protocol,
    Parameters,
    ErrorBehavior,
}

impl EvidenceKind {
    pub const ALL: [Self; 4] = [
        Self::Authentication,
        Self::Protocol,
        Self::Parameters,
        Self::ErrorBehavior,
    ];

    const fn code(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Protocol => "protocol",
            Self::Parameters => "parameters",
            Self::ErrorBehavior => "error_behavior",
        }
    }
}

/// The complete version baseline that sanitized evidence must have observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceBaseline {
    catalog_digest: String,
    probe_schema_version: String,
    codec_digest: String,
    liter_llm_version: String,
}

impl EvidenceBaseline {
    pub fn for_catalog(
        catalog: &GovernedProviderCatalog,
        probe_schema_version: impl Into<String>,
        codec_digest: impl Into<String>,
        liter_llm_version: impl Into<String>,
    ) -> Self {
        Self {
            catalog_digest: catalog.digest().to_owned(),
            probe_schema_version: probe_schema_version.into(),
            codec_digest: codec_digest.into(),
            liter_llm_version: liter_llm_version.into(),
        }
    }
}

/// Sanitized evidence metadata. Hashes identify separately stored approved evidence; this record
/// contains no raw request, response, account identity, credential, or customer data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderEvidence {
    provider: ProviderId,
    representative_model: ProviderModelId,
    credential_kind: CredentialKind,
    baseline: EvidenceBaseline,
    evidence_hashes: BTreeMap<EvidenceKind, String>,
    required_evidence: BTreeSet<EvidenceKind>,
}

impl ProviderEvidence {
    pub fn new(
        provider: ProviderId,
        representative_model: ProviderModelId,
        credential_kind: CredentialKind,
        baseline: EvidenceBaseline,
        evidence_hashes: BTreeMap<EvidenceKind, String>,
        required_evidence: BTreeSet<EvidenceKind>,
    ) -> Self {
        Self {
            provider,
            representative_model,
            credential_kind,
            baseline,
            evidence_hashes,
            required_evidence,
        }
    }

    pub fn representative_model(&self) -> &ProviderModelId {
        &self.representative_model
    }

    pub fn manifest_digest(&self) -> String {
        let canonical = serde_json::to_vec(self)
            .expect("sanitized Provider evidence serialization is infallible");
        hex::encode(Sha256::digest(canonical))
    }
}

/// Human approval is intentionally distinct from the evidence record. Rewriting any evidence
/// hash without refreshing this approval therefore fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceApproval {
    provider: ProviderId,
    manifest_digest: String,
}

impl EvidenceApproval {
    pub fn new(provider: ProviderId, manifest_digest: impl Into<String>) -> Self {
        Self {
            provider,
            manifest_digest: manifest_digest.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceGap {
    MissingEvidence(EvidenceKind),
    InvalidEvidenceHash(EvidenceKind),
    MissingApproval,
    EvidenceApprovalMismatch,
    ApprovalProviderMismatch,
    CatalogDigestMismatch,
    ProbeDigestMismatch,
    CodecDigestMismatch,
    LiterVersionMismatch,
    ModelProviderMismatch,
    CredentialKindMismatch,
    RequiredEvidenceMismatch,
}

impl EvidenceGap {
    pub fn code(&self) -> String {
        match self {
            Self::MissingEvidence(kind) => format!("missing_{}_evidence", kind.code()),
            Self::InvalidEvidenceHash(kind) => {
                format!("invalid_{}_evidence_hash", kind.code())
            }
            Self::MissingApproval => "missing_evidence_approval".to_owned(),
            Self::EvidenceApprovalMismatch => "evidence_approval_mismatch".to_owned(),
            Self::ApprovalProviderMismatch => "evidence_approval_provider_mismatch".to_owned(),
            Self::CatalogDigestMismatch => "catalog_digest_mismatch".to_owned(),
            Self::ProbeDigestMismatch => "probe_digest_mismatch".to_owned(),
            Self::CodecDigestMismatch => "codec_digest_mismatch".to_owned(),
            Self::LiterVersionMismatch => "liter_llm_version_mismatch".to_owned(),
            Self::ModelProviderMismatch => "representative_model_provider_mismatch".to_owned(),
            Self::CredentialKindMismatch => "credential_kind_mismatch".to_owned(),
            Self::RequiredEvidenceMismatch => "required_evidence_mismatch".to_owned(),
        }
    }

    const fn is_candidate_gap(&self) -> bool {
        matches!(self, Self::MissingEvidence(_) | Self::MissingApproval)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedProviderSupport {
    provider: ProviderId,
    status: ProviderSupportStatus,
    gaps: Vec<EvidenceGap>,
}

impl DerivedProviderSupport {
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn status(&self) -> ProviderSupportStatus {
        self.status.clone()
    }

    pub fn gaps(&self) -> &[EvidenceGap] {
        &self.gaps
    }
}

/// Derives support only from current, complete and independently approved model evidence.
#[derive(Debug, Clone)]
pub struct EvidenceRegistry {
    baseline: EvidenceBaseline,
    records: HashMap<ProviderId, (ProviderEvidence, EvidenceApproval)>,
}

impl EvidenceRegistry {
    pub fn new(baseline: EvidenceBaseline) -> Self {
        Self {
            baseline,
            records: HashMap::new(),
        }
    }

    pub fn baseline(&self) -> &EvidenceBaseline {
        &self.baseline
    }

    pub fn register(&mut self, evidence: ProviderEvidence, approval: EvidenceApproval) {
        self.records.insert(evidence.provider, (evidence, approval));
    }

    pub fn derive(
        &self,
        catalog: &GovernedProviderCatalog,
        provider: ProviderId,
    ) -> DerivedProviderSupport {
        let entry = catalog.entry(provider);
        let Some((evidence, approval)) = self.records.get(&provider) else {
            let mut gaps = entry
                .required_evidence()
                .iter()
                .copied()
                .map(EvidenceGap::MissingEvidence)
                .collect::<Vec<_>>();
            gaps.push(EvidenceGap::MissingApproval);
            return DerivedProviderSupport {
                provider,
                status: ProviderSupportStatus::Candidate,
                gaps,
            };
        };

        let mut gaps = Vec::new();
        if evidence.representative_model.provider() != provider {
            gaps.push(EvidenceGap::ModelProviderMismatch);
        }
        if evidence.credential_kind != entry.credential_kind() {
            gaps.push(EvidenceGap::CredentialKindMismatch);
        }
        if evidence.baseline.catalog_digest != self.baseline.catalog_digest
            || evidence.baseline.catalog_digest != catalog.digest()
        {
            gaps.push(EvidenceGap::CatalogDigestMismatch);
        }
        if evidence.baseline.probe_schema_version != self.baseline.probe_schema_version {
            gaps.push(EvidenceGap::ProbeDigestMismatch);
        }
        if self.baseline.probe_schema_version.trim().is_empty() {
            gaps.push(EvidenceGap::ProbeDigestMismatch);
        }
        if evidence.baseline.codec_digest != self.baseline.codec_digest {
            gaps.push(EvidenceGap::CodecDigestMismatch);
        }
        if self.baseline.codec_digest.trim().is_empty() {
            gaps.push(EvidenceGap::CodecDigestMismatch);
        }
        if evidence.baseline.liter_llm_version != self.baseline.liter_llm_version {
            gaps.push(EvidenceGap::LiterVersionMismatch);
        }
        if self.baseline.liter_llm_version != GOVERNED_LITER_LLM_VERSION {
            gaps.push(EvidenceGap::LiterVersionMismatch);
        }
        if evidence.required_evidence != *entry.required_evidence() {
            gaps.push(EvidenceGap::RequiredEvidenceMismatch);
        }
        for kind in entry.required_evidence() {
            match evidence.evidence_hashes.get(kind) {
                None => gaps.push(EvidenceGap::MissingEvidence(*kind)),
                Some(hash) if !is_sha256_hex(hash) => {
                    gaps.push(EvidenceGap::InvalidEvidenceHash(*kind));
                }
                Some(_) => {}
            }
        }
        if approval.provider != provider {
            gaps.push(EvidenceGap::ApprovalProviderMismatch);
        }
        if approval.manifest_digest != evidence.manifest_digest() {
            gaps.push(EvidenceGap::EvidenceApprovalMismatch);
        }

        let status = if gaps.is_empty() {
            ProviderSupportStatus::Supported
        } else if gaps.iter().all(EvidenceGap::is_candidate_gap) {
            ProviderSupportStatus::Candidate
        } else {
            ProviderSupportStatus::Blocked
        };
        DerivedProviderSupport {
            provider,
            status,
            gaps,
        }
    }

    pub fn derive_all(&self, catalog: &GovernedProviderCatalog) -> Vec<DerivedProviderSupport> {
        catalog
            .entries()
            .iter()
            .map(|entry| self.derive(catalog, entry.id()))
            .collect()
    }

    pub fn catalog_views(&self, catalog: &GovernedProviderCatalog) -> Vec<ProviderCatalogView> {
        self.derive_all(catalog)
            .into_iter()
            .map(|support| {
                let entry = catalog.entry(support.provider);
                ProviderCatalogView {
                    provider: support.provider,
                    display_name: entry.display_name().to_owned(),
                    credential_kind: entry.credential_kind(),
                    support_status: support.status,
                    evidence_gaps: support.gaps.iter().map(EvidenceGap::code).collect(),
                }
            })
            .collect()
    }

    pub fn is_nine_of_nine_supported(&self, catalog: &GovernedProviderCatalog) -> bool {
        let statuses = self.derive_all(catalog);
        statuses.len() == ProviderId::ALL.len()
            && statuses
                .iter()
                .all(|status| status.status == ProviderSupportStatus::Supported)
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
