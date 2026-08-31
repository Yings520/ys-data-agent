use std::sync::Arc;

use tokio::task::JoinHandle;

use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactId, CommandId, EventEnvelope,
    ExportFormat, Principal, RunEventKind, RunId, RunStatus, Sensitivity, SessionId, StepId,
    TaskId, WorkspaceId,
};
use ys_agent_runtime::{
    AgentServiceApi, CreateTaskRequest, EventSubscription, QueryArtifact, SendMessageRequest,
    ServiceReply, doctor::DoctorReport, export::PersistedResultBody,
};

use super::input::{DetailRequest, InputAction};
use super::{
    composer::ComposerState,
    palette::SlashPalette,
    theme::{ThemeRegistry, UiPreferences, YsdaTheme},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BaseView {
    #[default]
    Home,
    Clarification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Metrics,
    Query,
    Checks,
    Artifact,
    Sql,
    Diagnostics,
    Tasks,
    Connections,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientView {
    SlashPalette,
    ThemePicker,
    Detail(DetailKind),
    Help,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub goal: String,
    pub status: String,
    pub needs_input: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerView {
    pub state: String,
    pub conclusion: String,
    pub key_values: [Option<String>; 2],
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    UserMessage(String),
    Answer(AnswerView),
    Clarification {
        question: String,
        recommended_default: Option<String>,
    },
    Warning(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetailView {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiagnosticsView {
    pub session_id: Option<SessionId>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub step_id: Option<StepId>,
    pub query_phase: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TuiApp {
    pub workspace_name: String,
    pub principal_name: String,
    pub model_label: String,
    pub connection_label: String,
    pub permission_label: String,
    pub doctor_report: Option<DoctorReport>,
    pub focused_task: Option<TaskSummary>,
    pub transcript: Vec<TranscriptItem>,
    pub runtime_status: Option<String>,
    pub primary_artifact_id: Option<ArtifactId>,
    pub rendered_answer_artifact_id: Option<ArtifactId>,
    pub detail: Option<DetailView>,
    pub composer: ComposerState,
    pub slash_palette: SlashPalette,
    pub palette_draft: Option<String>,
    pub theme_registry: ThemeRegistry,
    pub theme_names: Vec<String>,
    pub theme_selected: usize,
    pub active_theme: YsdaTheme,
    pub preview_theme: Option<YsdaTheme>,
    pub preferences: UiPreferences,
    pub no_color: bool,
    pub pending_preferences: Option<UiPreferences>,
    pub scroll: u16,
    pub base_view: BaseView,
    pub transient: Option<TransientView>,
    pub diagnostics: DiagnosticsView,
    pub safe_warning: Option<String>,
    pub mouse_enabled: bool,
    pub should_quit: bool,
}

impl TuiApp {
    pub fn for_principal(principal: Principal) -> Self {
        let theme_registry = ThemeRegistry::default();
        let theme_names = theme_registry.names().map(str::to_owned).collect();
        let active_theme = theme_registry.resolve("deep-navy").expect("built-in theme");
        Self {
            workspace_name: "local".to_owned(),
            principal_name: principal.display_name,
            model_label: "not checked".to_owned(),
            connection_label: "not checked".to_owned(),
            permission_label: "read-only required".to_owned(),
            doctor_report: None,
            focused_task: None,
            transcript: Vec::new(),
            runtime_status: None,
            primary_artifact_id: None,
            rendered_answer_artifact_id: None,
            detail: None,
            composer: ComposerState::new(),
            slash_palette: SlashPalette::with_default_commands(),
            palette_draft: None,
            theme_registry,
            theme_names,
            theme_selected: 0,
            active_theme,
            preview_theme: None,
            preferences: UiPreferences::default(),
            no_color: false,
            pending_preferences: None,
            scroll: 0,
            base_view: BaseView::Home,
            transient: None,
            diagnostics: DiagnosticsView::default(),
            safe_warning: None,
            mouse_enabled: false,
            should_quit: false,
        }
    }

    pub fn test_home(workspace: &str, connection: &str, permission: &str, model: &str) -> Self {
        let mut app = Self::for_principal(Principal::local_operator("test-operator"));
        app.workspace_name = workspace.to_owned();
        app.connection_label = connection.to_owned();
        app.permission_label = permission.to_owned();
        app.model_label = model.to_owned();
        app
    }

    pub fn test_answer(
        question: &str,
        conclusion: &str,
        key_values: [Option<&str>; 2],
        explanation: Option<&str>,
    ) -> Self {
        let mut app = Self::test_home("ecommerce", "fixture", "read-only", "fixture-model");
        app.transcript
            .push(TranscriptItem::UserMessage(question.to_owned()));
        app.transcript.push(TranscriptItem::Answer(AnswerView {
            state: "completed · 1.7s".to_owned(),
            conclusion: conclusion.to_owned(),
            key_values: key_values.map(|value| value.map(str::to_owned)),
            explanation: explanation.map(str::to_owned),
        }));
        app
    }

    pub fn query_submission_enabled(&self) -> bool {
        self.doctor_report
            .as_ref()
            .is_some_and(DoctorReport::allows_query_submission)
    }

    pub fn sync_slash_palette(&mut self) {
        if self
            .transient
            .is_some_and(|view| view != TransientView::SlashPalette)
        {
            return;
        }

        let was_open = self.transient == Some(TransientView::SlashPalette);
        let command_has_argument = self
            .composer
            .text()
            .strip_prefix('/')
            .is_some_and(|command| {
                command
                    .split_once(char::is_whitespace)
                    .is_some_and(|(token, _)| !token.is_empty())
            });
        let visible = if command_has_argument {
            self.slash_palette.clear();
            false
        } else {
            self.slash_palette.update(self.composer.text())
        };
        if visible && !was_open {
            self.palette_draft = Some(String::new());
        } else if !visible {
            self.palette_draft = None;
        }
        self.transient = visible.then_some(TransientView::SlashPalette);
    }

    pub fn close_transient(&mut self) {
        if self.transient == Some(TransientView::SlashPalette) {
            self.composer
                .set_text(&self.palette_draft.take().unwrap_or_default());
            self.slash_palette.clear();
        }
        if self.transient == Some(TransientView::ThemePicker) {
            self.preview_theme = None;
        }
        self.transient = None;
        self.detail = None;
    }

    pub fn push_transcript(&mut self, item: TranscriptItem) {
        self.transcript.push(item);
        self.scroll = u16::MAX;
    }

    pub fn set_runtime_status(&mut self, status: impl Into<String>) {
        self.runtime_status = Some(status.into());
    }

    pub fn show_detail(&mut self, kind: DetailKind, detail: DetailView) {
        self.detail = Some(detail);
        self.transient = Some(TransientView::Detail(kind));
    }

    pub fn apply_preferences(&mut self, preferences: &UiPreferences, no_color: bool) {
        self.no_color = no_color;
        match self
            .theme_registry
            .resolve_preferences(preferences, no_color)
        {
            Ok(theme) => {
                self.active_theme = theme;
                self.preferences = preferences.clone();
                self.safe_warning = None;
            }
            Err(error) => self.safe_warning = Some(error.code().to_owned()),
        }
    }
}

/// Maps typed terminal actions onto the single AgentService boundary.
pub struct TuiController {
    service: Arc<dyn AgentServiceApi>,
    workspace_id: WorkspaceId,
    principal: Principal,
    session_id: Option<SessionId>,
    focused_task_id: Option<TaskId>,
    focused_run_id: Option<RunId>,
    subscription: Option<EventSubscription>,
    pending_command_id: Option<CommandId>,
    pending_submission: Option<JoinHandle<ys_agent_core::CoreResult<SubmissionCompletion>>>,
}

pub(super) enum SubmissionCompletion {
    Message {
        session_id: SessionId,
        reply: ServiceReply,
    },
    ClarificationAnswered,
}

impl TuiController {
    pub fn new(
        service: Arc<dyn AgentServiceApi>,
        workspace_id: WorkspaceId,
        principal: Principal,
    ) -> Self {
        Self {
            service,
            workspace_id,
            principal,
            session_id: None,
            focused_task_id: None,
            focused_run_id: None,
            subscription: None,
            pending_command_id: None,
            pending_submission: None,
        }
    }

    pub fn submission_in_flight(&self) -> bool {
        self.pending_submission.is_some()
    }

    pub(super) async fn take_ready_submission(
        &mut self,
    ) -> Option<ys_agent_core::CoreResult<SubmissionCompletion>> {
        if !self
            .pending_submission
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            return None;
        }
        let handle = self
            .pending_submission
            .take()
            .expect("finished submission exists");
        Some(handle.await.unwrap_or_else(|error| {
            Err(ys_agent_core::CoreError::Storage {
                message: format!("TUI submission task failed: {error}"),
            })
        }))
    }

    pub(super) fn complete_submission(
        &mut self,
        app: &mut TuiApp,
        completion: SubmissionCompletion,
    ) {
        match completion {
            SubmissionCompletion::Message { session_id, reply } => {
                self.session_id = Some(session_id);
                app.diagnostics.session_id = Some(session_id);
                self.apply_message_reply(app, reply);
            }
            SubmissionCompletion::ClarificationAnswered => {
                app.set_runtime_status("Clarification accepted; resuming the same Run");
                app.close_transient();
                app.base_view = BaseView::Home;
            }
        }
    }

    fn begin_command(&mut self) -> CommandId {
        let command_id = self.pending_command_id.unwrap_or_default();
        self.pending_command_id = Some(command_id);
        command_id
    }

    fn finish_command(&mut self) {
        self.pending_command_id = None;
    }

    async fn ensure_session(&mut self) -> ys_agent_core::CoreResult<SessionId> {
        if let Some(session_id) = self.session_id {
            return Ok(session_id);
        }
        let command_id = self.begin_command();
        let session = self
            .service
            .create_session(command_id, self.principal.clone())
            .await?;
        self.finish_command();
        self.session_id = Some(session.id);
        Ok(session.id)
    }

    pub async fn apply(
        &mut self,
        app: &mut TuiApp,
        action: InputAction,
    ) -> ys_agent_core::CoreResult<()> {
        match action {
            InputAction::Empty => {}
            InputAction::Quit => app.should_quit = true,
            InputAction::ShowDetail(request) => self.show_detail(app, request).await?,
            InputAction::OpenThemePicker => {
                app.theme_selected = app
                    .theme_names
                    .iter()
                    .position(|name| name == &app.active_theme.name)
                    .unwrap_or(0);
                app.preview_theme = None;
                app.transient = Some(TransientView::ThemePicker);
            }
            InputAction::SetThemeColor { token, color } => {
                let mut preferences = app.preferences.clone();
                preferences.theme = "custom".to_owned();
                preferences
                    .colors
                    .insert(token.as_str().to_owned(), color.as_persisted());
                let theme = app
                    .theme_registry
                    .resolve_preferences(&preferences, app.no_color)
                    .map_err(|error| {
                        ys_agent_core::CoreError::validation(error.code(), error.to_string())
                    })?;
                app.active_theme = theme;
                app.preferences = preferences.clone();
                app.pending_preferences = Some(preferences);
            }
            InputAction::ResetTheme => {
                let preferences = UiPreferences::default();
                app.active_theme = app
                    .theme_registry
                    .resolve_preferences(&preferences, app.no_color)
                    .expect("built-in deep-navy theme");
                app.preferences = preferences.clone();
                app.pending_preferences = Some(preferences);
            }
            InputAction::Help => app.transient = Some(TransientView::Help),
            InputAction::Connections => app.show_detail(
                DetailKind::Connections,
                DetailView {
                    title: "Connections".to_owned(),
                    lines: vec![format!(
                        "{} · {}",
                        app.connection_label, app.permission_label
                    )],
                },
            ),
            InputAction::Model => app.show_detail(
                DetailKind::Model,
                DetailView {
                    title: "Model".to_owned(),
                    lines: vec![app.model_label.clone()],
                },
            ),
            InputAction::Doctor => {
                let report = self.service.doctor().await?;
                app.transient =
                    (!report.allows_query_submission()).then_some(TransientView::Repair);
                app.doctor_report = Some(report);
            }
            InputAction::NewSession => self.new_session(app).await?,
            InputAction::ListTasks => self.list_tasks(app).await?,
            InputAction::NewTask(goal) => self.new_task(app, goal).await?,
            InputAction::ResumeTask { task_id } => self.resume_task(app, task_id).await?,
            InputAction::CancelRun { run_id } => {
                let command_id = self.begin_command();
                self.service
                    .cancel_run(command_id, &run_id, "cancelled from TUI".to_owned())
                    .await?;
                self.finish_command();
                app.set_runtime_status("Run cancellation requested");
            }
            InputAction::ExportArtifact {
                artifact_id,
                format,
            } => self.export(app, artifact_id, format).await?,
            InputAction::SendMessage(text) => {
                if app.base_view == BaseView::Clarification {
                    self.start_clarification_submission(app, text)?;
                } else {
                    self.start_message_submission(app, text)?;
                }
            }
        }
        Ok(())
    }

    async fn new_session(&mut self, app: &mut TuiApp) -> ys_agent_core::CoreResult<()> {
        let command_id = self.begin_command();
        let session = self
            .service
            .create_session(command_id, self.principal.clone())
            .await?;
        self.finish_command();
        self.session_id = Some(session.id);
        self.focused_task_id = None;
        self.focused_run_id = None;
        self.subscription = None;
        app.focused_task = None;
        app.transcript.clear();
        app.primary_artifact_id = None;
        app.rendered_answer_artifact_id = None;
        app.runtime_status = None;
        app.detail = None;
        app.transient = None;
        app.diagnostics = DiagnosticsView {
            session_id: Some(session.id),
            ..DiagnosticsView::default()
        };
        Ok(())
    }

    async fn list_tasks(&self, app: &mut TuiApp) -> ys_agent_core::CoreResult<()> {
        let tasks = self.service.list_tasks(&self.workspace_id).await?;
        let lines = tasks
            .into_iter()
            .map(|task| format!("{} · {:?} · {}", task.id, task.status, task.goal))
            .collect();
        app.show_detail(
            DetailKind::Tasks,
            DetailView {
                title: "Tasks".to_owned(),
                lines,
            },
        );
        Ok(())
    }

    async fn new_task(&mut self, app: &mut TuiApp, goal: String) -> ys_agent_core::CoreResult<()> {
        let session_id = self.ensure_session().await?;
        let command_id = self.begin_command();
        let task = self
            .service
            .create_task(CreateTaskRequest {
                command_id,
                session_id,
                goal: goal.clone(),
                acceptance_criteria: Vec::new(),
            })
            .await?;
        self.finish_command();
        self.focused_task_id = Some(task.id);
        app.focused_task = Some(TaskSummary {
            goal,
            status: format!("{:?}", task.status),
            needs_input: false,
        });
        app.diagnostics.session_id = Some(session_id);
        app.diagnostics.task_id = Some(task.id);
        app.set_runtime_status("Task created");
        Ok(())
    }

    async fn resume_task(
        &mut self,
        app: &mut TuiApp,
        task_id: TaskId,
    ) -> ys_agent_core::CoreResult<()> {
        if !self.service.doctor().await?.allows_query_submission() {
            return Err(ys_agent_core::CoreError::validation(
                "workspace_not_ready",
                "Doctor blockers disable Task resume",
            ));
        }
        let command_id = self.begin_command();
        let run_id = self.service.resume_task(command_id, &task_id).await?;
        self.finish_command();
        self.focused_task_id = Some(task_id);
        self.focused_run_id = Some(run_id);
        self.subscription = None;
        app.diagnostics.task_id = Some(task_id);
        app.diagnostics.run_id = Some(run_id);
        app.set_runtime_status("Task resumed");
        Ok(())
    }

    async fn export(
        &mut self,
        app: &mut TuiApp,
        artifact_id: ArtifactId,
        format: ExportFormat,
    ) -> ys_agent_core::CoreResult<()> {
        let command_id = self.begin_command();
        let metadata = self
            .service
            .export_artifact(
                command_id,
                &artifact_id,
                format,
                self.access(ArtifactAccessPurpose::Export),
            )
            .await?;
        self.finish_command();
        app.show_detail(
            DetailKind::Artifact,
            DetailView {
                title: "Export".to_owned(),
                lines: vec![
                    format!("Artifact: {}", metadata.id),
                    format!("Location: {}", metadata.storage_uri),
                ],
            },
        );
        Ok(())
    }

    fn start_message_submission(
        &mut self,
        app: &mut TuiApp,
        text: String,
    ) -> ys_agent_core::CoreResult<()> {
        if !app.query_submission_enabled() {
            app.transient = Some(TransientView::Repair);
            app.push_transcript(TranscriptItem::Warning(
                "Run /doctor and repair blockers before submitting a query".to_owned(),
            ));
            return Ok(());
        }
        if self.submission_in_flight() {
            return Err(ys_agent_core::CoreError::validation(
                "submission_in_progress",
                "Wait for the current request to finish",
            ));
        }
        app.push_transcript(TranscriptItem::UserMessage(text.clone()));
        app.set_runtime_status("Thinking…");

        let service = self.service.clone();
        let principal = self.principal.clone();
        let existing_session_id = self.session_id;
        let focused_task_id = self.focused_task_id;
        self.pending_submission = Some(tokio::spawn(async move {
            let session_id = match existing_session_id {
                Some(session_id) => session_id,
                None => {
                    service
                        .create_session(CommandId::new(), principal)
                        .await?
                        .id
                }
            };
            let reply = service
                .send_message(SendMessageRequest {
                    command_id: CommandId::new(),
                    session_id,
                    focused_task_id,
                    text,
                })
                .await?;
            Ok(SubmissionCompletion::Message { session_id, reply })
        }));
        Ok(())
    }

    fn start_clarification_submission(
        &mut self,
        app: &mut TuiApp,
        text: String,
    ) -> ys_agent_core::CoreResult<()> {
        if self.submission_in_flight() {
            return Err(ys_agent_core::CoreError::validation(
                "submission_in_progress",
                "Wait for the current request to finish",
            ));
        }
        let run_id = self.focused_run_id.ok_or_else(|| {
            ys_agent_core::CoreError::validation(
                "missing_waiting_run",
                "clarification mode has no focused Run",
            )
        })?;
        app.push_transcript(TranscriptItem::UserMessage(text.clone()));
        app.set_runtime_status("Submitting clarification…");
        let service = self.service.clone();
        self.pending_submission = Some(tokio::spawn(async move {
            service
                .answer_clarification(CommandId::new(), &run_id, text)
                .await?;
            Ok(SubmissionCompletion::ClarificationAnswered)
        }));
        Ok(())
    }

    fn apply_message_reply(&mut self, app: &mut TuiApp, reply: ServiceReply) {
        match reply {
            ServiceReply::Conversation { message } => {
                app.push_transcript(TranscriptItem::Answer(AnswerView {
                    state: "Chat".to_owned(),
                    conclusion: message,
                    key_values: [None, None],
                    explanation: None,
                }));
                app.set_runtime_status("Ready");
            }
            ServiceReply::RunScheduled { task_id, run_id } => self.focus_run(app, task_id, run_id),
            ServiceReply::ClarificationRequired {
                task_id,
                run_id,
                question,
            } => {
                self.focus_run(app, task_id, run_id);
                app.base_view = BaseView::Clarification;
                app.push_transcript(TranscriptItem::Clarification {
                    question,
                    recommended_default: None,
                });
            }
            ServiceReply::UnsupportedCapability {
                workflow, message, ..
            } => app.push_transcript(TranscriptItem::Warning(format!(
                "Unsupported {workflow:?}: {message}"
            ))),
        }
    }

    fn focus_run(&mut self, app: &mut TuiApp, task_id: TaskId, run_id: RunId) {
        self.focused_task_id = Some(task_id);
        self.focused_run_id = Some(run_id);
        self.subscription = None;
        app.diagnostics.task_id = Some(task_id);
        app.diagnostics.run_id = Some(run_id);
        app.primary_artifact_id = None;
        app.rendered_answer_artifact_id = None;
        app.set_runtime_status("Query scheduled");
    }

    fn access(&self, purpose: ArtifactAccessPurpose) -> ArtifactAccessContext {
        ArtifactAccessContext {
            workspace_id: self.workspace_id,
            principal_id: self.principal.id,
            purpose,
            max_sensitivity: Sensitivity::Internal,
        }
    }

    async fn show_detail(
        &self,
        app: &mut TuiApp,
        request: DetailRequest,
    ) -> ys_agent_core::CoreResult<()> {
        if matches!(request, DetailRequest::Diagnostics) {
            app.show_detail(
                DetailKind::Diagnostics,
                DetailView {
                    title: "Diagnostics".to_owned(),
                    lines: vec![
                        format!("Session: {:?}", app.diagnostics.session_id),
                        format!("Task: {:?}", app.diagnostics.task_id),
                        format!("Run: {:?}", app.diagnostics.run_id),
                        format!("Step: {:?}", app.diagnostics.step_id),
                        format!("Phase: {:?}", app.diagnostics.query_phase),
                    ],
                },
            );
            return Ok(());
        }
        let artifact_id = match request {
            DetailRequest::Artifact(Some(artifact_id)) => artifact_id,
            DetailRequest::Artifact(None)
            | DetailRequest::Metrics
            | DetailRequest::Query
            | DetailRequest::Checks
            | DetailRequest::Sql => app.primary_artifact_id.ok_or_else(|| {
                ys_agent_core::CoreError::validation(
                    "primary_artifact_missing",
                    "No completed Query Artifact is focused",
                )
            })?,
            DetailRequest::Diagnostics => unreachable!(),
        };
        let view = self
            .service
            .get_artifact(&artifact_id, self.access(ArtifactAccessPurpose::TuiPreview))
            .await?;
        let artifact: QueryArtifact = serde_json::from_slice(&view.preview).map_err(|error| {
            ys_agent_core::CoreError::validation(
                "invalid_query_artifact_preview",
                error.to_string(),
            )
        })?;
        let (kind, title, lines) = match request {
            DetailRequest::Artifact(_) => (
                DetailKind::Artifact,
                "Artifact",
                vec![
                    format!("Kind: {:?}", view.metadata.kind),
                    format!("Hash: {}", view.metadata.content_hash),
                    format!("Size: {} bytes", view.metadata.size_bytes),
                ],
            ),
            DetailRequest::Metrics => (
                DetailKind::Metrics,
                "Metrics",
                artifact
                    .metric
                    .map(|metric| vec![format!("{} v{}", metric.id, metric.version)])
                    .unwrap_or_else(|| vec!["No governed Metric was used".to_owned()]),
            ),
            DetailRequest::Query => (
                DetailKind::Query,
                "Query",
                vec![
                    format!("Question: {}", artifact.question),
                    format!("Source: {}", artifact.source_id.as_str()),
                    format!("Relations: {}", artifact.source_relations.join(", ")),
                ],
            ),
            DetailRequest::Checks => (
                DetailKind::Checks,
                "Checks",
                artifact
                    .warning_codes
                    .into_iter()
                    .map(|code| format!("Warning: {code}"))
                    .chain(std::iter::once(format!(
                        "Governance: {:?}",
                        artifact.semantic_status
                    )))
                    .collect(),
            ),
            DetailRequest::Sql => (
                DetailKind::Sql,
                "SQL",
                vec![
                    artifact
                        .executed_sql
                        .unwrap_or_else(|| "No SQL was executed for this answer".to_owned()),
                ],
            ),
            DetailRequest::Diagnostics => unreachable!(),
        };
        app.show_detail(
            kind,
            DetailView {
                title: title.to_owned(),
                lines,
            },
        );
        Ok(())
    }

    pub async fn doctor(&self) -> ys_agent_core::CoreResult<DoctorReport> {
        self.service.doctor().await
    }

    pub async fn next_service_event(&mut self) -> ys_agent_core::CoreResult<EventEnvelope> {
        loop {
            let Some(run_id) = self.focused_run_id else {
                return std::future::pending().await;
            };
            if self.subscription.is_none() {
                self.subscription = Some(self.service.subscribe_events(&run_id, 0).await?);
            }
            let event = self
                .subscription
                .as_mut()
                .expect("subscription initialized")
                .next()
                .await?;
            if self.focused_run_id == Some(event.run_id) {
                return Ok(event);
            }
            self.subscription = None;
        }
    }

    pub fn apply_service_event(&mut self, app: &mut TuiApp, envelope: EventEnvelope) {
        match envelope.event.kind {
            RunEventKind::StepStarted { step_id, label } => {
                app.diagnostics.step_id = Some(step_id);
                app.set_runtime_status(label);
            }
            RunEventKind::ToolCallProposed { call } => {
                app.set_runtime_status(format!("Checking {}", call.name))
            }
            RunEventKind::ToolExecutionSucceeded { .. } => {
                app.set_runtime_status("Governed check completed")
            }
            RunEventKind::ToolExecutionFailed { failure, .. }
            | RunEventKind::ToolExecutionIndeterminate { failure, .. } => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Warning(format!(
                    "Tool outcome: {}",
                    failure.code
                )));
            }
            RunEventKind::ClarificationRequested { .. } => {
                app.base_view = BaseView::Clarification;
                app.set_runtime_status("Waiting for clarification");
            }
            RunEventKind::RunWaiting { reason } => {
                app.set_runtime_status(format!("Waiting: {reason}"))
            }
            RunEventKind::RunCompleted {
                primary_artifact_id,
            } => {
                app.base_view = BaseView::Home;
                app.primary_artifact_id = Some(primary_artifact_id);
                app.set_runtime_status("Preparing verified answer");
            }
            RunEventKind::RunFailed { code, .. } => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Error(format!(
                    "What happened: {code}. Use /details for diagnostics."
                )));
            }
            RunEventKind::RunCancelled { .. } => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Warning("Run cancelled".to_owned()));
            }
            _ => {}
        }
    }

    pub async fn reload_durable_state(
        &mut self,
        app: &mut TuiApp,
    ) -> ys_agent_core::CoreResult<()> {
        let Some(run_id) = self.focused_run_id else {
            return Ok(());
        };
        let snapshot = self.service.get_run(&run_id).await?;
        app.diagnostics.run_id = Some(snapshot.run_id);
        app.diagnostics.task_id = Some(snapshot.task_id);
        app.diagnostics.step_id = snapshot.last_completed_step_id;
        app.diagnostics.query_phase = snapshot
            .workflow_state
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        match snapshot.status {
            RunStatus::WaitingForInput => {
                app.base_view = BaseView::Clarification;
                show_waiting_snapshot(app, &snapshot);
            }
            RunStatus::Succeeded => {
                app.base_view = BaseView::Home;
                self.show_success_snapshot(app, &snapshot).await?;
                self.clear_active_run();
            }
            RunStatus::Failed => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Error(user_readable_run_failure(&snapshot)));
                self.clear_active_run();
            }
            RunStatus::Cancelled => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Warning("Run cancelled".to_owned()));
                self.clear_active_run();
            }
            RunStatus::Queued | RunStatus::Running => {}
        }
        Ok(())
    }

    fn clear_active_run(&mut self) {
        self.focused_run_id = None;
        self.subscription = None;
    }

    async fn show_success_snapshot(
        &self,
        app: &mut TuiApp,
        snapshot: &ys_agent_core::RunSnapshot,
    ) -> ys_agent_core::CoreResult<()> {
        let Some(artifact_id) = snapshot.primary_artifact_id else {
            return Ok(());
        };
        app.primary_artifact_id = Some(artifact_id);
        let view = self
            .service
            .get_artifact(&artifact_id, self.access(ArtifactAccessPurpose::TuiPreview))
            .await?;
        if view.truncated {
            app.runtime_status = None;
            app.push_transcript(TranscriptItem::Warning(
                "Verified answer is available through /artifact; concise preview is too large"
                    .to_owned(),
            ));
            return Ok(());
        }
        let artifact: QueryArtifact = serde_json::from_slice(&view.preview).map_err(|error| {
            ys_agent_core::CoreError::validation(
                "invalid_query_artifact_preview",
                error.to_string(),
            )
        })?;
        if app.rendered_answer_artifact_id != Some(artifact_id) {
            let key_values = self.load_key_values(&artifact).await;
            app.push_transcript(TranscriptItem::Answer(AnswerView {
                state: "completed".to_owned(),
                conclusion: concise_line(&artifact.answer_summary, 240),
                key_values,
                explanation: artifact
                    .assumptions
                    .first()
                    .map(|value| concise_line(value, 160)),
            }));
            app.rendered_answer_artifact_id = Some(artifact_id);
        }
        app.runtime_status = None;
        Ok(())
    }

    async fn load_key_values(&self, artifact: &QueryArtifact) -> [Option<String>; 2] {
        let Some(reference) = &artifact.result_artifact else {
            return [None, None];
        };
        let Ok(view) = self
            .service
            .get_artifact(
                &reference.metadata.id,
                self.access(ArtifactAccessPurpose::TuiPreview),
            )
            .await
        else {
            return [None, None];
        };
        if view.truncated {
            return [None, None];
        }
        let Ok(result) = serde_json::from_slice::<PersistedResultBody>(&view.preview) else {
            return [None, None];
        };
        let [row] = result.rows.as_slice() else {
            return [None, None];
        };
        let mut values = [None, None];
        for (index, (column, value)) in result.columns.iter().zip(row).take(2).enumerate() {
            let rendered = match value {
                serde_json::Value::String(value) => value.clone(),
                other => other.to_string(),
            };
            values[index] = Some(concise_line(&format!("{column}: {rendered}"), 80));
        }
        values
    }
}

