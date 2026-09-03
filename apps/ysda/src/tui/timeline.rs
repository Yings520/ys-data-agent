use std::collections::VecDeque;

use ys_agent_core::{EventEnvelope, PolicyDecision, RunEventKind, RunSnapshot, RunStatus};
use ys_agent_runtime::{QueryArtifact, ServiceReply};

use super::navigation::FocusTarget;

pub const MAX_TIMELINE_STAGES: usize = 8;
const MAX_TIMELINE_WARNINGS: usize = 4;
const MAX_VISIBLE_TEXT_CHARS: usize = 240;
const SERVICE_REPLY_RANK: u8 = 1;
const EVENT_RANK: u8 = 2;
const RUNNING_SNAPSHOT_RANK: u8 = 3;
const TERMINAL_SNAPSHOT_RANK: u8 = 4;
const QUERY_ARTIFACT_RANK: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl HitRegion {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineState {
    pub scroll: usize,
    pub focus: FocusTarget,
    pub result_card_hit_region: Option<HitRegion>,
    question: Option<String>,
    question_rank: u8,
    status: TimelineStatus,
    status_rank: u8,
    stages: VecDeque<TimelineStage>,
    notice: Option<TimelineNotice>,
    notice_rank: u8,
    result_card: Option<TimelineResultCard>,
    last_event_sequence: Option<u64>,
    last_snapshot_version: Option<u64>,
    successful_primary_artifact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimelineStatus {
    #[default]
    Idle,
    Scheduled,
    Running,
    WaitingForInput,
    Denied,
    Failed,
    Cancelled,
    Succeeded,
}

impl TimelineStatus {
    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Denied | Self::Failed | Self::Cancelled | Self::Succeeded
        )
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Scheduled => "Scheduled",
            Self::Running => "Running",
            Self::WaitingForInput => "Waiting for input",
            Self::Denied => "Denied",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Succeeded => "Succeeded",
        }
    }

    pub const fn tone(self) -> TimelineTone {
        match self {
            Self::Idle | Self::Scheduled | Self::Running => TimelineTone::Neutral,
            Self::WaitingForInput | Self::Cancelled => TimelineTone::Warning,
            Self::Denied | Self::Failed => TimelineTone::Danger,
            Self::Succeeded => TimelineTone::Success,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineTone {
    Neutral,
    Warning,
    Danger,
    Success,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineStage {
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineNotice {
    pub reason: String,
    pub next_action: String,
    pub tone: TimelineTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineResultCard {
    pub answer_summary: String,
    pub verification: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct TimelineView<'a> {
    pub question: Option<&'a str>,
    pub status: TimelineStatus,
    pub stages: &'a VecDeque<TimelineStage>,
    pub notice: Option<&'a TimelineNotice>,
    pub result_card: Option<&'a TimelineResultCard>,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            scroll: 0,
            focus: FocusTarget::Composer,
            result_card_hit_region: None,
            question: None,
            question_rank: 0,
            status: TimelineStatus::Idle,
            status_rank: 0,
            stages: VecDeque::new(),
            notice: None,
            notice_rank: 0,
            result_card: None,
            last_event_sequence: None,
            last_snapshot_version: None,
            successful_primary_artifact: false,
        }
    }
}

pub fn render_lines(state: &TimelineState) -> Vec<String> {
    let mut lines = Vec::new();
    let view = state.view();
    if let Some(question) = view.question {
        lines.push(format!("You · {question}"));
    }
    lines.extend(view.stages.iter().map(|stage| format!("· {}", stage.label)));
    if view.status != TimelineStatus::Idle {
        lines.push(format!("Status · {}", view.status.label()));
    }
    if let Some(notice) = view.notice {
        lines.push(format!("Reason · {}", notice.reason));
        lines.push(format!("Next · {}", notice.next_action));
    }
    if let Some(card) = view.result_card {
        lines.push("Results".to_owned());
        lines.push(card.answer_summary.clone());
        lines.push(format!("Verification · {}", card.verification));
        lines.extend(
            card.warnings
                .iter()
                .map(|warning| format!("Warning · {warning}")),
        );
    }
    lines
}

impl TimelineState {
    pub fn focus_result_card(&mut self, hit_region: HitRegion) {
        self.focus = FocusTarget::TimelineResultCard;
        self.result_card_hit_region = Some(hit_region);
    }

    pub fn view(&self) -> TimelineView<'_> {
        TimelineView {
            question: self.question.as_deref(),
            status: self.status,
            stages: &self.stages,
            notice: self.notice.as_ref(),
            result_card: self.result_card.as_ref(),
        }
    }

    pub fn set_question(&mut self, question: impl AsRef<str>) {
        self.set_question_at_rank(question.as_ref(), SERVICE_REPLY_RANK);
    }

    pub fn begin_query(&mut self, question: impl AsRef<str>) {
        *self = Self::default();
        self.set_question(question);
    }

    pub fn apply_service_reply(&mut self, reply: &ServiceReply) {
        match reply {
            ServiceReply::Conversation { message } => self.set_notice(
                "Conversation response received",
                message,
                TimelineTone::Neutral,
                SERVICE_REPLY_RANK,
            ),
            ServiceReply::RunScheduled { .. } => {
                self.set_status(TimelineStatus::Scheduled, SERVICE_REPLY_RANK);
                self.set_status(TimelineStatus::Running, SERVICE_REPLY_RANK);
            }
            ServiceReply::ClarificationRequired { question, .. } => {
                self.set_status(TimelineStatus::WaitingForInput, SERVICE_REPLY_RANK);
                self.set_notice(
                    question,
                    "Answer the clarification to resume this Query",
                    TimelineTone::Warning,
                    SERVICE_REPLY_RANK,
                );
            }
            ServiceReply::UnsupportedCapability { workflow, .. } => {
                self.set_status(TimelineStatus::Denied, SERVICE_REPLY_RANK);
                self.set_notice(
                    &format!("{workflow:?} is not available"),
                    "Choose a supported Query workflow",
                    TimelineTone::Warning,
                    SERVICE_REPLY_RANK,
                );
            }
        }
    }

    /// Returns false only when the Event sequence was already observed. Accepted Events after a
    /// terminal fact still advance the cursor, but cannot downgrade the visible conclusion.
    pub fn apply_event(&mut self, envelope: &EventEnvelope) -> bool {
        if self
            .last_event_sequence
            .is_some_and(|sequence| envelope.sequence <= sequence)
        {
            return false;
        }
        self.last_event_sequence = Some(envelope.sequence);
        if self.status.is_terminal() && !is_terminal_event(&envelope.event.kind) {
            return true;
        }
        match &envelope.event.kind {
            RunEventKind::ProviderBound { .. } => self.push_stage("Provider bound"),
            RunEventKind::RunStarted | RunEventKind::RunResumed => {
                self.set_status(TimelineStatus::Running, EVENT_RANK)
            }
            RunEventKind::StepStarted { label, .. } => self.push_stage(label),
            RunEventKind::ModelRequested { .. } => self.push_stage("Preparing model request"),
            RunEventKind::ModelResponded { .. } => self.push_stage("Model response received"),
            RunEventKind::ToolCallProposed { call } => {
                self.push_stage(&format!("Checking {}", call.name))
            }
            RunEventKind::PolicyEvaluated { decision, .. } => match decision {
                PolicyDecision::Allow => self.push_stage("Policy check passed"),
                PolicyDecision::Deny { code, .. } => {
                    self.set_status(TimelineStatus::Denied, EVENT_RANK);
                    self.set_notice(
                        code,
                        "Review governed access and retry",
                        TimelineTone::Danger,
                        EVENT_RANK,
                    );
                }
                PolicyDecision::RequireConfirmation { code, .. } => {
                    self.set_status(TimelineStatus::WaitingForInput, EVENT_RANK);
                    self.set_notice(
                        code,
                        "Confirm the governed operation to continue",
                        TimelineTone::Warning,
                        EVENT_RANK,
                    );
                }
            },
            RunEventKind::ToolExecutionStarted { .. } => {
                self.push_stage("Governed operation running")
            }
            RunEventKind::ToolExecutionSucceeded { .. } => {
                self.push_stage("Governed operation completed")
            }
            RunEventKind::ToolExecutionFailed { failure, .. }
            | RunEventKind::ToolExecutionIndeterminate { failure, .. } => self.set_notice(
                &failure.code,
                "Review the operation outcome before retrying",
                TimelineTone::Warning,
                EVENT_RANK,
            ),
            RunEventKind::ArtifactCreated { .. } => self.push_stage("Artifact persisted"),
            RunEventKind::ClarificationRequested { question, .. } => {
                self.set_status(TimelineStatus::WaitingForInput, EVENT_RANK);
                self.set_notice(
                    question,
                    "Answer the clarification to resume this Query",
                    TimelineTone::Warning,
                    EVENT_RANK,
                );
            }
            RunEventKind::ClarificationAnswered { .. } => {
                self.set_status(TimelineStatus::Running, EVENT_RANK);
                self.push_stage("Clarification accepted");
            }
            RunEventKind::RunWaiting { reason } => {
                self.set_status(TimelineStatus::WaitingForInput, EVENT_RANK);
                self.set_notice(
                    reason,
                    "Provide the requested input to continue",
                    TimelineTone::Warning,
                    EVENT_RANK,
                );
            }
            RunEventKind::RunCompleted { .. } => {
                self.successful_primary_artifact = true;
                self.set_status(TimelineStatus::Succeeded, EVENT_RANK);
            }
            RunEventKind::RunFailed { code, .. } => {
                self.set_status(TimelineStatus::Failed, EVENT_RANK);
                self.set_notice(
                    code,
                    "Open diagnostics, correct the issue, and retry",
                    TimelineTone::Danger,
                    EVENT_RANK,
                );
            }
            RunEventKind::RunCancelled { reason } => {
                self.set_status(TimelineStatus::Cancelled, EVENT_RANK);
                self.set_notice(
                    reason,
                    "Start a new Query when ready",
                    TimelineTone::Warning,
                    EVENT_RANK,
                );
            }
            RunEventKind::RunStateProjected { snapshot } => {
                self.apply_snapshot(snapshot);
            }
        }
        true
    }

    pub fn apply_snapshot(&mut self, snapshot: &RunSnapshot) -> bool {
        if self
            .last_snapshot_version
            .is_some_and(|version| snapshot.version <= version)
        {
            return false;
        }
        self.last_snapshot_version = Some(snapshot.version);
        let rank = if matches!(
            snapshot.status,
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Cancelled
        ) {
            TERMINAL_SNAPSHOT_RANK
        } else {
            RUNNING_SNAPSHOT_RANK
        };
        if let Some(phase) = snapshot
            .workflow_state
            .get("phase")
            .and_then(serde_json::Value::as_str)
        {
            self.push_stage(phase);
        }
        match snapshot.status {
            RunStatus::Queued => self.set_status(TimelineStatus::Scheduled, rank),
            RunStatus::Running => self.set_status(TimelineStatus::Running, rank),
            RunStatus::WaitingForInput => {
                self.set_status(TimelineStatus::WaitingForInput, rank);
                let reason = snapshot
                    .pending_wait_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("question"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Additional input is required");
                self.set_notice(
                    reason,
                    "Provide the requested input to continue",
                    TimelineTone::Warning,
                    rank,
                );
            }
            RunStatus::Succeeded => {
                self.successful_primary_artifact = snapshot.primary_artifact_id.is_some();
                self.set_status(TimelineStatus::Succeeded, rank);
            }
            RunStatus::Failed => {
                self.successful_primary_artifact = false;
                let failure = snapshot.workflow_state.get("failure");
                let reason = failure
                    .and_then(|value| value.get("what_happened"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("The governed Query failed");
                let action = failure
                    .and_then(|value| value.get("required_user_action"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Open diagnostics, correct the issue, and retry");
                self.set_status(TimelineStatus::Failed, rank);
                self.set_notice(reason, action, TimelineTone::Danger, rank);
            }
            RunStatus::Cancelled => {
                self.successful_primary_artifact = false;
                self.set_status(TimelineStatus::Cancelled, rank);
                self.set_notice(
                    "The Query was cancelled",
                    "Start a new Query when ready",
                    TimelineTone::Warning,
                    rank,
                );
            }
        }
        true
    }

    pub fn apply_persisted_query_artifact(&mut self, artifact: &QueryArtifact) -> bool {
        if self.status != TimelineStatus::Succeeded
            || !self.successful_primary_artifact
            || !artifact.verification.hard_failures.is_empty()
        {
            return false;
        }
        self.set_question_at_rank(&artifact.question, QUERY_ARTIFACT_RANK);
        self.set_status(TimelineStatus::Succeeded, QUERY_ARTIFACT_RANK);
        let mut warnings = artifact
            .warning_codes
            .iter()
            .map(|warning| safe_text(warning))
            .filter(|warning| !warning.is_empty())
            .take(MAX_TIMELINE_WARNINGS)
            .collect::<Vec<_>>();
        warnings.sort();
        warnings.dedup();
        self.result_card = Some(TimelineResultCard {
            answer_summary: safe_text(&artifact.answer_summary),
            verification: format!("Verified · {:?}", artifact.semantic_status),
            warnings,
        });
        self.notice = None;
        self.notice_rank = QUERY_ARTIFACT_RANK;
        true
    }

    fn set_question_at_rank(&mut self, question: &str, rank: u8) {
        if rank < self.question_rank {
            return;
        }
        let question = safe_text(question);
        if !question.is_empty() {
            self.question = Some(question);
            self.question_rank = rank;
        }
    }

    fn set_status(&mut self, status: TimelineStatus, rank: u8) {
        if self.status.is_terminal() && !status.is_terminal() {
            return;
        }
        if rank >= self.status_rank {
            self.status = status;
            self.status_rank = rank;
        }
    }

    fn set_notice(&mut self, reason: &str, next_action: &str, tone: TimelineTone, rank: u8) {
        if rank < self.notice_rank {
            return;
        }
        self.notice = Some(TimelineNotice {
            reason: safe_text(reason),
            next_action: safe_text(next_action),
            tone,
        });
        self.notice_rank = rank;
    }

    fn push_stage(&mut self, label: &str) {
        let label = safe_text(label);
        if label.is_empty() || self.stages.back().is_some_and(|stage| stage.label == label) {
            return;
        }
        if self.stages.len() == MAX_TIMELINE_STAGES {
            self.stages.pop_front();
        }
        self.stages.push_back(TimelineStage { label });
    }
}

fn is_terminal_event(event: &RunEventKind) -> bool {
    matches!(
        event,
        RunEventKind::RunCompleted { .. }
            | RunEventKind::RunFailed { .. }
            | RunEventKind::RunCancelled { .. }
            | RunEventKind::RunStateProjected { .. }
    )
}

fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_VISIBLE_TEXT_CHARS)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use ys_agent_core::{
        ArtifactId, EventActor, EventEnvelope, EventId, PolicyDecision, QueryIntent,
        RetentionPolicy, RunEventKind, RunId, RunSnapshot, RunStatus, SemanticStatus, Sensitivity,
        SourceId, TaskId, ToolCallId, VersionedRunEvent, WorkflowKind, WorkspaceId,
    };
    use ys_agent_runtime::{QueryArtifact, ServiceReply, VerificationReport};

    use super::*;

    fn event(sequence: u64, kind: RunEventKind) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId::new(),
            workspace_id: WorkspaceId::new(),
            task_id: TaskId::new(),
            run_id: RunId::new(),
            sequence,
            occurred_at: Utc::now(),
            actor: EventActor::System,
            event: VersionedRunEvent::v1(kind),
        }
    }

    fn snapshot(
        status: RunStatus,
        version: u64,
        primary_artifact_id: Option<ArtifactId>,
    ) -> RunSnapshot {
        RunSnapshot {
            run_id: RunId::new(),
            task_id: TaskId::new(),
            workflow: WorkflowKind::Query,
            status,
            attempt: 1,
            retry_of_run_id: None,
            version,
            workflow_state: json!({
                "phase": "execute",
                "failure": {
                    "what_happened": "governed query failed",
                    "required_user_action": "review diagnostics"
                }
            }),
            pending_wait_metadata: Some(json!({ "question": "Choose a date range" })),
            primary_artifact_id,
            last_completed_step_id: None,
        }
    }

    fn query_artifact() -> QueryArtifact {
        QueryArtifact {
            question: "How many governed orders?".to_owned(),
            intent: QueryIntent::Metadata,
            answer_summary: "The governed catalog contains the requested order metric.".to_owned(),
            metric: None,
            semantic_status: SemanticStatus::Observed,
            source_id: SourceId::new("warehouse"),
            source_relations: Vec::new(),
            time_range: None,
            executed_sql: Some("SELECT secret_column FROM private_table".to_owned()),
            bound_parameters: Vec::new(),
            result_schema: Default::default(),
            result_artifact: None,
            freshness: None,
            verification: VerificationReport {
                checks: Vec::new(),
                hard_failures: Vec::new(),
                warnings: vec!["freshness_unknown".to_owned()],
                evidence_refs: Vec::new(),
            },
            assumptions: vec!["internal raw assumption".to_owned()],
            warning_codes: vec!["freshness_unknown".to_owned()],
            sensitivity: Sensitivity::Internal,
            retention_policy: RetentionPolicy::Session,
            expires_at: None,
            generated_at: Utc::now(),
        }
    }

    #[test]
    fn sequence_and_source_priority_prevent_terminal_state_downgrade() {
        let mut timeline = TimelineState::default();
        timeline.set_question("Which governed metric changed?");
        timeline.apply_service_reply(&ServiceReply::RunScheduled {
            task_id: TaskId::new(),
            run_id: RunId::new(),
        });
        assert_eq!(timeline.view().status, TimelineStatus::Running);
        assert!(timeline.apply_event(&event(
            2,
            RunEventKind::StepStarted {
                step_id: ys_agent_core::StepId::new(),
                label: "Compile governed query".to_owned(),
            }
        )));
        assert!(!timeline.apply_event(&event(1, RunEventKind::RunStarted)));

        timeline.apply_snapshot(&snapshot(RunStatus::Failed, 5, None));
        assert_eq!(timeline.view().status, TimelineStatus::Failed);
        assert!(timeline.apply_event(&event(3, RunEventKind::RunStarted)));
        timeline.apply_snapshot(&snapshot(RunStatus::Running, 4, None));

        let view = timeline.view();
        assert_eq!(view.status, TimelineStatus::Failed);
        assert_eq!(view.stages.len(), 2);
        assert!(
            view.stages
                .iter()
                .any(|stage| stage.label == "Compile governed query")
        );
        assert_eq!(
            view.notice.expect("failure notice").next_action,
            "review diagnostics"
        );
    }

    #[test]
    fn non_success_outcomes_have_reason_and_next_action_without_success_semantics() {
        let cases = [
            (
                RunEventKind::RunWaiting {
                    reason: "Need a governed date range".to_owned(),
                },
                TimelineStatus::WaitingForInput,
            ),
            (
                RunEventKind::PolicyEvaluated {
                    call_id: ToolCallId::new(),
                    decision: PolicyDecision::Deny {
                        code: "policy.read_denied".to_owned(),
                        message: "raw provider detail must not render".to_owned(),
                    },
                },
                TimelineStatus::Denied,
            ),
            (
                RunEventKind::RunFailed {
                    code: "query.execution_failed".to_owned(),
                    message: "raw transport body must not render".to_owned(),
                },
                TimelineStatus::Failed,
            ),
            (
                RunEventKind::RunCancelled {
                    reason: "Cancelled by operator".to_owned(),
                },
                TimelineStatus::Cancelled,
            ),
        ];

        for (kind, expected) in cases {
            let mut timeline = TimelineState::default();
            timeline.apply_event(&event(1, kind));
            let view = timeline.view();
            assert_eq!(view.status, expected);
            let notice = view.notice.expect("non-success notice");
            assert!(!notice.reason.is_empty());
            assert!(!notice.next_action.is_empty());
            assert_ne!(notice.tone, TimelineTone::Success);
            let rendered = render_lines(&timeline).join("\n");
            assert!(!rendered.contains("verified"));
            assert!(!rendered.contains("raw transport body"));
            assert!(!rendered.contains("raw provider detail"));
        }
    }

    #[test]
    fn only_success_with_a_primary_persisted_query_artifact_creates_a_safe_result_card() {
        let artifact = query_artifact();
        let mut timeline = TimelineState::default();
        timeline.apply_snapshot(&snapshot(RunStatus::Succeeded, 1, None));
        assert!(!timeline.apply_persisted_query_artifact(&artifact));
        assert!(timeline.view().result_card.is_none());

        timeline.apply_snapshot(&snapshot(RunStatus::Succeeded, 2, Some(ArtifactId::new())));
        assert!(timeline.apply_persisted_query_artifact(&artifact));
        let view = timeline.view();
        assert_eq!(view.status, TimelineStatus::Succeeded);
        let card = view.result_card.expect("result card");
        assert_eq!(card.verification, "Verified · Observed");
        assert_eq!(card.warnings, ["freshness_unknown"]);
        let rendered = render_lines(&timeline).join("\n");
        assert!(rendered.contains("How many governed orders?"));
        assert!(rendered.contains("The governed catalog contains"));
        assert!(!rendered.contains("SELECT secret_column"));
        assert!(!rendered.contains("private_table"));
        assert!(!rendered.contains("internal raw assumption"));
        assert!(!rendered.contains(&snapshot(RunStatus::Succeeded, 2, None).run_id.to_string()));
    }

    #[test]
    fn projected_stage_and_warning_state_is_bounded_and_sanitized() {
        let mut timeline = TimelineState::default();
        for sequence in 1..=20 {
            timeline.apply_event(&event(
                sequence,
                RunEventKind::StepStarted {
                    step_id: ys_agent_core::StepId::new(),
                    label: format!("stage-{sequence}\nsecret-control"),
                },
            ));
        }
        assert_eq!(timeline.view().stages.len(), MAX_TIMELINE_STAGES);
        assert!(
            timeline
                .view()
                .stages
                .iter()
                .all(|stage| !stage.label.contains('\n'))
        );
    }

    #[test]
    fn a_later_terminal_event_may_refine_but_never_resume_a_terminal_outcome() {
        let mut timeline = TimelineState::default();
        timeline.apply_event(&event(
            1,
            RunEventKind::PolicyEvaluated {
                call_id: ToolCallId::new(),
                decision: PolicyDecision::Deny {
                    code: "policy.denied".to_owned(),
                    message: "not rendered".to_owned(),
                },
            },
        ));
        timeline.apply_event(&event(
            2,
            RunEventKind::RunFailed {
                code: "query.denied".to_owned(),
                message: "not rendered".to_owned(),
            },
        ));
        timeline.apply_event(&event(3, RunEventKind::RunStarted));

        assert_eq!(timeline.view().status, TimelineStatus::Failed);
        assert_eq!(timeline.view().status.tone(), TimelineTone::Danger);
    }

    #[test]
    fn beginning_a_new_query_resets_bounded_run_cursors() {
        let mut timeline = TimelineState::default();
        timeline.begin_query("first question");
        assert!(timeline.apply_event(&event(4, RunEventKind::RunStarted)));
        timeline.begin_query("second question");

        assert!(timeline.apply_event(&event(1, RunEventKind::RunStarted)));
        assert_eq!(timeline.view().question, Some("second question"));
        assert_eq!(timeline.view().status, TimelineStatus::Running);
        assert!(timeline.view().stages.is_empty());
    }
}
