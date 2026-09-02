use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use ys_agent_core::{ProviderId, ProviderModelId};
use ys_agent_runtime::provider::evidence_collector::{
    ApprovedEvidenceCollection, EvidenceCollectionBaseline, EvidenceCollectionError,
    EvidenceCollectionResult, EvidenceCollectionTarget, EvidenceProbeObservation,
    LiveEvidenceCollector, LiveEvidenceProbe, SanitizedProviderEvidenceDocument,
};

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn baseline() -> EvidenceCollectionBaseline {
    EvidenceCollectionBaseline {
        catalog_digest: digest("catalog-v1"),
        probe_schema_version: "provider-probe-v1".to_owned(),
        codec_digest: digest("codec-v1"),
        liter_llm_version: "1.19.1".to_owned(),
    }
}

fn target(provider: ProviderId) -> EvidenceCollectionTarget {
    EvidenceCollectionTarget::new(
        provider,
        ProviderModelId::new(
            provider,
            format!("{}fixture-model", provider.model_prefix()),
        )
        .expect("governed representative model"),
    )
}

#[derive(Default)]
struct FixtureProbe {
    calls: Mutex<Vec<ProviderId>>,
    incomplete: bool,
}

#[async_trait]
impl LiveEvidenceProbe for FixtureProbe {
    async fn probe(
        &self,
        target: &EvidenceCollectionTarget,
    ) -> EvidenceCollectionResult<EvidenceProbeObservation> {
        self.calls
            .lock()
            .expect("fixture probe state")
            .push(target.provider);
        let chatgpt = target.provider == ProviderId::ChatGptSubscription;
        Ok(EvidenceProbeObservation {
            authentication: true,
            tool_call: true,
            tool_call_id_round_trip: true,
            multi_turn_tool_result: true,
            context: true,
            parameters: !self.incomplete,
            error_behavior: true,
            chatgpt_oauth: chatgpt,
            fixed_responses_backend: chatgpt,
        })
    }
}

async fn collect_fixture_evidence() {
    let baseline = baseline();
    let collector =
        LiveEvidenceCollector::new(ApprovedEvidenceCollection::equivalent(), baseline.clone())
            .expect("complete version baseline");
    let probe = FixtureProbe::default();
    let now = Utc::now();

    for provider in ProviderId::ALL {
        let target = target(provider);
        let document = collector
            .collect(target.clone(), &probe, now)
            .await
            .expect("fixture provides all required probe categories");
        let encoded = document.to_json().expect("sanitized JSON");
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("raw_prompt"));
        assert!(!encoded.contains("raw_response"));
        assert!(!encoded.contains("customer"));
        let restored = SanitizedProviderEvidenceDocument::from_json(
            &encoded,
            &baseline,
            &target,
            Duration::from_secs(60),
            now,
        )
        .expect("schema validates the emitted evidence");
        assert_eq!(restored.digest_sha256(), document.digest_sha256());
    }
    assert_eq!(
        probe.calls.lock().expect("fixture probe state").as_slice(),
        ProviderId::ALL
    );
}

#[tokio::test]
async fn approved_evidence_collection_accepts_only_sanitized_complete_documents() {
    collect_fixture_evidence().await;
}

#[tokio::test]
async fn evidence_documents_reject_incomplete_expired_mismatched_or_sensitive_inputs() {
    let baseline = baseline();
    let target = target(ProviderId::ChatGptSubscription);
    let now = Utc::now();
    let collector =
        LiveEvidenceCollector::new(ApprovedEvidenceCollection::production(), baseline.clone())
            .expect("complete baseline");
    let document = collector
        .collect(target.clone(), &FixtureProbe::default(), now)
        .await
        .expect("complete ChatGPT fixture evidence");

    let mut incomplete = serde_json::to_value(&document).expect("serialize document");
    incomplete["observed_evidence"] = json!(["authentication", "protocol", "parameters"]);
    assert_eq!(
        SanitizedProviderEvidenceDocument::from_json(
            &incomplete.to_string(),
            &baseline,
            &target,
            Duration::from_secs(60),
            now,
        )
        .expect_err("missing error evidence is rejected"),
        EvidenceCollectionError::Incomplete
    );

    let mut expired = serde_json::to_value(&document).expect("serialize document");
    expired["collected_at"] = json!((now - TimeDelta::days(8)).to_rfc3339());
    assert_eq!(
        SanitizedProviderEvidenceDocument::from_json(
            &expired.to_string(),
            &baseline,
            &target,
            Duration::from_secs(60),
            now,
        )
        .expect_err("expired evidence is rejected"),
        EvidenceCollectionError::Expired
    );

    let mut mismatched = document.clone();
    mismatched.baseline.catalog_digest = digest("other-catalog");
    assert_eq!(
        mismatched
            .validate(&baseline, &target, Duration::from_secs(60), now)
            .expect_err("a changed catalog digest is rejected"),
        EvidenceCollectionError::BaselineMismatch
    );

    let mut raw_field = serde_json::to_value(&document).expect("serialize document");
    raw_field["token"] = json!("credential-canary-must-not-enter-evidence");
    assert_eq!(
        SanitizedProviderEvidenceDocument::from_json(
            &raw_field.to_string(),
            &baseline,
            &target,
            Duration::from_secs(60),
            now,
        )
        .expect_err("schema denies a raw token field"),
        EvidenceCollectionError::InvalidDocument
    );

    let failed_collection = collector
        .collect(
            target,
            &FixtureProbe {
                incomplete: true,
                ..FixtureProbe::default()
            },
            now,
        )
        .await
        .expect_err("collector fails closed when a probe category is absent");
    assert_eq!(failed_collection, EvidenceCollectionError::Incomplete);
}
