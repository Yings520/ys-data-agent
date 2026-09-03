use std::{sync::Arc, time::Duration};

use tokio::task::JoinHandle;

use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactId, CommandId, CredentialGeneration,
    CredentialKind, CredentialMutation, CredentialMutationIntent, CredentialMutationRequest,
    EventEnvelope, ExportFormat, OperationId, Principal, ProfileId, ProfileName, ProfileRevision,
    ProtectedCredentialWrite, ProviderCredentialReference, ProviderErrorCode, ProviderField,
    ProviderId, ProviderManagementError, ProviderRemediation, RunEventKind, RunId, RunStatus,
    SaveProfileRequest, SaveProfileRevision, Sensitivity, SessionId, StepId, TaskId, WorkspaceId,
};
use ys_agent_runtime::{
    AgentServiceApi, CreateTaskRequest, EventSubscription, QueryArtifact, SendMessageRequest,
    ServiceReply, doctor::DoctorReport, export::PersistedResultBody,
};

use super::input::{DetailRequest, InputAction};
use super::{
    AsyncOperationRegistry, ProviderOperationCompletion, ProviderOperationPolicy,
    composer::ComposerState,
    palette::SlashPalette,
    provider_management::{
        ProviderManagementScreen, ProviderManagementScreenView, ProviderManagementStep,
        ProviderManagementView, ProviderOperationKind, ProviderProfileView, ProviderResultOutcome,
    },
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
    Providers,
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
    provider_screen: Option<ProviderManagementScreen>,
    provider_operations: AsyncOperationRegistry<ProviderOperationPayload>,
}

pub(super) enum SubmissionCompletion {
    Message {
        session_id: SessionId,
        reply: ServiceReply,
    },
    ClarificationAnswered,
}

