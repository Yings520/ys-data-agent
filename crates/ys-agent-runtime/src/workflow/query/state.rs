use serde::{Deserialize, Serialize};
use serde_json::Value;
use ys_agent_core::{ArtifactRef, CoreError, CoreResult, PolicyDecision, QueryIntent};

use crate::tools::QueryPhase;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationNeed {
    pub id: String,
    pub question: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryWorkflowState {
    pub phase: QueryPhase,
    pub question: String,
    pub intent: Option<QueryIntent>,
    pub metric_evidence: Option<ArtifactRef>,
    pub schema_evidence: Vec<ArtifactRef>,
    pub freshness_evidence: Option<ArtifactRef>,
    pub execution_plan: Option<ArtifactRef>,
    pub policy_decision: Option<PolicyDecision>,
    pub preflight: Option<ArtifactRef>,
    pub query_result: Option<ArtifactRef>,
    pub verification_report: Option<ArtifactRef>,
    pub assumptions: Vec<String>,
    pub warnings: Vec<String>,
    pub pending_clarification: Option<ClarificationNeed>,
    pub last_tool_output: Option<Value>,
}

impl QueryWorkflowState {
    pub fn new(question: impl Into<String>) -> CoreResult<Self> {
        let question = question.into();
        if question.trim().is_empty() {
            return Err(CoreError::validation(
                "empty_query_question",
                "Query Workflow needs a non-empty question",
            ));
        }
        Ok(Self {
            phase: QueryPhase::Clarify,
            question,
            intent: None,
            metric_evidence: None,
            schema_evidence: Vec::new(),
            freshness_evidence: None,
            execution_plan: None,
            policy_decision: None,
            preflight: None,
            query_result: None,
            verification_report: None,
            assumptions: Vec::new(),
            warnings: Vec::new(),
            pending_clarification: None,
            last_tool_output: None,
        })
    }

    pub fn from_snapshot(value: Value) -> CoreResult<Self> {
        serde_json::from_value(value)
            .map_err(|error| CoreError::validation("invalid_query_snapshot", error.to_string()))
    }

    pub fn to_snapshot(&self) -> CoreResult<Value> {
        serde_json::to_value(self).map_err(|error| {
            CoreError::validation("query_snapshot_serialization_failed", error.to_string())
        })
    }

    pub fn transition(&mut self, next: QueryPhase) -> CoreResult<()> {
        if !legal_transition(self.phase, next, self.intent) {
            return Err(CoreError::validation(
                "invalid_query_phase_transition",
                format!("cannot move from {:?} to {next:?}", self.phase),
            ));
        }
        self.validate_entry(next)?;
        self.phase = next;
        self.last_tool_output = None;
        Ok(())
    }

    pub fn return_to_plan(&mut self, warning: impl Into<String>) -> CoreResult<()> {
        if !matches!(
            self.phase,
            QueryPhase::ValidateAndPreflight | QueryPhase::Execute
        ) {
            return Err(CoreError::validation(
                "query_repair_not_allowed",
                "Only validation or execution failures may return to Plan",
            ));
        }
        self.phase = QueryPhase::Plan;
        self.execution_plan = None;
        self.preflight = None;
        self.query_result = None;
        self.verification_report = None;
        self.warnings.push(warning.into());
        self.last_tool_output = None;
        Ok(())
    }

    fn validate_entry(&self, next: QueryPhase) -> CoreResult<()> {
        match next {
            QueryPhase::Clarify | QueryPhase::ClassifyIntent => Ok(()),
            QueryPhase::ResolveContext => require(self.intent.is_some(), "query_intent_missing"),
            QueryPhase::Plan => match self.intent {
                Some(QueryIntent::GovernedMetric) => {
                    require(self.metric_evidence.is_some(), "metric_evidence_missing")
                }
                Some(QueryIntent::AdHocRead) => {
                    require(!self.schema_evidence.is_empty(), "schema_evidence_missing")
                }
                Some(QueryIntent::Metadata) => Err(CoreError::validation(
                    "metadata_plan_forbidden",
                    "Metadata does not fabricate an execution plan",
                )),
                None => require(false, "query_intent_missing"),
            },
            QueryPhase::ValidateAndPreflight => {
                require(self.execution_plan.is_some(), "query_plan_missing")
            }
            QueryPhase::Execute => require(self.preflight.is_some(), "query_preflight_missing"),
            QueryPhase::Verify => match self.intent {
                Some(QueryIntent::Metadata) => require(
                    !self.schema_evidence.is_empty(),
                    "metadata_evidence_missing",
                ),
                Some(QueryIntent::GovernedMetric | QueryIntent::AdHocRead) => {
                    require(self.query_result.is_some(), "query_result_missing")
                }
                None => require(false, "query_intent_missing"),
            },
            QueryPhase::Package => require(
                self.verification_report.is_some(),
                "verification_report_missing",
            ),
            QueryPhase::ReadyToComplete => require(
                self.verification_report.is_some(),
                "verification_report_missing",
            ),
        }
    }
}

fn require(condition: bool, code: &'static str) -> CoreResult<()> {
    if condition {
        Ok(())
    } else {
        Err(CoreError::validation(
            code,
            "required Query evidence is missing",
        ))
    }
}

fn legal_transition(current: QueryPhase, next: QueryPhase, intent: Option<QueryIntent>) -> bool {
    matches!(
        (current, next),
        (QueryPhase::Clarify, QueryPhase::ClassifyIntent)
            | (QueryPhase::ClassifyIntent, QueryPhase::ResolveContext)
            | (QueryPhase::ResolveContext, QueryPhase::Plan)
            | (QueryPhase::Plan, QueryPhase::ValidateAndPreflight)
            | (QueryPhase::ValidateAndPreflight, QueryPhase::Execute)
            | (QueryPhase::Execute, QueryPhase::Verify)
            | (QueryPhase::Verify, QueryPhase::Package)
            | (QueryPhase::Package, QueryPhase::ReadyToComplete)
    ) || (current == QueryPhase::ResolveContext
        && next == QueryPhase::Verify
        && intent == Some(QueryIntent::Metadata))
}

pub fn material_ambiguity(question: &str) -> Option<ClarificationNeed> {
    let lower = question.to_ascii_lowercase();
    let ambiguous_time = ["recently", "lately", "last period"]
        .iter()
        .any(|phrase| lower.contains(phrase));
    if ambiguous_time {
        return Some(ClarificationNeed {
            id: "query-time-range-v1".to_owned(),
            question: "Which exact time range and timezone should I use?".to_owned(),
            reason: "material_query_ambiguity".to_owned(),
        });
    }
    if lower.contains("gmv") && lower.contains("by region") && lower.contains("or market") {
        return Some(ClarificationNeed {
            id: "query-dimension-v1".to_owned(),
            question: "Should the governed dimension be region or market?".to_owned(),
            reason: "material_query_ambiguity".to_owned(),
        });
    }
    None
}

pub fn classify_intent(question: &str) -> QueryIntent {
    let lower = question.to_ascii_lowercase();
    if ["what columns", "schema", "freshness", "latest data"]
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        QueryIntent::Metadata
    } else if ["gmv", "revenue", "orders", "conversion rate"]
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        QueryIntent::GovernedMetric
    } else {
        QueryIntent::AdHocRead
    }
}

pub fn requires_current_freshness(question: &str) -> bool {
    let lower = question.to_ascii_lowercase();
    ["current", "latest", "right now", "sla"]
        .iter()
        .any(|phrase| lower.contains(phrase))
}
