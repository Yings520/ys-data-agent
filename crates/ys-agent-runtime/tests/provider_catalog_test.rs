use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use ys_agent_core::{
    CredentialKind, ParameterApplicability, ProviderId, ProviderModelId, ProviderParameterKey,
    ProviderSupportStatus,
};
use ys_agent_runtime::provider::{
    catalog::{GovernedProviderCatalog, ModelDiscoveryKind, ProviderProtocol},
    evidence::{
        EvidenceApproval, EvidenceBaseline, EvidenceGap, EvidenceKind, EvidenceRegistry,
        ProviderEvidence,
    },
};

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn complete_hashes(provider: ProviderId) -> BTreeMap<EvidenceKind, String> {
    EvidenceKind::ALL
        .into_iter()
        .map(|kind| (kind, hash(&format!("{provider:?}:{kind:?}"))))
        .collect()
}

fn evidence_for(
    catalog: &GovernedProviderCatalog,
    baseline: &EvidenceBaseline,
    provider: ProviderId,
) -> ProviderEvidence {
    ProviderEvidence::new(
        provider,
        ProviderModelId::new(
            provider,
            format!("{}representative-model", provider.model_prefix()),
        )
        .expect("representative model uses the governed prefix"),
        provider.required_credential_kind(),
        baseline.clone(),
        complete_hashes(provider),
        catalog.entry(provider).required_evidence().clone(),
    )
}

#[test]
fn governed_catalog_is_exact_offline_and_has_no_dynamic_or_openai_route() {
    let catalog = GovernedProviderCatalog::default();
    let entries = catalog.entries();
    assert_eq!(entries.len(), 9);
    assert_eq!(
        entries.iter().map(|entry| entry.id()).collect::<Vec<_>>(),
        ProviderId::ALL
    );

    let names = entries
        .iter()
        .map(|entry| entry.display_name())
        .collect::<BTreeSet<_>>();
    let prefixes = entries
        .iter()
        .map(|entry| entry.model_prefix())
        .collect::<BTreeSet<_>>();
    let endpoints = entries
        .iter()
        .map(|entry| entry.endpoint_key())
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), 9);
    assert_eq!(prefixes.len(), 9);
    assert_eq!(endpoints.len(), 9);

    for entry in entries {
        assert_eq!(entry.model_prefix(), entry.id().model_prefix());
        assert_eq!(
            entry.credential_kind(),
            entry.id().required_credential_kind()
        );
        assert!(!entry.model_prefix().starts_with("openai/"));
        assert!(!entry.display_name().to_ascii_lowercase().contains("openai"));
        assert!(matches!(
            entry.discovery(),
            ModelDiscoveryKind::FixedBackend | ModelDiscoveryKind::ProviderCatalog
        ));
        assert_eq!(
            entry.parameter_rule(&ProviderParameterKey::Timeout),
            Some(ParameterApplicability::Supported)
        );
        assert_eq!(
            entry.parameter_rule(&ProviderParameterKey::Retry),
            Some(ParameterApplicability::Supported)
        );
        assert_eq!(
            entry.parameter_rule(&ProviderParameterKey::Temperature),
            Some(ParameterApplicability::Conditional)
        );
        assert_eq!(
            entry.parameter_rule(&ProviderParameterKey::MaxTokens),
            Some(ParameterApplicability::Conditional)
        );
        assert_eq!(
            entry.required_evidence(),
            &EvidenceKind::ALL.into_iter().collect()
        );
    }
    assert_eq!(
        catalog.entry(ProviderId::ChatGptSubscription).protocol(),
        ProviderProtocol::Responses
    );
    assert_eq!(
        catalog
            .entry(ProviderId::ChatGptSubscription)
            .credential_kind(),
        CredentialKind::OAuthConnection
    );
    assert!(
        entries
            .iter()
            .filter(|entry| entry.id() != ProviderId::ChatGptSubscription)
            .all(|entry| entry.protocol() == ProviderProtocol::Chat)
    );
}