impl Drop for TuiController {
    fn drop(&mut self) {
        if let Some(handle) = self.pending_submission.take() {
            handle.abort();
        }
    }
}

fn show_waiting_snapshot(app: &mut TuiApp, snapshot: &ys_agent_core::RunSnapshot) {
    let metadata = snapshot.pending_wait_metadata.as_ref();
    let question = metadata
        .and_then(|value| value.get("question"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Please clarify the query before the same Run resumes");
    let recommended_default = metadata
        .and_then(|value| value.get("recommended_default"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let visible = app.transcript.iter().any(|item| matches!(item, TranscriptItem::Clarification { question: shown, .. } if shown == question));
    if !visible {
        app.push_transcript(TranscriptItem::Clarification {
            question: question.to_owned(),
            recommended_default,
        });
    }
    app.runtime_status = None;
}

fn concise_line(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn user_readable_run_failure(snapshot: &ys_agent_core::RunSnapshot) -> String {
    let failure = snapshot.workflow_state.get("failure");
    let what_happened = failure
        .and_then(|value| value.get("what_happened"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("The query Run failed");
    let required_action = failure
        .and_then(|value| value.get("required_user_action"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("inspect Evidence before retrying");
    format!(
        "What happened: {what_happened}. Required action: {required_action}. Use /details for retry and Evidence diagnostics."
    )
}