pub(super) enum ProviderOperationPayload {
    Discovery(Vec<ys_agent_core::DiscoveredModel>),
    Saved {
        browse: ProviderManagementView,
        profile_id: ProfileId,
        resume_step: ProviderManagementStep,
    },
    Committed(ProviderManagementView),
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
            provider_screen: None,
            provider_operations: AsyncOperationRegistry::new(
                ProviderOperationPolicy::new(Duration::from_secs(30), 2)
                    .expect("fixed Provider operation policy is valid"),
            ),
        }
    }

    pub fn submission_in_flight(&self) -> bool {
        self.pending_submission.is_some()
    }

    pub fn provider_operation_in_flight(&self) -> bool {
        self.provider_operations.active_count() > 0
    }

    /// Schedules a Provider operation outside the render loop. The reducer owns the operation ID
    /// and receives only its committed result; all data access remains behind `AgentServiceApi`.
    pub fn start_provider_operation(
        &mut self,
        kind: ProviderOperationKind,
    ) -> ys_agent_core::CoreResult<OperationId> {
        let (command, secret, resume_step) = {
            let screen = self.provider_screen.as_mut().ok_or_else(|| {
                ys_agent_core::CoreError::validation(
                    "provider_screen_not_open",
                    "Open /providers before starting a Provider operation",
                )
            })?;
            let command = screen.edit_command().ok_or_else(|| {
                ys_agent_core::CoreError::validation(
                    "provider_edit_incomplete",
                    "Complete the Provider edit fields before starting an operation",
                )
            })?;
            let secret = (kind == ProviderOperationKind::SaveDraft)
                .then(|| screen.take_secret_input())
                .flatten();
            (command, secret, screen.view().step)
        };
        let profile_id = command.profile_id;
        if kind != ProviderOperationKind::SaveDraft && profile_id.is_none() {
            return Err(ys_agent_core::CoreError::validation(
                "provider_profile_not_persisted",
                "Save the Provider Draft before discovery, validation, OAuth, or activation",
            ));
        }
        let resume_step = resume_step.ok_or_else(|| {
            ys_agent_core::CoreError::validation(
                "provider_operation_not_requested",
                "The current Provider screen state does not allow this operation",
            )
        })?;
        let service = self.service.clone();
        let mut secret = secret;
        let operation_id = self
            .provider_operations
            .start(kind, move |operation_id, _| {
                let service = service.clone();
                let command = command.clone();
                let secret = secret.take();
                async move {
                    let payload = match kind {
                        ProviderOperationKind::DiscoverModels => {
                            let profile_id =
                                profile_id.expect("non-save operations require a Profile");
                            let detail = service.provider_load_profile(profile_id).await?;
                            let generation = detail.credential_generation.ok_or_else(|| {
                                ys_agent_core::ProviderManagementError::new(
                                    ys_agent_core::ProviderErrorCode::CredentialMissing,
                                    Some(ys_agent_core::ProviderField::Credential),
                                    ys_agent_core::ProviderRemediation::ConfigureCredentialStore,
                                )
                            })?;
                            let models = service
                                .provider_discover_models(ys_agent_core::DiscoverModelsRequest {
                                    operation_id,
                                    profile_id,
                                    profile_revision: detail.revision,
                                    provider: command.provider,
                                    credential_generation: generation,
                                })
                                .await?;
                            ProviderOperationPayload::Discovery(models)
                        }
                        ProviderOperationKind::Validate => {
                            let profile_id =
                                profile_id.expect("non-save operations require a Profile");
                            let detail = service.provider_load_profile(profile_id).await?;
                            service
                                .provider_validate(ys_agent_core::ValidateProfileRequest {
                                    operation_id,
                                    profile_id,
                                    revision: detail.revision,
                                    observed_context_limit: command.observed_context_limit,
                                })
                                .await?;
                            ProviderOperationPayload::Committed(
                                load_provider_management_view(service.as_ref()).await?,
                            )
                        }
                        ProviderOperationKind::Activate => {
                            let profile_id =
                                profile_id.expect("non-save operations require a Profile");
                            service
                                .provider_activate_current(profile_id, operation_id)
                                .await?;
                            ProviderOperationPayload::Committed(
                                load_provider_management_view(service.as_ref()).await?,
                            )
                        }
                        ProviderOperationKind::OAuth => {
                            let profile_id =
                                profile_id.expect("non-save operations require a Profile");
                            service
                                .provider_start_oauth(profile_id, operation_id)
                                .await?;
                            ProviderOperationPayload::Committed(
                                load_provider_management_view(service.as_ref()).await?,
                            )
                        }
                        ProviderOperationKind::SaveDraft => {
                            let detail = save_provider_draft(
                                service.as_ref(),
                                command,
                                secret,
                                operation_id,
                                resume_step,
                            )
                            .await?;
                            ProviderOperationPayload::Saved {
                                browse: load_provider_management_view(service.as_ref()).await?,
                                profile_id: detail.summary.profile_id,
                                resume_step,
                            }
                        }
                    };
                    Ok(payload)
                }
            });
        let screen = self
            .provider_screen
            .as_mut()
            .expect("screen was present while scheduling operation");
        if !screen.start_operation(operation_id, kind) {
            let _ = self.provider_operations.cancel(operation_id);
            return Err(ys_agent_core::CoreError::validation(
                "provider_operation_not_requested",
                "The current Provider screen state does not allow this operation",
            ));
        }
        Ok(operation_id)
    }

    pub fn advance_provider_step(&mut self, app: &mut TuiApp) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let advanced = screen.next_step();
        if advanced {
            refresh_provider_detail(app, screen);
        }
        advanced
    }

    pub(super) fn provider_screen_view(&self) -> Option<ProviderManagementScreenView> {
        self.provider_screen
            .as_ref()
            .map(ProviderManagementScreen::view)
    }

    pub(super) fn start_provider_draft(&mut self, app: &mut TuiApp) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let sequence = screen.view().browse.profiles.len().saturating_add(1);
        let started = screen.start_create(format!("Provider Profile {sequence}"));
        if started {
            refresh_provider_detail(app, screen);
        }
        started
    }

    pub(super) fn select_provider_for_draft(
        &mut self,
        app: &mut TuiApp,
        provider: ProviderId,
    ) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let selected = screen.select_provider(provider);
        if selected {
            refresh_provider_detail(app, screen);
        }
        selected
    }

    pub(super) fn select_provider_authentication(
        &mut self,
        app: &mut TuiApp,
        authentication: super::provider_management::ProviderAuthentication,
    ) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let selected = screen.select_authentication(authentication);
        if selected {
            refresh_provider_detail(app, screen);
        }
        selected
    }

    pub(super) fn append_provider_text(&mut self, app: &mut TuiApp, character: char) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let changed = match screen.view().step {
            Some(ProviderManagementStep::Authentication) => {
                screen.append_secret_character(character)
            }
            Some(ProviderManagementStep::Model) => screen.append_manual_model_character(character),
            _ => false,
        };
        if changed {
            refresh_provider_detail(app, screen);
        }
        changed
    }

    pub(super) fn delete_provider_text(&mut self, app: &mut TuiApp) -> bool {
        let Some(screen) = self.provider_screen.as_mut() else {
            return false;
        };
        let changed = match screen.view().step {
            Some(ProviderManagementStep::Authentication) => screen.delete_secret_character(),
            Some(ProviderManagementStep::Model) => screen.delete_manual_model_character(),
            _ => false,
        };
        if changed {
            refresh_provider_detail(app, screen);
        }
        changed
    }

    pub fn request_provider_activation(&mut self) -> ys_agent_core::CoreResult<OperationId> {
        let screen = self.provider_screen.as_mut().ok_or_else(|| {
            ys_agent_core::CoreError::validation(
                "provider_screen_not_open",
                "Open /providers before activating a Provider",
            )
        })?;
        if !screen.request_activation() || screen.confirm_activation().is_none() {
            return Err(ys_agent_core::CoreError::validation(
                "provider_activation_not_ready",
                "Validate the current Provider revision before activation",
            ));
        }
        self.start_provider_operation(ProviderOperationKind::Activate)
    }

    pub fn retry_provider_operation(&mut self) -> ys_agent_core::CoreResult<Option<OperationId>> {
        let Some(screen) = self.provider_screen.as_mut() else {
            return Ok(None);
        };
        let Some(request) = screen.retry() else {
            return Ok(None);
        };
        let super::provider_management::ProviderScreenRequest::Operation(kind) = request;
        self.start_provider_operation(kind).map(Some)
    }

    pub(super) fn take_ready_provider_operation(
        &mut self,
    ) -> Option<ProviderOperationCompletion<ProviderOperationPayload>> {
        self.provider_operations.try_next_completion()
    }

    pub(super) fn apply_provider_operation(
        &mut self,
        app: &mut TuiApp,
        completion: ProviderOperationCompletion<ProviderOperationPayload>,
    ) {
        let Some(screen) = self.provider_screen.as_mut() else {
            return;
        };
        match completion.result {
            Ok(ProviderOperationPayload::Discovery(models)) => {
                let _ = screen.complete_discovery(completion.operation_id, models);
            }
            Ok(ProviderOperationPayload::Saved {
                browse,
                profile_id,
                resume_step,
            }) => {
                let _ =
                    screen.complete_saved_draft(completion.operation_id, profile_id, resume_step);
                screen.replace_browse(browse);
                app.set_runtime_status("Provider Draft saved; run Validate before activation");
            }
            Ok(ProviderOperationPayload::Committed(browse)) => {
                let _ = screen
                    .complete_operation(completion.operation_id, ProviderResultOutcome::Succeeded);
                screen.replace_browse(browse);
            }
            Err(error) => {
                let _ = screen.complete_operation(
                    completion.operation_id,
                    ProviderResultOutcome::Failed(error),
                );
            }
        }
        refresh_provider_detail(app, screen);
    }

    pub async fn cancel_provider_operation(&mut self, app: &mut TuiApp) {
        let Some(screen) = self.provider_screen.as_mut() else {
            return;
        };
        let Some(operation_id) = screen.cancel_busy() else {
            return;
        };
        let _ = self.provider_operations.cancel(operation_id);
        let _ = self.service.cancel_provider_operation(operation_id).await;
        refresh_provider_detail(app, screen);
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
            InputAction::Providers => self.open_provider_management(app, false).await?,
            InputAction::Model => self.open_provider_management(app, true).await?,
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

    /// Loads only masked, local Provider-management views through `AgentServiceApi`. No TUI path
    /// receives a repository, Vault, discovery client, or raw credential. `/model` calls this
    /// same route and advances the one reducer to its Model step instead of opening legacy UI.
    async fn open_provider_management(
        &mut self,
        app: &mut TuiApp,
        open_model_step: bool,
    ) -> ys_agent_core::CoreResult<()> {
        let catalog = self
            .service
            .provider_catalog()
            .await
            .map_err(provider_to_core)?;
        let active = self
            .service
            .provider_active()
            .await
            .map_err(provider_to_core)?;
        let summaries = self
            .service
            .provider_list_profiles()
            .await
            .map_err(provider_to_core)?;
        let mut profiles = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let detail = self
                .service
                .provider_load_profile(summary.profile_id)
                .await
                .map_err(provider_to_core)?;
            profiles.push(ProviderProfileView::from_detail(detail, None));
        }
        let browse = ProviderManagementView::new(catalog, profiles, active, false);
        let mut screen = ProviderManagementScreen::new(browse);
        if open_model_step {
            let view = screen.view();
            let profile = view
                .browse
                .active
                .as_ref()
                .and_then(|active| {
                    view.browse.profiles.iter().find(|profile| {
                        profile.profile_id == active.profile_id
                            && profile.revision == active.profile_revision
                    })
                })
                .or_else(|| view.browse.profiles.first());
            if let Some(profile) = profile
                && screen.start_edit(profile)
            {
                let _ = screen.next_step();
                let _ = screen.next_step();
            }
        }
        let view = screen.view();
        app.show_detail(
            DetailKind::Providers,
            DetailView {
                title: "Provider management".to_owned(),
                lines: provider_management_lines(&view),
            },
        );
        self.provider_screen = Some(screen);
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

fn provider_management_lines(
    view: &super::provider_management::ProviderManagementScreenView,
) -> Vec<String> {
    let mut lines = match &view.browse.active {
        Some(active) => vec![format!(
            "Active · {:?} · {} · {}",
            active.provider,
            active.model.as_str(),
            active.profile_revision
        )],
        None => vec!["No active Provider Profile".to_owned()],
    };
    lines.extend(view.browse.profiles.iter().map(|profile| {
        let marker = if profile.is_active { "active" } else { "saved" };
        format!(
            "{marker} · {} · {:?} · {} · {:?} · {:?}",
            profile.name,
            profile.provider,
            profile.model.as_str(),
            profile.state,
            profile.credential_status,
        )
    }));
    if view.browse.profiles.is_empty() {
        lines.push(
            "Create a Profile to configure Provider, authentication, model, and validation"
                .to_owned(),
        );
    }
    if let Some(step) = view.step {
        lines.push(format!("Editing step · {step:?}"));
    }
    if let Some(edit) = &view.edit {
        lines.push(format!("Draft · {} · {:?}", edit.name, edit.provider));
        lines.push(format!(
            "Authentication · {:?} · credential {}",
            edit.authentication,
            edit.credential_mask.unwrap_or("not entered")
        ));
        lines.push(format!(
            "Model · {} · parameters {:?}",
            edit.model
                .as_ref()
                .map_or("not selected", ys_agent_core::ProviderModelId::as_str),
            edit.parameters,
        ));
    }
    if let Some(busy) = &view.busy {
        lines.push(format!("Operation in progress · {:?}", busy.kind));
    }
    if let Some(result) = &view.result
        && let ProviderResultOutcome::Failed(error) = &result.outcome
    {
        lines.push(format!(
            "Provider field error · {} · {:?}",
            error.code(),
            error.field()
        ));
        lines.push(format!("Remediation · {:?}", error.remediation()));
    }
    lines.push(
        "Keys: n new · 1-9 Provider · k API key · o OAuth · Enter next · s save Draft · v validate · a activate · Esc cancel"
            .to_owned(),
    );
    lines
}

fn refresh_provider_detail(app: &mut TuiApp, screen: &ProviderManagementScreen) {
    let view = screen.view();
    app.show_detail(
        DetailKind::Providers,
        DetailView {
            title: "Provider management".to_owned(),
            lines: provider_management_lines(&view),
        },
    );
}

async fn load_provider_management_view(
    service: &dyn AgentServiceApi,
) -> ys_agent_core::ProviderResult<ProviderManagementView> {
    let catalog = service.provider_catalog().await?;
    let active = service.provider_active().await?;
    let summaries = service.provider_list_profiles().await?;
    let mut profiles = Vec::with_capacity(summaries.len());
    for summary in summaries {
        let detail = service.provider_load_profile(summary.profile_id).await?;
        profiles.push(ProviderProfileView::from_detail(detail, None));
    }
    Ok(ProviderManagementView::new(
        catalog, profiles, active, false,
    ))
}

/// Saves one reducer edit through the application boundary. New and changed profiles are first
/// persisted as Draft so validation can bind its evidence to an immutable revision. A supplied
/// API key moves directly into the protected credential command and is never retained by the
/// screen, view, or retry closure.
async fn save_provider_draft(
    service: &dyn AgentServiceApi,
    command: super::provider_management::ProviderEditCommand,
    secret: Option<ys_agent_core::SecretValue>,
    operation_id: OperationId,
    resume_step: ProviderManagementStep,
) -> ys_agent_core::ProviderResult<ys_agent_core::ProfileDetail> {
    let required_kind = command.provider.required_credential_kind();
    let selected_kind = match command.authentication {
        super::provider_management::ProviderAuthentication::ApiKey => CredentialKind::ApiKey,
        super::provider_management::ProviderAuthentication::OAuth => {
            CredentialKind::OAuthConnection
        }
    };
    if selected_kind != required_kind {
        return Err(provider_edit_error(
            ProviderErrorCode::AuthenticationInvalid,
            ProviderField::Credential,
        ));
    }

    // Validation already committed the exact revision used by activation. Its later Save Draft
    // button is an explicit acknowledgement, not a second write that would invalidate evidence.
    if resume_step == ProviderManagementStep::SaveActivate
        && secret.is_none()
        && let Some(profile_id) = command.profile_id
    {
        return service.provider_load_profile(profile_id).await;
    }

    let existing = match command.profile_id {
        Some(profile_id) => Some(service.provider_load_profile(profile_id).await?),
        None => None,
    };
    let profile_id = existing
        .as_ref()
        .map(|detail| detail.summary.profile_id)
        .unwrap_or_else(ProfileId::new);
    let expected_current_revision = existing.as_ref().map(|detail| detail.revision);
    let revision_number = expected_current_revision
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| provider_edit_error(ProviderErrorCode::Internal, ProviderField::Provider))?;
    let carried_generation = existing.as_ref().and_then(|detail| {
        (detail.summary.provider == command.provider)
            .then_some(detail.credential_generation)
            .flatten()
    });
    let name = ProfileName::new(command.name).map_err(|_| {
        provider_edit_error(
            ProviderErrorCode::ProfileNameConflict,
            ProviderField::ProfileName,
        )
    })?;
    let revision = ProfileRevision::draft(
        profile_id,
        revision_number,
        command.provider,
        command.model,
        command.parameters,
        carried_generation,
    )
    .map_err(|_| {
        provider_edit_error(ProviderErrorCode::InvalidModelPrefix, ProviderField::Model)
    })?;
    let saved = service
        .provider_save_profile(SaveProfileRequest {
            operation_id,
            revision: SaveProfileRevision {
                precondition: ys_agent_core::RevisionPrecondition {
                    profile_id,
                    expected_current_revision,
                },
                name,
                revision,
            },
        })
        .await?;

    let Some(secret) = secret else {
        return Ok(saved);
    };
    if required_kind != CredentialKind::ApiKey {
        return Err(provider_edit_error(
            ProviderErrorCode::OAuthNotConnected,
            ProviderField::OAuth,
        ));
    }
    let old_generation = saved.credential_generation;
    let generation_number = old_generation
        .map(CredentialGeneration::number)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            provider_edit_error(ProviderErrorCode::Internal, ProviderField::Credential)
        })?;
    let generation =
        CredentialGeneration::new(profile_id, generation_number, CredentialKind::ApiKey).map_err(
            |_| provider_edit_error(ProviderErrorCode::Internal, ProviderField::Credential),
        )?;
    let intent = match old_generation {
        Some(old_generation) => CredentialMutationIntent::replace(
            operation_id,
            profile_id,
            saved.revision,
            old_generation,
            generation,
        ),
        None => {
            CredentialMutationIntent::create(operation_id, profile_id, saved.revision, generation)
        }
    }
    .map_err(|_| provider_edit_error(ProviderErrorCode::Internal, ProviderField::Credential))?;
    service
        .provider_mutate_credential(CredentialMutationRequest {
            intent,
            mutation: CredentialMutation::Replace(ProtectedCredentialWrite {
                reference: ProviderCredentialReference {
                    profile_id,
                    generation,
                },
                secret,
            }),
        })
        .await
}

