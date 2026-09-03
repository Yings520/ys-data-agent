//! Release-time derivation of Provider support from sanitized, approved evidence.
//!
//! An absent evidence manifest is a valid management-release state: every Provider remains a
//! Candidate with explicit gaps. A caller must opt into the stricter 9/9 declaration, which is
//! rejected unless every governed Provider has current complete evidence and matching approval.

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ys_agent_core::{ProviderId, ProviderSupportStatus};

use super::{
    catalog::GovernedProviderCatalog,
    evidence::{EvidenceApproval, EvidenceBaseline, EvidenceRegistry, ProviderEvidence},
    evidence_collector::{
        EvidenceCollectionBaseline, EvidenceCollectionTarget, SanitizedProviderEvidenceDocument,
    },
};

pub const PROVIDER_EVIDENCE_MANIFEST_SCHEMA_VERSION: u16 = 1;

/// An approval references a sanitized document digest; it never carries raw probe data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEvidenceApprovalDeclaration {
    pub provider: ProviderId,
    pub document_digest: String,
}

/// The only on-disk release-evidence shape accepted by the gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEvidenceManifest {
    pub schema_version: u16,
    pub documents: Vec<SanitizedProviderEvidenceDocument>,
    pub approvals: Vec<ProviderEvidenceApprovalDeclaration>,
}

impl ProviderEvidenceManifest {
    pub fn new(
        documents: Vec<SanitizedProviderEvidenceDocument>,
        approvals: Vec<ProviderEvidenceApprovalDeclaration>,
    ) -> Self {
        Self {
            schema_version: PROVIDER_EVIDENCE_MANIFEST_SCHEMA_VERSION,
            documents,
            approvals,
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), Vec::new())
    }

    pub fn from_json(source: &str) -> ProviderEvidenceGateResult<Self> {
        let manifest: Self =
            serde_json::from_str(source).map_err(|_| ProviderEvidenceGateError::InvalidManifest)?;
        if manifest.schema_version != PROVIDER_EVIDENCE_MANIFEST_SCHEMA_VERSION {
            return Err(ProviderEvidenceGateError::InvalidManifest);
        }
        Ok(manifest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProviderEvidenceGateError {
    #[error("Provider evidence manifest is invalid")]
    InvalidManifest,
    #[error("Provider evidence baseline does not match the governed catalog")]
    BaselineMismatch,
    #[error("The governed Provider catalog is not exactly the supported nine")]
    CatalogMismatch,
    #[error("A 9/9 Provider support declaration requires approved current evidence")]
    NineOfNineRequired,
}

impl ProviderEvidenceGateError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidManifest => "provider.evidence.invalid_manifest",
            Self::BaselineMismatch => "provider.evidence.baseline_mismatch",
            Self::CatalogMismatch => "provider.evidence.catalog_mismatch",
            Self::NineOfNineRequired => "provider.evidence.nine_of_nine_required",
        }
    }
}

pub type ProviderEvidenceGateResult<T> = Result<T, ProviderEvidenceGateError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderEvidenceGateProvider {
    provider: ProviderId,
    support_status: ProviderSupportStatus,
    evidence_gaps: Vec<String>,
}

impl ProviderEvidenceGateProvider {
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    pub fn support_status(&self) -> ProviderSupportStatus {
        self.support_status.clone()
    }

