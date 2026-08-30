use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ys_agent_core::{
    ArtifactRef, CoreError, CoreResult, FreshnessObservation, QueryIntent, QueryParameter,
    RetentionPolicy, SemanticStatus, Sensitivity, SourceId, TimeRange,
};

use super::VerificationReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricReference {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    Timestamp,
    Text,
    Integer,
    Real,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedParameter {
    pub kind: ParameterKind,
    pub display: String,
}

impl From<&QueryParameter> for RedactedParameter {
    fn from(parameter: &QueryParameter) -> Self {
        match parameter {
            QueryParameter::Timestamp(value) => Self {
                kind: ParameterKind::Timestamp,
                display: value.to_rfc3339(),
            },
            QueryParameter::Text(_) => Self {
                kind: ParameterKind::Text,
                display: "[REDACTED]".to_owned(),
            },
            QueryParameter::Integer(_) => Self {
                kind: ParameterKind::Integer,
                display: "[REDACTED]".to_owned(),
            },
            QueryParameter::Real(_) => Self {
                kind: ParameterKind::Real,
                display: "[REDACTED]".to_owned(),
            },
            QueryParameter::Boolean(_) => Self {
                kind: ParameterKind::Boolean,
                display: "[REDACTED]".to_owned(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultColumn {
    pub name: String,
    pub data_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultSchema {
    pub columns: Vec<ResultColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryArtifact {
    pub question: String,
    pub intent: QueryIntent,
    pub answer_summary: String,
    pub metric: Option<MetricReference>,
    pub semantic_status: SemanticStatus,
    pub source_id: SourceId,
    pub source_relations: Vec<String>,
    pub time_range: Option<TimeRange>,
    pub executed_sql: Option<String>,
    pub bound_parameters: Vec<RedactedParameter>,
    pub result_schema: ResultSchema,
    pub result_artifact: Option<ArtifactRef>,
    pub freshness: Option<FreshnessObservation>,
    pub verification: VerificationReport,
    pub assumptions: Vec<String>,
    pub warning_codes: Vec<String>,
    pub sensitivity: Sensitivity,
    pub retention_policy: RetentionPolicy,
    pub expires_at: Option<DateTime<Utc>>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct QueryArtifactInput {
    pub question: String,
    pub intent: QueryIntent,
    pub answer_summary: String,
    pub metric: Option<MetricReference>,
    pub semantic_status: SemanticStatus,
    pub source_id: SourceId,
    pub source_relations: Vec<String>,
    pub time_range: Option<TimeRange>,
    pub executed_sql: Option<String>,
    pub parameters: Vec<QueryParameter>,
    pub result_schema: ResultSchema,
    pub result_artifact: Option<ArtifactRef>,
    pub freshness: Option<FreshnessObservation>,
    pub verification: VerificationReport,
    pub assumptions: Vec<String>,
    pub sensitivity: Sensitivity,
    pub retention_policy: RetentionPolicy,
    pub expires_at: Option<DateTime<Utc>>,
    pub generated_at: DateTime<Utc>,
}

impl QueryArtifact {
    pub fn package(input: QueryArtifactInput) -> CoreResult<Self> {
        if !input.verification.hard_failures.is_empty() {
            return Err(CoreError::validation(
                "completion_gate_failed",
                format!(
                    "Query verification has hard failures: {}",
                    input.verification.hard_failures.join(",")
                ),
            ));
        }
        if input.question.trim().is_empty() || input.answer_summary.trim().is_empty() {
            return Err(CoreError::validation(
                "query_artifact_text_missing",
                "QueryArtifact needs a question and answer summary",
            ));
        }
        match input.intent {
            QueryIntent::GovernedMetric => {
                if input.metric.is_none()
                    || input.executed_sql.is_none()
                    || input.result_artifact.is_none()
                    || input.freshness.is_none()
                    || input.semantic_status != SemanticStatus::Confirmed
                {
                    return Err(CoreError::validation(
                        "incomplete_metric_artifact",
                        "GovernedMetric Artifact is missing contract, execution, or freshness data",
                    ));
                }
            }
            QueryIntent::AdHocRead => {
                if input.executed_sql.is_none()
                    || input.result_artifact.is_none()
                    || input.semantic_status != SemanticStatus::Inferred
                {
                    return Err(CoreError::validation(
                        "incomplete_adhoc_artifact",
                        "AdHocRead Artifact is missing execution data or inferred status",
                    ));
                }
            }
            QueryIntent::Metadata => {
                if input.executed_sql.is_some()
                    || input.result_artifact.is_some()
                    || input.semantic_status != SemanticStatus::Observed
                {
                    return Err(CoreError::validation(
                        "invalid_metadata_artifact",
                        "Metadata Artifact cannot claim query execution",
                    ));
                }
            }
        }
        if input.sensitivity == Sensitivity::Restricted
            && (input.expires_at.is_none()
                || matches!(input.retention_policy, RetentionPolicy::Session))
        {
            return Err(CoreError::validation(
                "restricted_query_retention_missing",
                "Restricted QueryArtifact needs explicit expiry and non-session retention",
            ));
        }

        let mut warning_codes = input.verification.warnings.clone();
        warning_codes.sort();
        warning_codes.dedup();
        if warning_codes.iter().any(|code| code == "empty_result")
            && summary_claims_zero(&input.answer_summary)
        {
            return Err(CoreError::validation(
                "empty_result_reported_as_zero",
                "An empty result cannot be summarized as numeric zero",
            ));
        }

        Ok(Self {
            question: input.question,
            intent: input.intent,
            answer_summary: input.answer_summary,
            metric: input.metric,
            semantic_status: input.semantic_status,
            source_id: input.source_id,
            source_relations: input.source_relations,
            time_range: input.time_range,
            executed_sql: input.executed_sql,
            bound_parameters: input
                .parameters
                .iter()
                .map(RedactedParameter::from)
                .collect(),
            result_schema: input.result_schema,
            result_artifact: input.result_artifact,
            freshness: input.freshness,
            verification: input.verification,
            assumptions: input.assumptions,
            warning_codes,
            sensitivity: input.sensitivity,
            retention_policy: input.retention_policy,
            expires_at: input.expires_at,
            generated_at: input.generated_at,
        })
    }
}

fn summary_claims_zero(summary: &str) -> bool {
    let normalized = summary.to_ascii_lowercase();
    [" is 0", " was 0", " equals 0", ": 0"]
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use ys_agent_core::{QueryIntent, RetentionPolicy, SemanticStatus, Sensitivity, SourceId};

    use super::{QueryArtifact, QueryArtifactInput, ResultSchema};
    use crate::workflow::query::VerificationReport;

    #[test]
    fn empty_result_cannot_be_packaged_as_zero() {
        let error = QueryArtifact::package(QueryArtifactInput {
            question: "What columns exist?".to_owned(),
            intent: QueryIntent::Metadata,
            answer_summary: "Column count is 0".to_owned(),
            metric: None,
            semantic_status: SemanticStatus::Observed,
            source_id: SourceId::new("sqlite-demo"),
            source_relations: vec!["mart_orders".to_owned()],
            time_range: None,
            executed_sql: None,
            parameters: Vec::new(),
            result_schema: ResultSchema::default(),
            result_artifact: None,
            freshness: None,
            verification: VerificationReport {
                checks: Vec::new(),
                hard_failures: Vec::new(),
                warnings: vec!["empty_result".to_owned()],
                evidence_refs: Vec::new(),
            },
            assumptions: Vec::new(),
            sensitivity: Sensitivity::Internal,
            retention_policy: RetentionPolicy::Session,
            expires_at: None,
            generated_at: Utc::now(),
        })
        .unwrap_err();

        assert_eq!(error.code(), "empty_result_reported_as_zero");
    }
}