fn provider_edit_error(code: ProviderErrorCode, field: ProviderField) -> ProviderManagementError {
    ProviderManagementError::new(code, Some(field), ProviderRemediation::ReturnToEdit)
}

fn provider_to_core(error: ys_agent_core::ProviderManagementError) -> ys_agent_core::CoreError {
    ys_agent_core::CoreError::validation(error.code(), error.code())
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

#[cfg(test)]
mod provider_management_tests {
    use std::sync::Arc;

    use ys_agent_adapters::{
        credential::keyring::InMemoryCredentialVault,
        model::{discovery::LiterModelDiscovery, liter::LiterProviderFactory},
    };
    use ys_agent_core::{
        CredentialKind, ProfileId, ProfileName, ProfileRevision, ProviderCatalogView, ProviderId,
        ProviderProfileRepository, ProviderSupportStatus, RevisionPrecondition,
        RunProviderBindingRepository, SaveProfileRevision, WorkspaceId,
    };
    use ys_agent_runtime::{
        InProcessAgentService, NoopRunScheduler,
        provider::{
            api::InProcessProviderManagementApi,
            catalog::GovernedProviderCatalog,
            service::{CredentialService, ProviderManagementService},
        },
    };
    use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

    use super::*;

    fn catalog_views() -> Vec<ProviderCatalogView> {
        ProviderId::ALL
            .into_iter()
            .map(|provider| ProviderCatalogView {
                provider,
                display_name: format!("{provider:?}"),
                credential_kind: provider.required_credential_kind(),
                support_status: ProviderSupportStatus::Candidate,
                evidence_gaps: vec!["evidence_pending".to_owned()],
            })
            .collect()
    }

    #[tokio::test]
    async fn providers_and_legacy_model_use_one_masked_service_route() {
        let directory = tempfile::tempdir().expect("temporary runtime directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("open runtime store"),
        );
        let profile_id = ProfileId::new();
        let repository = store.provider_repository();
        repository
            .save_revision(SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id,
                    expected_current_revision: None,
                },
                name: ProfileName::new("TUI managed Profile").expect("valid Profile name"),
                revision: ProfileRevision::draft(
                    profile_id,
                    1,
                    ProviderId::DeepSeek,
                    ys_agent_core::ProviderModelId::new(ProviderId::DeepSeek, "deepseek/tui")
                        .expect("valid governed model"),
                    ys_agent_core::ProviderParameters::default(),
                    None,
                )
                .expect("valid Draft"),
            })
            .await
            .expect("persist Draft");
        let profiles: Arc<dyn ProviderProfileRepository> = Arc::new(repository);
        let run_bindings: Arc<dyn RunProviderBindingRepository> =
            Arc::new(store.run_binding_repository());
        let vault = Arc::new(InMemoryCredentialVault::new());
        let lifecycle = Arc::new(ProviderManagementService::new(profiles.clone()));
        let credentials = Arc::new(CredentialService::new(
            profiles.clone(),
            run_bindings.clone(),
            vault.clone(),
        ));
        let provider_api = Arc::new(InProcessProviderManagementApi::new(
            GovernedProviderCatalog::default(),
            catalog_views(),
            profiles,
            vault,
            run_bindings,
            lifecycle,
            credentials,
            Arc::new(LiterModelDiscovery::new()),
            Arc::new(LiterProviderFactory::new()),
        ));
        let workspace_id = WorkspaceId::new();
        let artifacts = Arc::new(
            LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
        );
        let service = Arc::new(
            InProcessAgentService::new(workspace_id, store, artifacts, Arc::new(NoopRunScheduler))
                .with_provider_management_api(provider_api),
        );
        let principal = Principal::local_operator("tui-provider-test");
        let mut controller = TuiController::new(service, workspace_id, principal.clone());
        let mut app = TuiApp::for_principal(principal);

        controller
            .apply(&mut app, InputAction::Providers)
            .await
            .expect("Provider manager opens through AgentServiceApi");
        assert_eq!(
            app.transient,
            Some(TransientView::Detail(DetailKind::Providers))
        );
        assert!(
            app.detail
                .as_ref()
                .expect("Provider detail")
                .lines
                .iter()
                .any(|line| line.contains("TUI managed Profile"))
        );

        controller
            .apply(&mut app, InputAction::Model)
            .await
            .expect("legacy model command reuses Provider manager");
        let detail = app.detail.expect("same Provider detail");
        assert_eq!(detail.title, "Provider management");
        assert!(
            detail
                .lines
                .iter()
                .any(|line| line.contains("Editing step · Model"))
        );
        assert!(!format!("{detail:?}").contains("api_key"));
        assert_eq!(
            CredentialKind::ApiKey,
            ProviderId::DeepSeek.required_credential_kind()
        );
    }
}