    pub fn evidence_gaps(&self) -> &[String] {
        &self.evidence_gaps
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderEvidenceGateVerdict {
    catalog_is_exact: bool,
    nine_of_nine_supported: bool,
    providers: Vec<ProviderEvidenceGateProvider>,
}

impl ProviderEvidenceGateVerdict {
    pub const fn catalog_is_exact(&self) -> bool {
        self.catalog_is_exact
    }

    pub const fn is_nine_of_nine_supported(&self) -> bool {
        self.nine_of_nine_supported
    }

    pub fn providers(&self) -> &[ProviderEvidenceGateProvider] {
        &self.providers
    }

    pub fn require_nine_of_nine(&self) -> ProviderEvidenceGateResult<()> {
        self.nine_of_nine_supported
            .then_some(())
            .ok_or(ProviderEvidenceGateError::NineOfNineRequired)
    }
}

/// Computes Provider support for an exact governed catalog and one current evidence baseline.
#[derive(Debug, Clone)]
pub struct ProviderEvidenceGate {
    catalog: GovernedProviderCatalog,
    expected_baseline: EvidenceCollectionBaseline,
    maximum_age: Duration,
}

impl ProviderEvidenceGate {
    pub fn new(
        catalog: GovernedProviderCatalog,
        expected_baseline: EvidenceCollectionBaseline,
        maximum_age: Duration,
    ) -> ProviderEvidenceGateResult<Self> {
        if !catalog_is_exact(&catalog) {
            return Err(ProviderEvidenceGateError::CatalogMismatch);
        }
        if !expected_baseline.is_complete()
            || expected_baseline.catalog_digest != catalog.digest()
            || maximum_age.is_zero()
        {
            return Err(ProviderEvidenceGateError::BaselineMismatch);
        }
        Ok(Self {
            catalog,
            expected_baseline,
            maximum_age,
        })
    }

    /// Evaluates each catalog entry independently so management can still show actionable gaps.
    /// Invalid submitted evidence always blocks its Provider; it can never be downgraded into a
    /// misleading Supported result.
    pub fn evaluate(
        &self,
        manifest: &ProviderEvidenceManifest,
        now: DateTime<Utc>,
    ) -> ProviderEvidenceGateVerdict {
        if manifest.schema_version != PROVIDER_EVIDENCE_MANIFEST_SCHEMA_VERSION {
            return invalid_manifest_verdict(&self.catalog);
        }
        let mut documents = HashMap::new();
        let mut approvals = HashMap::new();
        let mut invalid = HashMap::new();

        for document in &manifest.documents {
            let target = EvidenceCollectionTarget::new(
                document.provider,
                document.representative_model.clone(),
            );
            let error = document
                .validate(&self.expected_baseline, &target, self.maximum_age, now)
                .err();
            if documents.insert(document.provider, document).is_some() {
                invalid.insert(
                    document.provider,
                    ProviderEvidenceGateError::InvalidManifest.code(),
                );
            } else if let Some(error) = error {
                invalid.insert(document.provider, error.code());
            }
        }

        for approval in &manifest.approvals {
            if approvals.insert(approval.provider, approval).is_some()
                || !documents.contains_key(&approval.provider)
            {
                invalid.insert(
                    approval.provider,
                    ProviderEvidenceGateError::InvalidManifest.code(),
                );
            }
        }

        let registry_baseline = EvidenceBaseline::for_catalog(
            &self.catalog,
            self.expected_baseline.probe_schema_version.clone(),
            self.expected_baseline.codec_digest.clone(),
            self.expected_baseline.liter_llm_version.clone(),
        );
        let mut registry = EvidenceRegistry::new(registry_baseline);
        for provider in ProviderId::ALL {
            if invalid.contains_key(&provider) {
                continue;
            }
            let (Some(document), Some(approval)) =
                (documents.get(&provider), approvals.get(&provider))
            else {
                continue;
            };
            if approval.document_digest != document.digest_sha256() {
                invalid.insert(provider, "evidence_approval_mismatch");
                continue;
            }
            let evidence = ProviderEvidence::new(
                provider,
                document.representative_model.clone(),
                document.credential_kind,
                registry.baseline().clone(),
                document.evidence_hashes(),
                self.catalog.entry(provider).required_evidence().clone(),
            );
            let registry_approval = EvidenceApproval::new(provider, evidence.manifest_digest());
            registry.register(evidence, registry_approval);
        }

        let providers = ProviderId::ALL
            .into_iter()
            .map(|provider| {
                if let Some(error) = invalid.get(&provider) {
                    return ProviderEvidenceGateProvider {
                        provider,
                        support_status: ProviderSupportStatus::Blocked,
                        evidence_gaps: vec![(*error).to_owned()],
                    };
                }
                if documents.contains_key(&provider) && !approvals.contains_key(&provider) {
                    return ProviderEvidenceGateProvider {
                        provider,
                        support_status: ProviderSupportStatus::Candidate,
                        evidence_gaps: vec!["missing_evidence_approval".to_owned()],
                    };
                }
                let support = registry.derive(&self.catalog, provider);
                ProviderEvidenceGateProvider {
                    provider,
                    support_status: support.status(),
                    evidence_gaps: support.gaps().iter().map(|gap| gap.code()).collect(),
                }
            })
            .collect::<Vec<_>>();
        let catalog_is_exact = catalog_is_exact(&self.catalog);
        let nine_of_nine_supported = catalog_is_exact
            && providers.len() == ProviderId::ALL.len()
            && providers
                .iter()
                .all(|provider| provider.support_status == ProviderSupportStatus::Supported);
        ProviderEvidenceGateVerdict {
            catalog_is_exact,
            nine_of_nine_supported,
            providers,
        }
    }
}

fn catalog_is_exact(catalog: &GovernedProviderCatalog) -> bool {
    catalog.entries().len() == ProviderId::ALL.len()
        && catalog
            .entries()
            .iter()
            .zip(ProviderId::ALL)
            .all(|(entry, provider)| entry.id() == provider)
}

fn invalid_manifest_verdict(catalog: &GovernedProviderCatalog) -> ProviderEvidenceGateVerdict {
    ProviderEvidenceGateVerdict {
        catalog_is_exact: catalog_is_exact(catalog),
        nine_of_nine_supported: false,
        providers: ProviderId::ALL
            .into_iter()
            .map(|provider| ProviderEvidenceGateProvider {
                provider,
                support_status: ProviderSupportStatus::Blocked,
                evidence_gaps: vec![ProviderEvidenceGateError::InvalidManifest.code().to_owned()],
            })
            .collect(),
    }
}
