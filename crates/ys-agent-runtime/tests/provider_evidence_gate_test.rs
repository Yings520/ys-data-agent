use std::{collections::BTreeSet, process::Command, time::Duration};

use chrono::{TimeDelta, Utc};
use ys_agent_core::{ProviderId, ProviderModelId, ProviderSupportStatus};
use ys_agent_runtime::provider::{
    catalog::GovernedProviderCatalog,
    evidence::GOVERNED_LITER_LLM_VERSION,
    evidence_collector::{
        EvidenceCollectionBaseline, EvidenceEnvironment, SanitizedProviderEvidenceDocument,
    },
    evidence_gate::{
        ProviderEvidenceApprovalDeclaration, ProviderEvidenceGate, ProviderEvidenceGateError,
        ProviderEvidenceManifest,
    },
    validation::COMPATIBILITY_PROBE_SCHEMA_VERSION,
};

const CODEC_DIGEST: &str = "68c43396fcc413377b975e93fd16b972c0e258c13693ca4288f5fdeca7f48d09";

fn baseline(catalog: &GovernedProviderCatalog) -> EvidenceCollectionBaseline {
    EvidenceCollectionBaseline {
        catalog_digest: catalog.digest().to_owned(),
        probe_schema_version: COMPATIBILITY_PROBE_SCHEMA_VERSION.to_owned(),
        codec_digest: CODEC_DIGEST.to_owned(),
        liter_llm_version: GOVERNED_LITER_LLM_VERSION.to_owned(),
    }
}

fn evidence_document(
    provider: ProviderId,
    baseline: EvidenceCollectionBaseline,
    collected_at: chrono::DateTime<Utc>,
) -> SanitizedProviderEvidenceDocument {
    let chatgpt = provider == ProviderId::ChatGptSubscription;
    SanitizedProviderEvidenceDocument {
        schema_version: 1,
        environment: EvidenceEnvironment::ApprovedEquivalent,
        provider,
        representative_model: ProviderModelId::new(
            provider,
            format!("{}release-gate-model", provider.model_prefix()),
        )
        .expect("catalog prefix"),
        credential_kind: provider.required_credential_kind(),
        collected_at,
        baseline,
        observed_evidence: BTreeSet::from([
            ys_agent_runtime::provider::evidence::EvidenceKind::Authentication,
            ys_agent_runtime::provider::evidence::EvidenceKind::Protocol,
            ys_agent_runtime::provider::evidence::EvidenceKind::Parameters,
            ys_agent_runtime::provider::evidence::EvidenceKind::ErrorBehavior,
        ]),
        tool_call_id_round_trip: true,
        multi_turn_tool_result: true,
        context: true,
        chatgpt_oauth: chatgpt,
        fixed_responses_backend: chatgpt,
    }
}

fn complete_manifest(
    catalog: &GovernedProviderCatalog,
    now: chrono::DateTime<Utc>,
) -> ProviderEvidenceManifest {
    let documents = ProviderId::ALL
        .into_iter()
        .map(|provider| evidence_document(provider, baseline(catalog), now))
        .collect::<Vec<_>>();
    let approvals = documents
        .iter()
        .map(|document| ProviderEvidenceApprovalDeclaration {
            provider: document.provider,
            document_digest: document.digest_sha256(),
        })
        .collect();
    ProviderEvidenceManifest::new(documents, approvals)
}

fn gate(catalog: GovernedProviderCatalog) -> ProviderEvidenceGate {
    let expected_baseline = baseline(&catalog);
    ProviderEvidenceGate::new(catalog, expected_baseline, Duration::from_secs(60 * 60))
        .expect("current governed baseline")
}

#[test]
fn missing_evidence_keeps_management_publishable_but_never_claims_supported_or_nine_of_nine() {
    let catalog = GovernedProviderCatalog::default();
    let verdict = gate(catalog).evaluate(&ProviderEvidenceManifest::empty(), Utc::now());

    assert!(verdict.catalog_is_exact());
    assert!(!verdict.is_nine_of_nine_supported());
    assert_eq!(verdict.providers().len(), ProviderId::ALL.len());
    assert!(verdict.providers().iter().all(|provider| {
        provider.support_status() == ProviderSupportStatus::Candidate
            && provider
                .evidence_gaps()
                .contains(&"missing_authentication_evidence".to_owned())
            && provider
                .evidence_gaps()
                .contains(&"missing_evidence_approval".to_owned())
    }));
    assert_eq!(
        verdict
            .require_nine_of_nine()
            .expect_err("an incomplete catalog cannot make a 9/9 release declaration"),
        ProviderEvidenceGateError::NineOfNineRequired
    );
}

