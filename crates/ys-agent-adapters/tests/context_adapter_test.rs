use std::path::{Path, PathBuf};

use ys_agent_adapters::{DbtManifestAdapter, FileMetricRegistry};
use ys_agent_core::{
    ContextSourceType, InstructionTrust, MetricProvider, MetricStatus, QueryContextProvider,
};

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[tokio::test]
async fn only_active_metrics_are_queryable_by_default() {
    let registry = FileMetricRegistry::load(fixture_path("fixtures/metrics/metrics.json"))
        .await
        .expect("load metric registry");

    assert!(
        registry
            .resolve_active("commerce.gmv")
            .await
            .expect("resolve active metric")
            .is_some()
    );
    assert!(
        registry
            .resolve_active("commerce.gmv_draft")
            .await
            .expect("resolve draft metric")
            .is_none()
    );
}

#[tokio::test]
async fn display_alias_resolution_is_case_insensitive_but_still_active_only() {
    let registry = FileMetricRegistry::load(fixture_path("fixtures/metrics/metrics.json"))
        .await
        .expect("load metric registry");
    let metric = registry
        .get_metric("GMV")
        .await
        .expect("resolve alias")
        .expect("active metric");

    assert_eq!(metric.id, "commerce.gmv");
    assert_eq!(metric.status, MetricStatus::Active);
    assert_eq!(metric.owner, "data-team");
}

#[tokio::test]
async fn dbt_manifest_evidence_keeps_provenance_and_hash() {
    let adapter = DbtManifestAdapter::load(fixture_path("fixtures/dbt/manifest.json"))
        .await
        .expect("load dbt manifest");
    let evidence = adapter
        .find_model("model.shop.mart_orders")
        .await
        .expect("find dbt model");

    assert_eq!(evidence.source_type, ContextSourceType::DbtManifest);
    assert!(evidence.version.starts_with("sha256:"));
    assert_eq!(evidence.version.len(), "sha256:".len() + 64);
    assert_eq!(evidence.instruction_trust, InstructionTrust::UntrustedData);
    assert_eq!(evidence.source, "dbt://model.shop.mart_orders");
}

#[tokio::test]
async fn dbt_relation_lookup_returns_project_evidence_not_observed_schema() {
    let adapter = DbtManifestAdapter::load(fixture_path("fixtures/dbt/manifest.json"))
        .await
        .expect("load dbt manifest");
    let evidence = adapter
        .load_evidence("mart_orders")
        .await
        .expect("search dbt manifest");

    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].source_type, ContextSourceType::DbtManifest);
    assert!(evidence[0].text.contains("model.shop.mart_orders"));
}