#[test]
fn static_catalog_without_approved_evidence_can_only_be_candidate() {
    let catalog = GovernedProviderCatalog::default();
    let baseline = EvidenceBaseline::for_catalog(&catalog, "probe-v1", "codec-v1", "1.19.1");
    let registry = EvidenceRegistry::new(baseline);

    let statuses = registry.derive_all(&catalog);
    assert_eq!(statuses.len(), 9);
    assert!(!registry.is_nine_of_nine_supported(&catalog));
    for status in statuses {
        assert_eq!(status.status(), ProviderSupportStatus::Candidate);
        for kind in EvidenceKind::ALL {
            assert!(status.gaps().contains(&EvidenceGap::MissingEvidence(kind)));
        }
        assert!(status.gaps().contains(&EvidenceGap::MissingApproval));
    }
    assert!(registry.catalog_views(&catalog).iter().all(|view| {
        view.support_status == ProviderSupportStatus::Candidate && !view.evidence_gaps.is_empty()
    }));
}

#[test]
fn only_complete_current_and_separately_approved_evidence_derives_supported() {
    let catalog = GovernedProviderCatalog::default();
    let baseline = EvidenceBaseline::for_catalog(&catalog, "probe-v1", "codec-v1", "1.19.1");
    let mut registry = EvidenceRegistry::new(baseline.clone());
    for provider in ProviderId::ALL {
        let evidence = evidence_for(&catalog, &baseline, provider);
        let approval = EvidenceApproval::new(provider, evidence.manifest_digest());
        registry.register(evidence, approval);
    }

    assert!(registry.is_nine_of_nine_supported(&catalog));
    assert!(
        registry
            .derive_all(&catalog)
            .iter()
            .all(|status| status.status() == ProviderSupportStatus::Supported
                && status.gaps().is_empty())
    );
}

#[test]
fn missing_stale_or_changed_evidence_downgrades_with_specific_gaps() {
    let catalog = GovernedProviderCatalog::default();
    let baseline = EvidenceBaseline::for_catalog(&catalog, "probe-v1", "codec-v1", "1.19.1");
    let provider = ProviderId::DeepSeek;
    let approved = evidence_for(&catalog, &baseline, provider);
    let approval = EvidenceApproval::new(provider, approved.manifest_digest());

    let mut changed_hashes = complete_hashes(provider);
    changed_hashes.insert(EvidenceKind::Protocol, hash("changed-protocol-evidence"));
    let changed = ProviderEvidence::new(
        provider,
        approved.representative_model().clone(),
        provider.required_credential_kind(),
        baseline.clone(),
        changed_hashes,
        catalog.entry(provider).required_evidence().clone(),
    );
    let mut registry = EvidenceRegistry::new(baseline.clone());
    registry.register(changed, approval);
    let status = registry.derive(&catalog, provider);
    assert_eq!(status.status(), ProviderSupportStatus::Blocked);
    assert!(
        status
            .gaps()
            .contains(&EvidenceGap::EvidenceApprovalMismatch)
    );

    let stale_baseline = EvidenceBaseline::for_catalog(&catalog, "probe-v1", "old-codec", "1.19.1");
    let stale = evidence_for(&catalog, &stale_baseline, provider);
    let stale_approval = EvidenceApproval::new(provider, stale.manifest_digest());
    let mut registry = EvidenceRegistry::new(baseline);
    registry.register(stale, stale_approval);
    let status = registry.derive(&catalog, provider);
    assert_eq!(status.status(), ProviderSupportStatus::Blocked);
    assert!(status.gaps().contains(&EvidenceGap::CodecDigestMismatch));

    let mut missing_hashes = complete_hashes(provider);
    missing_hashes.remove(&EvidenceKind::ErrorBehavior);
    let incomplete = ProviderEvidence::new(
        provider,
        approved.representative_model().clone(),
        provider.required_credential_kind(),
        registry.baseline().clone(),
        missing_hashes,
        catalog.entry(provider).required_evidence().clone(),
    );
    let incomplete_approval = EvidenceApproval::new(provider, incomplete.manifest_digest());
    registry.register(incomplete, incomplete_approval);
    let status = registry.derive(&catalog, provider);
    assert_eq!(status.status(), ProviderSupportStatus::Candidate);
    assert!(
        status
            .gaps()
            .contains(&EvidenceGap::MissingEvidence(EvidenceKind::ErrorBehavior))
    );
}
