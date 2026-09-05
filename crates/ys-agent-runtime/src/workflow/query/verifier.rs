use serde::{Deserialize, Serialize};
use ys_agent_core::{
    ArtifactId, ArtifactRef, DatasourceDigest, PolicyDecision, QueryIntent, SemanticStatus,
    TimeRange,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub code: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub checks: Vec<VerificationCheck>,
    pub hard_failures: Vec<String>,
    pub warnings: Vec<String>,
    pub evidence_refs: Vec<ArtifactRef>,
    #[serde(default)]
    pub datasource_binding: Option<DatasourceDigest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessState {
    Fresh,
    Stale,
    Unknown,
    NotRequired,
}

#[derive(Debug, Clone)]
pub struct VerificationInput {
    pub datasource_binding: Option<DatasourceDigest>,
    pub intent: QueryIntent,
    pub policy_decision: Option<PolicyDecision>,
    pub data_query_permission_present: bool,
    pub source_scope_matches: bool,
    pub field_scope_matches: bool,
    pub query_budget_passed: bool,
    pub result_policy_passed: bool,
    pub claims_reference_authorized_evidence: bool,
    pub artifact_metadata_complete: bool,
    pub executed_result: Option<ArtifactRef>,
    pub requested_time_range: Option<TimeRange>,
    pub compiled_time_range: Option<TimeRange>,
    pub requested_metric: Option<(String, String)>,
    pub compiled_metric: Option<(String, String)>,
    pub relation_matches: bool,
    pub result_schema_matches: bool,
    pub freshness_evidence: Option<ArtifactRef>,
    pub freshness_state: FreshnessState,
    pub current_data_required: bool,
    pub assumption_refs: Vec<ArtifactId>,
    pub ast_policy_passed: bool,
    pub semantic_status: SemanticStatus,
    pub observed_metadata: Vec<ArtifactRef>,
    pub invented_metric: bool,
    pub invented_sql_result: bool,
    pub invented_business_conclusion: bool,
    pub result_truncated: bool,
    pub result_empty: bool,
    pub result_all_null: bool,
    pub unconfirmed_assumptions: bool,
    pub sensitive_columns_redacted: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QueryVerifier;

impl QueryVerifier {
    pub fn verify(&self, input: VerificationInput) -> VerificationReport {
        let mut report = VerificationReport {
            checks: Vec::new(),
            hard_failures: Vec::new(),
            warnings: Vec::new(),
            evidence_refs: Vec::new(),
            datasource_binding: input.datasource_binding.clone(),
        };

        hard_check(
            &mut report,
            "policy_allowed",
            matches!(input.policy_decision, Some(PolicyDecision::Allow)),
            "PolicyDecision must be Allow",
        );
        hard_check(
            &mut report,
            "data_query_permission",
            input.data_query_permission_present,
            "DataQuery permission must be present",
        );
        hard_check(
            &mut report,
            "source_scope",
            input.source_scope_matches,
            "Source must match AllowedDataScope",
        );
        hard_check(
            &mut report,
            "field_scope",
            input.field_scope_matches,
            "Fields must match AllowedDataScope",
        );
        hard_check(
            &mut report,
            "query_budget",
            input.query_budget_passed,
            "QueryBudget must pass",
        );
        hard_check(
            &mut report,
            "result_policy",
            input.result_policy_passed,
            "ResultPolicy must pass",
        );
        hard_check(
            &mut report,
            "claim_evidence",
            input.claims_reference_authorized_evidence,
            "Every answer claim needs authorized Evidence",
        );
        hard_check(
            &mut report,
            "artifact_metadata",
            input.artifact_metadata_complete,
            "Sensitivity and retention metadata must be complete",
        );

        match input.intent {
            QueryIntent::GovernedMetric => verify_metric(&mut report, &input),
            QueryIntent::AdHocRead => verify_adhoc(&mut report, &input),
            QueryIntent::Metadata => verify_metadata(&mut report, &input),
        }
        add_warnings(&mut report, &input);
        deduplicate(&mut report.hard_failures);
        deduplicate(&mut report.warnings);
        deduplicate_evidence(&mut report.evidence_refs);
        report
    }
}

fn hard_check(
    report: &mut VerificationReport,
    code: &'static str,
    passed: bool,
    detail: &'static str,
) {
    report.checks.push(VerificationCheck {
        code: code.to_owned(),
        passed,
        detail: detail.to_owned(),
    });
    if !passed {
        report.hard_failures.push(code.to_owned());
    }
}

fn verify_metric(report: &mut VerificationReport, input: &VerificationInput) {
    hard_check(
        report,
        "metric_result_evidence",
        input.executed_result.is_some(),
        "GovernedMetric needs executed result Evidence",
    );
    hard_check(
        report,
        "metric_time_range",
        input.requested_time_range.is_some()
            && input.requested_time_range == input.compiled_time_range,
        "Requested and compiled half-open time ranges must match",
    );
    hard_check(
        report,
        "metric_contract_identity",
        input.requested_metric.is_some() && input.requested_metric == input.compiled_metric,
        "Active metric id and version must match compilation Evidence",
    );
    hard_check(
        report,
        "metric_relation",
        input.relation_matches,
        "Metric relation must equal the contract relation",
    );
    hard_check(
        report,
        "metric_result_schema",
        input.result_schema_matches,
        "Result schema must match execution Evidence",
    );
    hard_check(
        report,
        "metric_freshness_evidence",
        input.freshness_evidence.is_some(),
        "GovernedMetric needs freshness Evidence",
    );
    hard_check(
        report,
        "metric_semantic_status",
        input.semantic_status == SemanticStatus::Confirmed,
        "GovernedMetric semantic status must be confirmed",
    );
    hard_check(
        report,
        "required_freshness_known",
        !input.current_data_required || input.freshness_state != FreshnessState::Unknown,
        "Current/latest/SLA questions require known freshness",
    );

    if let Some(result) = &input.executed_result {
        report.evidence_refs.push(result.clone());
    }
    if let Some(freshness) = &input.freshness_evidence {
        report.evidence_refs.push(freshness.clone());
    }
}

fn verify_adhoc(report: &mut VerificationReport, input: &VerificationInput) {
    hard_check(
        report,
        "adhoc_result_evidence",
        input.executed_result.is_some(),
        "AdHocRead needs executed result Evidence",
    );
    hard_check(
        report,
        "adhoc_assumptions",
        !input.assumption_refs.is_empty(),
        "AdHocRead needs durable assumption references",
    );
    hard_check(
        report,
        "adhoc_ast_policy",
        input.ast_policy_passed,
        "AdHoc SQL needs AST read-only policy Evidence",
    );
    hard_check(
        report,
        "adhoc_result_schema",
        input.result_schema_matches,
        "AdHoc result schema must match execution Evidence",
    );
    hard_check(
        report,
        "adhoc_semantic_status",
        input.semantic_status == SemanticStatus::Inferred,
        "AdHoc semantic status must be inferred",
    );

    if let Some(result) = &input.executed_result {
        report.evidence_refs.push(result.clone());
    }
}

fn verify_metadata(report: &mut VerificationReport, input: &VerificationInput) {
    hard_check(
        report,
        "metadata_observed_evidence",
        !input.observed_metadata.is_empty() || input.freshness_evidence.is_some(),
        "Metadata needs authorized observed schema or freshness Evidence",
    );
    hard_check(
        report,
        "metadata_no_invented_metric",
        !input.invented_metric,
        "Metadata must not invent a metric contract",
    );
    hard_check(
        report,
        "metadata_no_invented_sql_result",
        !input.invented_sql_result && input.executed_result.is_none(),
        "Metadata must not invent or claim a SQL result",
    );
    hard_check(
        report,
        "metadata_no_business_conclusion",
        !input.invented_business_conclusion,
        "Metadata must not invent a business conclusion",
    );
    hard_check(
        report,
        "metadata_semantic_status",
        input.semantic_status == SemanticStatus::Observed,
        "Metadata semantic status must be observed",
    );

    report
        .evidence_refs
        .extend(input.observed_metadata.iter().cloned());
    if let Some(freshness) = &input.freshness_evidence {
        report.evidence_refs.push(freshness.clone());
    }
}

fn add_warnings(report: &mut VerificationReport, input: &VerificationInput) {
    if input.freshness_state == FreshnessState::Stale {
        report.warnings.push("freshness_sla_failed".to_owned());
    }
    if input.intent == QueryIntent::AdHocRead {
        report.warnings.push("semantic_status_inferred".to_owned());
    }
    if input.result_truncated {
        report.warnings.push("result_truncated".to_owned());
    }
    if input.result_empty {
        report.warnings.push("empty_result".to_owned());
    }
    if input.result_all_null {
        report.warnings.push("all_null_result".to_owned());
    }
    if input.unconfirmed_assumptions {
        report.warnings.push("unconfirmed_assumptions".to_owned());
    }
    if input.freshness_state == FreshnessState::Unknown && !input.current_data_required {
        report.warnings.push("freshness_unknown".to_owned());
    }
    if input.sensitive_columns_redacted {
        report
            .warnings
            .push("sensitive_columns_redacted_or_local_only".to_owned());
    }
}

fn deduplicate(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn deduplicate_evidence(values: &mut Vec<ArtifactRef>) {
    values.sort_by_key(|artifact| artifact.id().to_string());
    values.dedup_by_key(|artifact| artifact.id());
}

#[cfg(test)]
mod tests {
    use ys_agent_core::{PolicyDecision, QueryIntent, SemanticStatus};

    use super::{FreshnessState, QueryVerifier, VerificationInput};

    fn common(intent: QueryIntent) -> VerificationInput {
        VerificationInput {
            datasource_binding: None,
            intent,
            policy_decision: Some(PolicyDecision::Allow),
            data_query_permission_present: true,
            source_scope_matches: true,
            field_scope_matches: true,
            query_budget_passed: true,
            result_policy_passed: true,
            claims_reference_authorized_evidence: true,
            artifact_metadata_complete: true,
            executed_result: None,
            requested_time_range: None,
            compiled_time_range: None,
            requested_metric: None,
            compiled_metric: None,
            relation_matches: true,
            result_schema_matches: true,
            freshness_evidence: None,
            freshness_state: FreshnessState::NotRequired,
            current_data_required: false,
            assumption_refs: Vec::new(),
            ast_policy_passed: true,
            semantic_status: SemanticStatus::Observed,
            observed_metadata: Vec::new(),
            invented_metric: false,
            invented_sql_result: false,
            invented_business_conclusion: false,
            result_truncated: false,
            result_empty: false,
            result_all_null: false,
            unconfirmed_assumptions: false,
            sensitive_columns_redacted: false,
        }
    }

    #[test]
    fn premature_metric_completion_has_hard_failures() {
        let mut input = common(QueryIntent::GovernedMetric);
        input.semantic_status = SemanticStatus::Confirmed;
        let report = QueryVerifier.verify(input);

        assert!(
            report
                .hard_failures
                .contains(&"metric_result_evidence".to_owned())
        );
        assert!(
            report
                .hard_failures
                .contains(&"metric_contract_identity".to_owned())
        );
    }

    #[test]
    fn empty_is_a_warning_not_a_synthetic_value() {
        let mut input = common(QueryIntent::Metadata);
        input.result_empty = true;
        let report = QueryVerifier.verify(input);

        assert!(report.warnings.contains(&"empty_result".to_owned()));
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.contains("zero"))
        );
    }
}
