use std::sync::Arc;

use async_trait::async_trait;
use ys_agent_core::CoreResult;
use ys_agent_runtime::doctor::{
    DoctorInputs, DoctorProbe, DoctorRunner, ModelReadiness, QueryCapability, SourceReadiness,
    WorkspaceDoctor,
};

struct MissingMetricAndUnsafeRole;

#[async_trait]
impl DoctorProbe for MissingMetricAndUnsafeRole {
    async fn inspect(&self) -> CoreResult<DoctorInputs> {
        Ok(DoctorInputs {
            missing_config_keys: Vec::new(),
            model: ModelReadiness {
                reachable: true,
                supports_tool_calls: true,
                supports_tool_call_ids: true,
                supports_multi_turn_tool_results: true,
                context_limit: Some(32_000),
            },
            source: SourceReadiness {
                reachable: true,
                query_capability: true,
                catalog_capability: true,
                freshness_capability: true,
                database_read_only: false,
            },
            query_policy_valid: true,
            metric_registry_valid: false,
            dbt_manifest_valid: None,
            timezone_explicit: true,
            freshness_rules_explicit: true,
            query_budget_explicit: true,
            artifact_directory_private_and_writable: true,
            export_directory_private_and_writable: true,
        })
    }
}

#[tokio::test]
async fn doctor_reports_blockers_warnings_and_repairs_without_secrets() {
    let doctor = WorkspaceDoctor::new(Arc::new(MissingMetricAndUnsafeRole));
    let report = doctor.run().await.expect("doctor report");

    assert!(report.has_blocker("database_not_read_only"));
    assert!(report.has_warning("metric_registry_missing"));
    assert!(
        report
            .repairs
            .iter()
            .any(|item| item.contains("read-only role"))
    );
    assert!(!report.to_string().contains("canary-password"));
    assert!(
        !report
            .ready_capabilities
            .contains(&QueryCapability::GovernedMetric)
    );
}