#[test]
fn only_complete_current_and_approved_documents_allow_all_nine_supported() {
    let catalog = GovernedProviderCatalog::default();
    let now = Utc::now();
    let verdict = gate(catalog.clone()).evaluate(&complete_manifest(&catalog, now), now);

    assert!(verdict.catalog_is_exact());
    assert!(verdict.is_nine_of_nine_supported());
    assert!(
        verdict
            .providers()
            .iter()
            .all(|provider| provider.support_status() == ProviderSupportStatus::Supported)
    );
    verdict
        .require_nine_of_nine()
        .expect("a complete approved catalog may make the declaration");
}

#[test]
fn baseline_drift_or_incomplete_evidence_blocks_that_provider_and_sensitive_manifest_fields_fail_closed()
 {
    let catalog = GovernedProviderCatalog::default();
    let now = Utc::now();
    let mut manifest = complete_manifest(&catalog, now);
    let document = manifest
        .documents
        .iter_mut()
        .find(|document| document.provider == ProviderId::DeepSeek)
        .expect("DeepSeek document");
    document.baseline.codec_digest = "0".repeat(64);

    let verdict = gate(catalog.clone()).evaluate(&manifest, now);
    let deepseek = verdict
        .providers()
        .iter()
        .find(|provider| provider.provider() == ProviderId::DeepSeek)
        .expect("DeepSeek verdict");
    assert_eq!(deepseek.support_status(), ProviderSupportStatus::Blocked);
    assert_eq!(
        deepseek.evidence_gaps(),
        ["provider.evidence.baseline_mismatch"]
    );
    assert!(!verdict.is_nine_of_nine_supported());

    let mut incomplete = complete_manifest(&catalog, now);
    let document = incomplete
        .documents
        .iter_mut()
        .find(|document| document.provider == ProviderId::Anthropic)
        .expect("Anthropic document");
    document
        .observed_evidence
        .remove(&ys_agent_runtime::provider::evidence::EvidenceKind::ErrorBehavior);
    let verdict = gate(catalog.clone()).evaluate(&incomplete, now);
    let anthropic = verdict
        .providers()
        .iter()
        .find(|provider| provider.provider() == ProviderId::Anthropic)
        .expect("Anthropic verdict");
    assert_eq!(anthropic.support_status(), ProviderSupportStatus::Blocked);
    assert_eq!(anthropic.evidence_gaps(), ["provider.evidence.incomplete"]);

    let mut expired = complete_manifest(&catalog, now);
    expired.documents[0].collected_at = now - TimeDelta::days(2);
    assert_eq!(
        gate(catalog.clone()).evaluate(&expired, now).providers()[0].support_status(),
        ProviderSupportStatus::Blocked
    );

    let mut raw_manifest = serde_json::to_value(complete_manifest(&catalog, now))
        .expect("sanitized manifest serialization");
    raw_manifest["documents"][0]["token"] = serde_json::json!("must-not-be-accepted");
    assert_eq!(
        ProviderEvidenceManifest::from_json(&raw_manifest.to_string())
            .expect_err("unknown sensitive fields must fail closed"),
        ProviderEvidenceGateError::InvalidManifest
    );
}

#[test]
fn release_gate_binary_reports_candidate_management_mode_and_rejects_strict_nine_of_nine_without_evidence()
 {
    let management = Command::new(env!("CARGO_BIN_EXE_provider-evidence-gate"))
        .output()
        .expect("run provider evidence gate");
    assert!(management.status.success());
    let management_stdout = String::from_utf8(management.stdout).expect("UTF-8 output");
    assert!(management_stdout.contains("\"nine_of_nine_supported\":false"));
    assert!(management_stdout.contains("missing_evidence_approval"));

    let strict = Command::new(env!("CARGO_BIN_EXE_provider-evidence-gate"))
        .arg("--require-nine-of-nine")
        .output()
        .expect("run strict provider evidence gate");
    assert!(!strict.status.success());
    let strict_stderr = String::from_utf8(strict.stderr).expect("UTF-8 output");
    assert!(strict_stderr.contains("provider.evidence.nine_of_nine_required"));
}
