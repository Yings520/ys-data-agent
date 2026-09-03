use std::{collections::HashMap, future::Future, io, path::PathBuf, sync::Arc, time::Duration};

use crossterm::{
    cursor::{Hide, SetCursorStyle, Show},
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use ratatui_textarea::Input;
use tokio::{
    sync::{Semaphore, watch},
    task::{Id as TokioTaskId, JoinSet},
    time,
};
use ys_agent_core::{
    CoreError, CoreResult, OperationId, ProviderErrorCode, ProviderManagementError,
    ProviderRemediation, ProviderResult, ProviderRetryability,
};

use crate::bootstrap::AppDependencies;

use super::{
    AsyncOperationBusy, DisplayContextRefreshTrigger, RouteKey, TranscriptItem, TransientView,
    TuiApp, TuiController, UiPreferenceStore, UiPreferences, parse_input,
    provider_management::{
        ProviderAuthentication, ProviderManagementStateKind, ProviderOperationKind,
    },
    render,
};

type RealTerminal = Terminal<CrosstermBackend<io::Stdout>>;

const MAX_PROVIDER_OPERATION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PROVIDER_OPERATION_RETRIES: u8 = 2;

/// Bounded timing policy supplied from already-validated Provider parameters. The registry also
/// validates it defensively before it can schedule any network, OAuth, Vault, or probe work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderOperationPolicy {
    timeout: Duration,
    max_retries: u8,
}

impl ProviderOperationPolicy {
    pub fn new(timeout: Duration, max_retries: u8) -> CoreResult<Self> {
        if timeout.is_zero() || timeout > MAX_PROVIDER_OPERATION_TIMEOUT {
            return Err(CoreError::validation(
                "invalid_provider_operation_timeout",
                "Provider operation timeout must be between one millisecond and five minutes",
            ));
        }
        if max_retries > MAX_PROVIDER_OPERATION_RETRIES {
            return Err(CoreError::validation(
                "invalid_provider_operation_retries",
                "Provider operation retries exceed the approved bound",
            ));
        }
        Ok(Self {
            timeout,
            max_retries,
        })
    }

    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    pub const fn max_retries(self) -> u8 {
        self.max_retries
    }
}

/// Cooperative cancellation signal passed to a spawned Provider operation. It has no operation
/// payload and cannot reveal a secret; the wrapper also drops the in-flight future on cancel.
#[derive(Clone)]
pub struct ProviderOperationCancellation {
    cancelled: watch::Receiver<bool>,
}

impl ProviderOperationCancellation {
    pub async fn cancelled(&mut self) {
        if !*self.cancelled.borrow() {
            let _ = self.cancelled.changed().await;
        }
    }
}

#[derive(Debug)]
pub struct ProviderOperationCompletion<T> {
    pub operation_id: OperationId,
    pub route_key: RouteKey,
    pub kind: ProviderOperationKind,
    pub attempts: u8,
    pub result: ProviderResult<T>,
}

struct OperationControl {
    cancel: watch::Sender<bool>,
    cancelled: bool,
    kind: ProviderOperationKind,
    route_key: RouteKey,
}

enum OperationExit<T> {
    Completed(ProviderOperationCompletion<T>),
    Cancelled(OperationId),
}

/// Owns Provider-operation lifetimes for the TUI event loop. All work is spawned and globally
/// serialized, so a discovery/probe burst cannot block rendering or multiply Provider cost. A
/// service-facing integration may start a new operation only after the prior completion is
/// observed; durable save/activation retries remain explicit and journal-protected.
pub struct AsyncOperationRegistry<T> {
    policy: ProviderOperationPolicy,
    gate: Arc<Semaphore>,
    operations: HashMap<OperationId, OperationControl>,
    task_operations: HashMap<TokioTaskId, OperationId>,
    tasks: JoinSet<OperationExit<T>>,
}

impl<T> AsyncOperationRegistry<T>
where
    T: Send + 'static,
{
    pub fn new(policy: ProviderOperationPolicy) -> Self {
        Self {
            policy,
            // A single Provider operation is intentional: no parallel model probing, OAuth, or
            // Vault work is needed for one interactive TUI screen.
            gate: Arc::new(Semaphore::new(1)),
            operations: HashMap::new(),
            task_operations: HashMap::new(),
            tasks: JoinSet::new(),
        }
    }

    /// Allocates a fresh ID and schedules work without awaiting it on the render loop.
    pub fn start<F, Fut>(&mut self, kind: ProviderOperationKind, operation: F) -> OperationId
    where
        F: FnMut(OperationId, ProviderOperationCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = ProviderResult<T>> + Send + 'static,
    {
        self.schedule(kind, RouteKey::default(), operation)
    }

    /// Starts one route-bound Provider mutation. Production callers cannot queue a second
    /// mutation while the first remains active.
    pub fn start_on_route<F, Fut>(
        &mut self,
        kind: ProviderOperationKind,
        route_key: RouteKey,
        operation: F,
    ) -> Result<OperationId, AsyncOperationBusy>
    where
        F: FnMut(OperationId, ProviderOperationCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = ProviderResult<T>> + Send + 'static,
    {
        if self.active_count() > 0 {
            return Err(AsyncOperationBusy);
        }
        Ok(self.schedule(kind, route_key, operation))
    }

    fn schedule<F, Fut>(
        &mut self,
        kind: ProviderOperationKind,
        route_key: RouteKey,
        operation: F,
    ) -> OperationId
    where
        F: FnMut(OperationId, ProviderOperationCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = ProviderResult<T>> + Send + 'static,
    {
        let operation_id = OperationId::new();
        let (cancel, receiver) = watch::channel(false);
        self.operations.insert(
            operation_id,
            OperationControl {
                cancel,
                cancelled: false,
                kind,
                route_key,
            },
        );
        let task = self.tasks.spawn(run_provider_operation(
            operation_id,
            route_key,
            kind,
            self.policy,
            self.gate.clone(),
            ProviderOperationCancellation {
                cancelled: receiver,
            },
            operation,
        ));
        self.task_operations.insert(task.id(), operation_id);
        operation_id
    }

    /// Marks an unfinished operation cancelled immediately. Its wrapper observes the signal and
    /// drops the future; any completion already racing with Esc is discarded by `next_completion`.
    pub fn cancel(&mut self, operation_id: OperationId) -> bool {
        let Some(control) = self.operations.get_mut(&operation_id) else {
            return false;
        };
        if control.cancelled {
            return false;
        }
        control.cancelled = true;
        let _ = control.cancel.send(true);
        true
    }

    pub fn active_count(&self) -> usize {
        self.operations
            .values()
            .filter(|control| !control.cancelled)
            .count()
    }

    /// Delivers only the latest non-cancelled result. Cancellation/late completion consumes its
    /// task slot without exposing a state change to the reducer.
    pub async fn next_completion(&mut self) -> Option<ProviderOperationCompletion<T>> {
        while let Some(joined) = self.tasks.join_next_with_id().await {
            let (task_id, exit) = match joined {
                Ok(value) => value,
                Err(error) => {
                    let Some(operation_id) = self.task_operations.remove(&error.id()) else {
                        continue;
                    };
                    let Some(control) = self.operations.remove(&operation_id) else {
                        continue;
                    };
                    if control.cancelled {
                        continue;
                    }
                    return Some(ProviderOperationCompletion {
                        operation_id,
                        route_key: control.route_key,
                        kind: control.kind,
                        attempts: 0,
                        result: Err(internal_operation_error()),
                    });
                }
            };
            self.task_operations.remove(&task_id);
            let operation_id = match &exit {
                OperationExit::Completed(completion) => completion.operation_id,
                OperationExit::Cancelled(operation_id) => *operation_id,
            };
            let cancelled = self
                .operations
                .remove(&operation_id)
                .is_none_or(|control| control.cancelled);
            if cancelled {
                continue;
            }
            if let OperationExit::Completed(completion) = exit {
                return Some(completion);
            }
        }
        None
    }

    /// Returns one completed operation without awaiting. The terminal loop uses this polling
    /// form so the service-event subscription remains the sole mutable asynchronous borrow of
    /// the controller inside `tokio::select!`.
    pub fn try_next_completion(&mut self) -> Option<ProviderOperationCompletion<T>> {
        while let Some(joined) = self.tasks.try_join_next_with_id() {
            let (task_id, exit) = match joined {
                Ok(value) => value,
                Err(error) => {
                    let Some(operation_id) = self.task_operations.remove(&error.id()) else {
                        continue;
                    };
                    let Some(control) = self.operations.remove(&operation_id) else {
                        continue;
                    };
                    if control.cancelled {
                        continue;
                    }
                    return Some(ProviderOperationCompletion {
                        operation_id,
                        route_key: control.route_key,
                        kind: control.kind,
                        attempts: 0,
                        result: Err(internal_operation_error()),
                    });
                }
            };
            self.task_operations.remove(&task_id);
            let operation_id = match &exit {
                OperationExit::Completed(completion) => completion.operation_id,
                OperationExit::Cancelled(operation_id) => *operation_id,
            };
            let cancelled = self
                .operations
                .remove(&operation_id)
                .is_none_or(|control| control.cancelled);
            if cancelled {
                continue;
            }
            if let OperationExit::Completed(completion) = exit {
                return Some(completion);
            }
        }
        None
    }
}

async fn run_provider_operation<T, F, Fut>(
    operation_id: OperationId,
    route_key: RouteKey,
    kind: ProviderOperationKind,
    policy: ProviderOperationPolicy,
    gate: Arc<Semaphore>,
    mut cancellation: ProviderOperationCancellation,
    mut operation: F,
) -> OperationExit<T>
where
    T: Send + 'static,
    F: FnMut(OperationId, ProviderOperationCancellation) -> Fut + Send + 'static,
    Fut: Future<Output = ProviderResult<T>> + Send + 'static,
{
    let permit = tokio::select! {
        _ = cancellation.cancelled() => return OperationExit::Cancelled(operation_id),
        permit = gate.acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => return OperationExit::Completed(ProviderOperationCompletion {
                operation_id,
                route_key,
                kind,
                attempts: 0,
                result: Err(internal_operation_error()),
            }),
        },
    };
    let _permit = permit;
    let mut attempts = 0_u8;
    loop {
        attempts = attempts.saturating_add(1);
        let operation_cancellation = cancellation.clone();
        let result = tokio::select! {
            _ = cancellation.cancelled() => return OperationExit::Cancelled(operation_id),
            result = time::timeout(policy.timeout(), operation(operation_id, operation_cancellation)) => result,
        };
        match result {
            Ok(Ok(value)) => {
                return OperationExit::Completed(ProviderOperationCompletion {
                    operation_id,
                    route_key,
                    kind,
                    attempts,
                    result: Ok(value),
                });
            }
            Ok(Err(error)) if retries_are_safe(kind, &error, attempts, policy) => continue,
            Ok(Err(error)) => {
                return OperationExit::Completed(ProviderOperationCompletion {
                    operation_id,
                    route_key,
                    kind,
                    attempts,
                    result: Err(error),
                });
            }
            Err(_) if retries_are_safe(kind, &timeout_error(), attempts, policy) => continue,
            Err(_) => {
                return OperationExit::Completed(ProviderOperationCompletion {
                    operation_id,
                    route_key,
                    kind,
                    attempts,
                    result: Err(timeout_error()),
                });
            }
        }
    }
}

fn retries_are_safe(
    kind: ProviderOperationKind,
    error: &ProviderManagementError,
    attempts: u8,
    policy: ProviderOperationPolicy,
) -> bool {
    matches!(
        kind,
        ProviderOperationKind::DiscoverModels | ProviderOperationKind::Validate
    ) && attempts <= policy.max_retries()
        && error.retryability() == ProviderRetryability::Bounded
}

fn timeout_error() -> ProviderManagementError {
    ProviderManagementError::new(ProviderErrorCode::Timeout, None, ProviderRemediation::Retry)
}

fn internal_operation_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        None,
        ProviderRemediation::ContactSupport,
    )
}

pub struct TerminalGuard {
    terminal: RealTerminal,
    modes: TerminalModes,
}

#[derive(Debug, Default)]
struct TerminalModes {
    raw: bool,
    alternate_screen: bool,
    bracketed_paste: bool,
    cursor_hidden: bool,
    cursor_shape_changed: bool,
    focus_events: bool,
    mouse_capture: bool,
}

impl TerminalGuard {
    fn enter(mouse_enabled: bool) -> io::Result<Self> {
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        let mut guard = Self {
            terminal,
            modes: TerminalModes::default(),
        };
        if let Err(error) = guard.enable_modes(mouse_enabled) {
            guard.restore();
            return Err(error);
        }
        Ok(guard)
    }

    fn enable_modes(&mut self, mouse_enabled: bool) -> io::Result<()> {
        enable_raw_mode()?;
        self.modes.raw = true;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.modes.alternate_screen = true;
        execute!(self.terminal.backend_mut(), EnableBracketedPaste)?;
        self.modes.bracketed_paste = true;
        execute!(self.terminal.backend_mut(), EnableFocusChange)?;
        self.modes.focus_events = true;
        if mouse_enabled {
            execute!(self.terminal.backend_mut(), EnableMouseCapture)?;
            self.modes.mouse_capture = true;
        }
        execute!(self.terminal.backend_mut(), SetCursorStyle::BlinkingBar)?;
        self.modes.cursor_shape_changed = true;
        execute!(self.terminal.backend_mut(), Hide)?;
        self.modes.cursor_hidden = true;
        Ok(())
    }

    fn draw(&mut self, app: &TuiApp) -> io::Result<()> {
        self.terminal.draw(|frame| render(frame, app)).map(|_| ())
    }

    fn restore(&mut self) {
        if self.modes.mouse_capture {
            let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
            self.modes.mouse_capture = false;
        }
        if self.modes.focus_events {
            let _ = execute!(self.terminal.backend_mut(), DisableFocusChange);
            self.modes.focus_events = false;
        }
        if self.modes.bracketed_paste {
            let _ = execute!(self.terminal.backend_mut(), DisableBracketedPaste);
            self.modes.bracketed_paste = false;
        }
        if self.modes.cursor_hidden {
            let _ = execute!(self.terminal.backend_mut(), Show);
            self.modes.cursor_hidden = false;
        }
        if self.modes.cursor_shape_changed {
            let _ = execute!(
                self.terminal.backend_mut(),
                SetCursorStyle::DefaultUserShape
            );
            self.modes.cursor_shape_changed = false;
        }
        if self.modes.alternate_screen {
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            self.modes.alternate_screen = false;
        }
        if self.modes.raw {
            let _ = disable_raw_mode();
            self.modes.raw = false;
        }
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub async fn run_tui(dependencies: AppDependencies) -> CoreResult<()> {
    let mut app = TuiApp::for_principal(dependencies.principal.clone());
    app.workspace_name = dependencies.display.workspace_name;
    app.model_label = dependencies.display.model_label;
    app.connection_label = dependencies.display.connection_label;
    app.permission_label = dependencies.display.permission_label;

    let preference_store = UiPreferenceStore::new(PathBuf::from(".ysda/ui.toml"));
    let no_color = std::env::var_os("NO_COLOR").is_some();
    match preference_store.load() {
        Ok(preferences) => app.apply_preferences(&preferences, no_color),
        Err(error) => {
            app.apply_preferences(&UiPreferences::default(), no_color);
            app.safe_warning = Some(error.code().to_owned());
        }
    }
    app.mouse_enabled = std::env::var("YSDA_TUI_MOUSE").is_ok_and(|value| value == "1");

    let mut controller = TuiController::new(
        dependencies.service,
        dependencies.workspace_id,
        dependencies.principal,
    );
    controller.request_display_context_refresh(&app, DisplayContextRefreshTrigger::Startup);
    let report = controller.doctor().await?;
    app.transient = (!report.allows_query_submission()).then_some(TransientView::Repair);
    app.doctor_report = Some(report);

    let mut terminal = TerminalGuard::enter(app.mouse_enabled).map_err(terminal_error)?;
    let mut events = EventStream::new();
    let mut dirty = true;

    loop {
        if let Some(preferences) = app.pending_preferences.take()
            && let Err(error) = preference_store.persist(&preferences)
        {
            app.safe_warning = Some(error.code().to_owned());
            dirty = true;
        }
        if let Some(completion) = controller.take_ready_submission().await {
            match completion {
                Ok(completion) => controller.complete_submission(&mut app, completion),
                Err(error) => {
                    app.runtime_status = None;
                    app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                }
            }
            controller.request_display_context_refresh(
                &app,
                DisplayContextRefreshTrigger::QueryStateChanged,
            );
            dirty = true;
        }
        if let Some(completion) = controller.take_ready_provider_operation() {
            controller.apply_provider_operation(&mut app, completion);
            controller.request_display_context_refresh(
                &app,
                DisplayContextRefreshTrigger::ProviderOperationCompleted,
            );
            dirty = true;
        }
        if controller.apply_ready_display_context(&mut app) {
            dirty = true;
        }
        if app.should_quit {
            return Ok(());
        }
        if dirty {
            terminal.draw(&app).map_err(terminal_error)?;
            dirty = false;
        }
        let tick = if app.runtime_status.is_some() || !app.composer.text().is_empty() {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(250)
        };
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(event)) => {
                    if handle_terminal_event(&mut app, &mut controller, event).await? { return Ok(()); }
                    dirty = true;
                }
                Some(Err(error)) => return Err(terminal_error(error)),
                None => return Ok(()),
            },
            service_event = controller.next_service_event() => {
                let event = service_event?;
                controller.apply_service_event(&mut app, event);
                controller.reload_durable_state(&mut app).await?;
                controller.request_display_context_refresh(
                    &app,
                    DisplayContextRefreshTrigger::QueryStateChanged,
                );
                dirty = true;
            },
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(terminal_error)?;
                return Ok(());
            },
            _ = time::sleep(tick) => {
                if app.runtime_status.is_some() { dirty = true; }
            }
        }
    }
}

async fn handle_terminal_event(
    app: &mut TuiApp,
    controller: &mut TuiController,
    event: Event,
) -> CoreResult<bool> {
    match event {
        Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
        Event::Paste(text) => {
            app.composer.insert_paste(&text);
            app.sync_slash_palette();
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(true);
            }
            if key.code == KeyCode::Esc {
                if app.transient == Some(TransientView::Detail(super::DetailKind::Providers)) {
                    if controller.provider_operation_in_flight() {
                        controller.cancel_provider_operation(app).await;
                    } else {
                        controller.close_provider_management(app);
                    }
                    return Ok(false);
                }
                app.close_transient();
                return Ok(false);
            }
            if app.transient == Some(TransientView::ThemePicker) {
                handle_theme_picker_key(app, key)?;
                return Ok(false);
            }
            if app.transient == Some(TransientView::ModePicker) {
                handle_mode_picker_key(app, key);
                return Ok(false);
            }
            if app.transient == Some(TransientView::SlashPalette) {
                handle_palette_key(app, controller, key).await?;
                return Ok(false);
            }
            if app.transient == Some(TransientView::Detail(super::DetailKind::Providers)) {
                let provider_view = controller.provider_screen_view();
                let step = provider_view.as_ref().and_then(|view| view.step);
                if key.code == KeyCode::Enter {
                    let _ = controller.advance_provider_step(app);
                    return Ok(false);
                }
                if key.code == KeyCode::Backspace {
                    let _ = controller.delete_provider_text(app);
                    return Ok(false);
                }
                if key.code == KeyCode::Char('n')
                    && provider_view
                        .as_ref()
                        .is_some_and(|view| view.state == ProviderManagementStateKind::Browse)
                {
                    let _ = controller.start_provider_draft(app);
                    return Ok(false);
                }
                if let KeyCode::Char(digit @ '1'..='9') = key.code
                    && step == Some(super::provider_management::ProviderManagementStep::Provider)
                {
                    let index = usize::from((digit as u8).saturating_sub(b'1'));
                    if let Some(provider) = ys_agent_core::ProviderId::ALL.get(index).copied() {
                        let _ = controller.select_provider_for_draft(app, provider);
                    }
                    return Ok(false);
                }
                if key.code == KeyCode::Char('k')
                    && step
                        == Some(super::provider_management::ProviderManagementStep::Authentication)
                {
                    let _ = controller
                        .select_provider_authentication(app, ProviderAuthentication::ApiKey);
                    return Ok(false);
                }
                if key.code == KeyCode::Char('o')
                    && step
                        == Some(super::provider_management::ProviderManagementStep::Authentication)
                {
                    let selected = provider_view
                        .as_ref()
                        .and_then(|view| view.edit.as_ref())
                        .and_then(|edit| edit.authentication);
                    if selected == Some(ProviderAuthentication::OAuth) {
                        if let Err(error) =
                            controller.start_provider_operation(ProviderOperationKind::OAuth)
                        {
                            app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                        }
                    } else {
                        let _ = controller
                            .select_provider_authentication(app, ProviderAuthentication::OAuth);
                    }
                    return Ok(false);
                }
                let operation = match key.code {
                    KeyCode::Char('D')
                        if step
                            == Some(super::provider_management::ProviderManagementStep::Model) =>
                    {
                        Some(ProviderOperationKind::DiscoverModels)
                    }
                    KeyCode::Char('v')
                        if step
                            == Some(super::provider_management::ProviderManagementStep::Validate) =>
                    {
                        Some(ProviderOperationKind::Validate)
                    }
                    KeyCode::Char('s')
                        if matches!(
                            step,
                            Some(
                                super::provider_management::ProviderManagementStep::Validate
                                    | super::provider_management::ProviderManagementStep::SaveActivate
                            )
                        ) =>
                    {
                        Some(ProviderOperationKind::SaveDraft)
                    }
                    _ => None,
                };
                if key.code == KeyCode::Char('a')
                    && step
                        == Some(super::provider_management::ProviderManagementStep::SaveActivate)
                {
                    if let Err(error) = controller.request_provider_activation() {
                        app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                    }
                    return Ok(false);
                }
                if key.code == KeyCode::Char('r')
                    && provider_view
                        .as_ref()
                        .is_some_and(|view| view.state == ProviderManagementStateKind::Result)
                {
                    if let Err(error) = controller.retry_provider_operation() {
                        app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                    }
                    return Ok(false);
                }
                if let Some(operation) = operation {
                    if let Err(error) = controller.start_provider_operation(operation) {
                        app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                    }
                    return Ok(false);
                }
                if let KeyCode::Char(character) = key.code {
                    let _ = controller.append_provider_text(app, character);
                    return Ok(false);
                }
            }
            if key.code == KeyCode::Char('r') && app.display_context_unavailable() {
                controller
                    .request_display_context_refresh(app, DisplayContextRefreshTrigger::UserRetry);
                return Ok(false);
            }
            match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => submit_composer(app, controller).await?,
                (KeyCode::Up, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                    app.composer.history_up()
                }
                (KeyCode::Down, modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                    app.composer.history_down()
                }
                (KeyCode::Char('z'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                    app.composer.undo()
                }
                (KeyCode::Char('y'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                    app.composer.redo()
                }
                _ => {
                    app.composer.handle_input(Input::from(key));
                    app.sync_slash_palette();
                }
            }
        }
        Event::Mouse(mouse)
            if app.mouse_enabled
                && app.transient == Some(TransientView::SlashPalette)
                && mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
        {
            let (_, terminal_height) = crossterm::terminal::size().map_err(terminal_error)?;
            let panel_height = 10_u16.min(terminal_height.saturating_sub(2));
            let first_result_row = terminal_height
                .saturating_sub(panel_height)
                .saturating_add(2);
            let row = usize::from(mouse.row.saturating_sub(first_result_row));
            app.slash_palette.select_visible_row(row);
        }
        Event::Mouse(mouse) if app.transient == Some(TransientView::SlashPalette) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => app.slash_palette.move_up(),
                MouseEventKind::ScrollDown => app.slash_palette.move_down(),
                _ => {}
            }
        }
        Event::Mouse(mouse) if app.transient == Some(TransientView::ThemePicker) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    app.theme_selected = app.theme_selected.saturating_sub(1)
                }
                MouseEventKind::ScrollDown => {
                    app.theme_selected =
                        (app.theme_selected + 1).min(app.theme_names.len().saturating_sub(1))
                }
                _ => {}
            }
            preview_theme(app)?;
        }
        Event::Mouse(_) | Event::Key(_) => {}
    }
    Ok(false)
}

async fn handle_palette_key(
    app: &mut TuiApp,
    controller: &mut TuiController,
    key: KeyEvent,
) -> CoreResult<()> {
    match key.code {
        KeyCode::Up => app.slash_palette.move_up(),
        KeyCode::Down => app.slash_palette.move_down(),
        KeyCode::PageUp => app.slash_palette.page_up(),
        KeyCode::PageDown => app.slash_palette.page_down(),
        KeyCode::Tab | KeyCode::Enter => {
            let Some((command, requires_arguments)) = app.slash_palette.completion() else {
                return Ok(());
            };
            app.slash_palette.clear();
            let palette_draft = app.palette_draft.take().unwrap_or_default();
            app.transient = None;
            if requires_arguments {
                app.composer.set_text(&format!("{command} "));
            } else if key.code == KeyCode::Tab {
                app.composer.set_text(&command);
            } else {
                app.composer.set_text(&palette_draft);
                let action = parse_input(&command).map_err(|error| {
                    CoreError::validation("invalid_slash_command", error.to_string())
                })?;
                if let Err(error) = controller.apply(app, action).await {
                    app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
                }
            }
        }
        _ => {
            app.composer.handle_input(Input::from(key));
            app.sync_slash_palette();
        }
    }
    Ok(())
}

fn handle_mode_picker_key(app: &mut TuiApp, key: KeyEvent) {
    let action = match key.code {
        KeyCode::Up => Some(super::ModePickerAction::MoveUp),
        KeyCode::Down => Some(super::ModePickerAction::MoveDown),
        KeyCode::Enter => Some(super::ModePickerAction::Confirm),
        KeyCode::Backspace => Some(super::ModePickerAction::Backspace),
        KeyCode::Char(character) if key.modifiers.is_empty() => {
            Some(super::ModePickerAction::Insert(character))
        }
        _ => None,
    };
    let Some(action) = action else {
        return;
    };
    let Some(picker) = app.mode_picker.as_mut() else {
        app.transient = None;
        return;
    };
    if let super::ModePickerOutcome::Confirmed(mode) = picker.reduce(action) {
        app.query_mode = mode;
        app.mode_picker = None;
        app.transient = app.mode_picker_return.take();
    }
}

fn handle_theme_picker_key(app: &mut TuiApp, key: KeyEvent) -> CoreResult<()> {
    match key.code {
        KeyCode::Up => app.theme_selected = app.theme_selected.saturating_sub(1),
        KeyCode::Down => {
            app.theme_selected =
                (app.theme_selected + 1).min(app.theme_names.len().saturating_sub(1))
        }
        KeyCode::Enter => {
            let preferences = UiPreferences {
                theme: app.theme_names[app.theme_selected].clone(),
                colors: Default::default(),
            };
            app.active_theme = app
                .theme_registry
                .resolve_preferences(&preferences, app.no_color)
                .map_err(|error| CoreError::validation(error.code(), error.to_string()))?;
            app.preferences = preferences.clone();
            app.pending_preferences = Some(preferences);
            app.preview_theme = None;
            app.transient = None;
            return Ok(());
        }
        _ => return Ok(()),
    }
    preview_theme(app)
}

fn preview_theme(app: &mut TuiApp) -> CoreResult<()> {
    let preferences = UiPreferences {
        theme: app.theme_names[app.theme_selected].clone(),
        colors: Default::default(),
    };
    app.preview_theme = Some(
        app.theme_registry
            .resolve_preferences(&preferences, app.no_color)
            .map_err(|error| CoreError::validation(error.code(), error.to_string()))?,
    );
    Ok(())
}

async fn submit_composer(app: &mut TuiApp, controller: &mut TuiController) -> CoreResult<()> {
    let raw = app.composer.text();
    match parse_input(&raw) {
        Ok(action) => {
            if controller.submission_in_flight() && action != super::InputAction::Quit {
                app.push_transcript(TranscriptItem::Warning(
                    "A request is already in progress; your draft was kept".to_owned(),
                ));
                return Ok(());
            }
            app.composer.submit();
            if let Err(error) = controller.apply(app, action).await {
                app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
            }
        }
        Err(error) => {
            app.push_transcript(TranscriptItem::Warning(error.to_string()));
        }
    }
    Ok(())
}

fn terminal_error(error: io::Error) -> CoreError {
    CoreError::Storage {
        message: format!("terminal I/O failed: {error}"),
    }
}

fn user_readable_error(error: &CoreError) -> String {
    format!(
        "What happened: {}. Retry safety and required action are recorded in the Run evidence.",
        error.code()
    )
}

#[cfg(test)]
mod tests {
    use std::{
        future,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;
    use tokio::sync::{Notify, Semaphore, oneshot};
    use ys_agent_adapters::model::FakeModelProvider;
    use ys_agent_core::{
        ActivateProfileRequest, ActiveProviderView, AgentAction, CompatibilityEvidenceView,
        CredentialMutationRequest, CredentialViewStatus, DeleteProfileRequest,
        DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel, ModelResponse,
        OAuthConnectionView, OperationId, Principal, ProfileDetail, ProfileId, ProfileName,
        ProfileState, ProfileSummary, ProviderCatalogView, ProviderDoctorView, ProviderErrorCode,
        ProviderManagementApi, ProviderManagementError, ProviderRemediation, ProviderResult,
        ProviderSupportStatus, RemoteRevocationOutcome, SaveProfileRequest, ValidateProfileRequest,
        ValidationId, WorkspaceId,
    };
    use ys_agent_runtime::{
        InProcessAgentService, NoopRunScheduler, StaticRunProviderBindingSource,
        doctor::{DoctorReport, QueryCapability},
    };
    use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

    use crate::tui::{
        ContentRoute, InputAction, provider_management::ProviderOperationKind, render_to_string,
    };

    use super::{
        AsyncOperationRegistry, ProviderOperationPolicy, RouteKey, TuiApp, TuiController,
        handle_terminal_event, user_readable_error,
    };

    #[tokio::test]
    async fn provider_operations_start_without_blocking_the_tui_loop() {
        let mut registry: AsyncOperationRegistry<()> = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_secs(1), 0).expect("valid bounded policy"),
        );

        assert_eq!(registry.active_count(), 0);
        let operation_id = registry.start(ProviderOperationKind::DiscoverModels, |_, _| async {
            future::pending().await
        });
        assert_eq!(registry.active_count(), 1);
        tokio::time::timeout(Duration::from_millis(25), tokio::task::yield_now())
            .await
            .expect("the event loop remains able to yield while Provider work is pending");
        assert!(registry.cancel(operation_id));
        assert!(
            tokio::time::timeout(Duration::from_secs(1), registry.next_completion())
                .await
                .expect("cancelled task is reaped")
                .is_none()
        );
    }

    #[tokio::test]
    async fn route_bound_provider_work_is_single_flight_and_preserves_its_route_key() {
        let mut registry = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_secs(1), 0).expect("valid bounded policy"),
        );
        let route_key = RouteKey {
            route: ContentRoute::ProviderManagement,
            generation: 7,
        };
        let release = Arc::new(Notify::new());
        let pending_release = release.clone();
        let operation_id = registry
            .start_on_route(ProviderOperationKind::Validate, route_key, move |_, _| {
                let pending_release = pending_release.clone();
                async move {
                    pending_release.notified().await;
                    Ok(1_u8)
                }
            })
            .expect("first Provider mutation starts");
        assert!(
            registry
                .start_on_route(ProviderOperationKind::Activate, route_key, |_, _| async {
                    Ok(2_u8)
                })
                .is_err(),
            "a second Provider mutation is rejected instead of queued"
        );
        release.notify_one();
        let completion = registry.next_completion().await.expect("completion");
        assert_eq!(completion.operation_id, operation_id);
        assert_eq!(completion.route_key, route_key);
    }

    #[tokio::test]
    async fn retries_are_bounded_for_safe_probe_work_and_not_for_durable_saves() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut registry: AsyncOperationRegistry<()> = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_secs(1), 2).expect("approved retry bound"),
        );
        let probe_attempts = attempts.clone();
        let probe_id = registry.start(ProviderOperationKind::Validate, move |_, _| {
            let probe_attempts = probe_attempts.clone();
            async move {
                probe_attempts.fetch_add(1, Ordering::SeqCst);
                Err(ProviderManagementError::new(
                    ProviderErrorCode::Network,
                    None,
                    ProviderRemediation::Retry,
                ))
            }
        });
        let probe = registry.next_completion().await.expect("probe completion");
        assert_eq!(probe.operation_id, probe_id);
        assert_eq!(probe.attempts, 3);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(
            probe.result.expect_err("network remains a failure").code(),
            "provider.network"
        );

        let save_attempts = Arc::new(AtomicUsize::new(0));
        let operation_attempts = save_attempts.clone();
        let save_id = registry.start(ProviderOperationKind::SaveDraft, move |_, _| {
            let operation_attempts = operation_attempts.clone();
            async move {
                operation_attempts.fetch_add(1, Ordering::SeqCst);
                Err(ProviderManagementError::new(
                    ProviderErrorCode::Network,
                    None,
                    ProviderRemediation::Retry,
                ))
            }
        });
        let save = registry.next_completion().await.expect("save completion");
        assert_eq!(save.operation_id, save_id);
        assert_eq!(save.attempts, 1);
        assert_eq!(save_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_drops_late_result_and_unblocks_the_next_operation() {
        let mut registry = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_secs(1), 0).expect("valid bounded policy"),
        );
        let (started, started_rx) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let pending_release = release.clone();
        let mut started = Some(started);
        let cancelled_id = registry.start(ProviderOperationKind::DiscoverModels, move |_, _| {
            let started = started.take();
            let pending_release = pending_release.clone();
            async move {
                started
                    .expect("operation starts once")
                    .send(())
                    .expect("test receives start");
                pending_release.notified().await;
                Ok(1_u8)
            }
        });
        started_rx.await.expect("operation entered its future");
        assert!(registry.cancel(cancelled_id));
        release.notify_one();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), registry.next_completion())
                .await
                .expect("cancelled task is reaped")
                .is_none(),
            "a completion racing with cancellation must not update the reducer"
        );

        let next_id = registry.start(ProviderOperationKind::DiscoverModels, |_, _| async {
            Ok(2_u8)
        });
        let next = registry
            .next_completion()
            .await
            .expect("next operation completes");
        assert_eq!(next.operation_id, next_id);
        assert_eq!(next.result.expect("new result"), 2);
    }

    #[tokio::test]
    async fn registry_serializes_provider_work_and_turns_timeout_into_stable_failure() {
        let mut registry = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_secs(1), 0)
                .expect("valid timeout and retry bound"),
        );
        let (first_started, first_started_rx) = oneshot::channel();
        let (second_started, mut second_started_rx) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let first_release = release.clone();
        let mut first_started = Some(first_started);
        registry.start(ProviderOperationKind::Validate, move |_, _| {
            let first_started = first_started.take();
            let first_release = first_release.clone();
            async move {
                first_started
                    .expect("first operation starts once")
                    .send(())
                    .expect("test receives first start");
                first_release.notified().await;
                Ok(1_u8)
            }
        });
        let mut second_started = Some(second_started);
        registry.start(ProviderOperationKind::Validate, move |_, _| {
            let second_started = second_started.take();
            async move {
                second_started
                    .expect("second operation starts once")
                    .send(())
                    .expect("test receives second start");
                Ok(2_u8)
            }
        });
        first_started_rx.await.expect("first starts");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut second_started_rx)
                .await
                .is_err(),
            "a second probe may not start while the first owns the Provider gate"
        );
        release.notify_one();
        assert_eq!(
            registry
                .next_completion()
                .await
                .expect("first completion")
                .result
                .expect("first result"),
            1
        );
        assert_eq!(
            registry
                .next_completion()
                .await
                .expect("second completion")
                .result
                .expect("second result"),
            2
        );

        let mut timeout_registry: AsyncOperationRegistry<()> = AsyncOperationRegistry::new(
            ProviderOperationPolicy::new(Duration::from_millis(10), 1)
                .expect("valid timeout and retry bound"),
        );
        let timeout_id = timeout_registry.start(ProviderOperationKind::Validate, |_, _| async {
            future::pending().await
        });
        let timeout = timeout_registry
            .next_completion()
            .await
            .expect("timed out completion");
        assert_eq!(timeout.operation_id, timeout_id);
        assert_eq!(timeout.attempts, 2);
        assert_eq!(
            timeout.result.expect_err("stable timeout").code(),
            "provider.timeout"
        );
    }

    #[test]
    fn operation_policy_rejects_unbounded_timeout_and_retry_values() {
        assert!(ProviderOperationPolicy::new(Duration::ZERO, 0).is_err());
        assert!(ProviderOperationPolicy::new(Duration::from_secs(301), 0).is_err());
        assert!(ProviderOperationPolicy::new(Duration::from_secs(1), 3).is_err());
    }

    async fn take_submission(
        controller: &mut TuiController,
    ) -> ys_agent_core::CoreResult<super::super::app::SubmissionCompletion> {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(completion) = controller.take_ready_submission().await {
                    return completion;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("submission completion")
    }

    #[derive(Default)]
    struct KeyboardProviderState {
        profile: Option<ProfileDetail>,
        active: Option<ActiveProviderView>,
    }

    /// A service-boundary fake used only to exercise the TUI's complete keyboard workflow. It
    /// accepts no network or filesystem configuration and never inspects the moved secret value.
    #[derive(Default)]
    struct KeyboardProviderApi {
        state: Mutex<KeyboardProviderState>,
    }

    fn fake_provider_error() -> ProviderManagementError {
        ProviderManagementError::new(
            ProviderErrorCode::Internal,
            None,
            ProviderRemediation::ContactSupport,
        )
    }

    impl KeyboardProviderApi {
        fn profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
            self.state
                .lock()
                .expect("test state lock")
                .profile
                .clone()
                .filter(|detail| detail.summary.profile_id == profile_id)
                .ok_or_else(fake_provider_error)
        }
    }

    #[async_trait]
    impl ProviderManagementApi for KeyboardProviderApi {
        async fn catalog(&self) -> ProviderResult<Vec<ProviderCatalogView>> {
            Ok(ys_agent_core::ProviderId::ALL
                .into_iter()
                .map(|provider| ProviderCatalogView {
                    provider,
                    display_name: format!("{provider:?}"),
                    credential_kind: provider.required_credential_kind(),
                    support_status: ProviderSupportStatus::Candidate,
                    evidence_gaps: vec!["test_evidence_gap".to_owned()],
                })
                .collect())
        }

        async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
            Ok(self
                .state
                .lock()
                .expect("test state lock")
                .profile
                .as_ref()
                .map(|detail| vec![detail.summary.clone()])
                .unwrap_or_default())
        }

        async fn active_provider(&self) -> ProviderResult<Option<ActiveProviderView>> {
            Ok(self.state.lock().expect("test state lock").active.clone())
        }

        async fn load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
            self.profile(profile_id)
        }

        async fn save_profile(&self, request: SaveProfileRequest) -> ProviderResult<ProfileDetail> {
            let revision = request.revision.revision;
            let detail = ProfileDetail {
                summary: ProfileSummary {
                    profile_id: revision.profile_id(),
                    name: request.revision.name.as_str().to_owned(),
                    provider: revision.provider(),
                    state: ProfileState::Draft,
                    credential_status: CredentialViewStatus::Missing,
                    is_active: false,
                },
                revision: revision.revision(),
                credential_generation: revision.credential_generation(),
                model: revision.model().clone(),
                parameters: revision.parameters().clone(),
                validation_id: None,
                oauth_status: None,
            };
            self.state.lock().expect("test state lock").profile = Some(detail.clone());
            Ok(detail)
        }

        async fn copy_profile(
            &self,
            _source: ProfileId,
            _name: ProfileName,
        ) -> ProviderResult<ProfileDetail> {
            Err(fake_provider_error())
        }

        async fn mutate_credential(
            &self,
            request: CredentialMutationRequest,
        ) -> ProviderResult<ProfileDetail> {
            let mut state = self.state.lock().expect("test state lock");
            let detail = state.profile.as_mut().ok_or_else(fake_provider_error)?;
            detail.revision = detail
                .revision
                .checked_add(1)
                .ok_or_else(fake_provider_error)?;
            detail.credential_generation = request.intent.new_generation();
            detail.summary.credential_status = CredentialViewStatus::Saved;
            Ok(detail.clone())
        }

        async fn delete_profile(&self, _request: DeleteProfileRequest) -> ProviderResult<()> {
            Err(fake_provider_error())
        }

        async fn discover_models(
            &self,
            _request: DiscoverModelsRequest,
        ) -> ProviderResult<Vec<DiscoveredModel>> {
            Ok(vec![DiscoveredModel {
                model: "deepseek/keyboard".to_owned(),
                context_limit: Some(32_768),
            }])
        }

        async fn validate_profile(
            &self,
            request: ValidateProfileRequest,
        ) -> ProviderResult<CompatibilityEvidenceView> {
            let mut state = self.state.lock().expect("test state lock");
            let detail = state.profile.as_mut().ok_or_else(fake_provider_error)?;
            if detail.summary.profile_id != request.profile_id
                || detail.revision != request.revision
                || detail.summary.credential_status != CredentialViewStatus::Saved
            {
                return Err(fake_provider_error());
            }
            detail.summary.state = ProfileState::Ready;
            let validation_id = ValidationId::new();
            detail.validation_id = Some(validation_id);
            Ok(CompatibilityEvidenceView {
                validation_id,
                state: ProfileState::Ready,
                credential_status: CredentialViewStatus::Saved,
                error: None,
            })
        }

        async fn activate(
            &self,
            request: ActivateProfileRequest,
        ) -> ProviderResult<ActiveProviderView> {
            self.activate_current(request.precondition.profile_id, request.operation_id)
                .await
        }

        async fn activate_current(
            &self,
            profile_id: ProfileId,
            _operation_id: OperationId,
        ) -> ProviderResult<ActiveProviderView> {
            let mut state = self.state.lock().expect("test state lock");
            let detail = state.profile.as_ref().ok_or_else(fake_provider_error)?;
            if detail.summary.profile_id != profile_id
                || detail.summary.state != ProfileState::Ready
            {
                return Err(fake_provider_error());
            }
            let active = ActiveProviderView {
                activation_revision: 1,
                profile_id,
                profile_revision: detail.revision,
                provider: detail.summary.provider,
                model: detail.model.clone(),
                parameters: detail.parameters.clone(),
            };
            state.active = Some(active.clone());
            Ok(active)
        }

        async fn credential_status(
            &self,
            profile_id: ProfileId,
        ) -> ProviderResult<CredentialViewStatus> {
            Ok(self.profile(profile_id)?.summary.credential_status)
        }

        async fn oauth_connection(
            &self,
            _profile_id: ProfileId,
        ) -> ProviderResult<OAuthConnectionView> {
            Err(fake_provider_error())
        }

        async fn doctor(&self) -> ProviderResult<ProviderDoctorView> {
            Ok(ProviderDoctorView {
                active: self.active_provider().await?,
                credential_status: None,
                blockers: Vec::new(),
                warnings: Vec::new(),
            })
        }

        async fn cancel_operation(&self, _operation_id: OperationId) -> ProviderResult<()> {
            Ok(())
        }

        async fn start_oauth(
            &self,
            _profile_id: ProfileId,
            _operation_id: OperationId,
        ) -> ProviderResult<DeviceAuthorizationView> {
            Err(fake_provider_error())
        }

        async fn complete_oauth(
            &self,
            _operation_id: OperationId,
        ) -> ProviderResult<OAuthConnectionView> {
            Err(fake_provider_error())
        }

        async fn refresh_oauth(
            &self,
            _profile_id: ProfileId,
            _operation_id: OperationId,
        ) -> ProviderResult<OAuthConnectionView> {
            Err(fake_provider_error())
        }

        async fn reauthorize_oauth(
            &self,
            _profile_id: ProfileId,
            _operation_id: OperationId,
        ) -> ProviderResult<DeviceAuthorizationView> {
            Err(fake_provider_error())
        }

        async fn logout_oauth(
            &self,
            _profile_id: ProfileId,
            _operation_id: OperationId,
        ) -> ProviderResult<RemoteRevocationOutcome> {
            Err(fake_provider_error())
        }
    }

    async fn apply_provider_completion(controller: &mut TuiController, app: &mut TuiApp) {
        let completion = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(completion) = controller.take_ready_provider_operation() {
                    return completion;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Provider operation completes through the service boundary");
        controller.apply_provider_operation(app, completion);
    }

    #[tokio::test]
    async fn provider_keyboard_flow_selects_saves_validates_and_activates_without_config_file() {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts = Arc::new(
            LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
        );
        let workspace_id = WorkspaceId::new();
        let provider_api = Arc::new(KeyboardProviderApi::default());
        let service = Arc::new(
            InProcessAgentService::new(workspace_id, store, artifacts, Arc::new(NoopRunScheduler))
                .with_provider_management_api(provider_api),
        );
        let principal = Principal::local_operator("keyboard-provider-test");
        let mut controller = TuiController::new(service, workspace_id, principal.clone());
        let mut app = TuiApp::for_principal(principal);

        controller
            .apply(&mut app, InputAction::Providers)
            .await
            .expect("open Provider manager through AgentServiceApi");
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
        )
        .await
        .expect("start draft with keyboard");
        let deepseek_index = ys_agent_core::ProviderId::ALL
            .iter()
            .position(|provider| *provider == ys_agent_core::ProviderId::DeepSeek)
            .expect("DeepSeek is in the governed catalog");
        let provider_key =
            char::from(b'1' + u8::try_from(deepseek_index).expect("catalog is short"));
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(
                KeyCode::Char(provider_key),
                KeyModifiers::NONE,
            )),
        )
        .await
        .expect("select Provider with keyboard");
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("advance to authentication");
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
        )
        .await
        .expect("select API key authentication");
        for character in "s3cret".chars() {
            handle_terminal_event(
                &mut app,
                &mut controller,
                Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
            )
            .await
            .expect("type masked credential");
        }
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("advance to model");
        for character in "deepseek/keyboard".chars() {
            handle_terminal_event(
                &mut app,
                &mut controller,
                Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
            )
            .await
            .expect("type manually governed model");
        }
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("advance to parameters");
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("advance to validation");
        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
        )
        .await
        .expect("save Draft before validation");
        apply_provider_completion(&mut controller, &mut app).await;
        let saved = controller
            .provider_screen_view()
            .expect("Provider screen remains open");
        assert_eq!(
            saved.step,
            Some(super::super::provider_management::ProviderManagementStep::Validate)
        );
        assert!(
            saved
                .edit
                .as_ref()
                .expect("saved edit")
                .profile_id
                .is_some()
        );
        assert!(
            !format!("{saved:?}").contains("s3cret"),
            "the typed credential must not escape the mask boundary"
        );

        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)),
        )
        .await
        .expect("validate saved Draft");
        apply_provider_completion(&mut controller, &mut app).await;
        assert_eq!(
            controller.provider_screen_view().expect("screen").step,
            Some(super::super::provider_management::ProviderManagementStep::SaveActivate)
        );

        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
        )
        .await
        .expect("confirm and schedule activation");
        apply_provider_completion(&mut controller, &mut app).await;
        assert!(
            controller
                .provider_screen_view()
                .expect("screen")
                .browse
                .active
                .is_some(),
            "only the committed service result may mark a Profile active"
        );

        controller
            .apply(&mut app, InputAction::Providers)
            .await
            .expect("offline-safe active snapshot remains browseable without configuration files");
        assert!(
            app.detail
                .as_ref()
                .expect("Provider detail")
                .lines
                .iter()
                .any(|line| line.starts_with("Active · DeepSeek"))
        );
    }

    #[tokio::test]
    async fn invalid_slash_command_keeps_draft_for_correction() {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts = Arc::new(
            LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
        );
        let workspace_id = WorkspaceId::new();
        let service = Arc::new(
            InProcessAgentService::new(workspace_id, store, artifacts, Arc::new(NoopRunScheduler))
                .with_run_provider_binding_source(Arc::new(
                    StaticRunProviderBindingSource::for_test(),
                )),
        );
        let principal = Principal::local_operator("test-operator");
        let mut controller = TuiController::new(service, workspace_id, principal.clone());
        let mut app = TuiApp::for_principal(principal);
        app.composer.set_text("/你好");

        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("invalid command handling");

        assert_eq!(app.composer.text(), "/你好");
        assert!(matches!(
            app.transcript.last(),
            Some(super::TranscriptItem::Warning(text))
                if text.contains("available commands: /mode  /model")
        ));
    }

    #[tokio::test]
    async fn tui_submission_acknowledges_before_model_returns() {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts = Arc::new(
            LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
        );
        let release_model = Arc::new(Semaphore::new(0));
        let model = Arc::new(FakeModelProvider::new({
            let release_model = release_model.clone();
            move |_| {
                let release_model = release_model.clone();
                async move {
                    let _permit = release_model.acquire().await.expect("model release");
                    Ok(ModelResponse {
                        action: AgentAction::Respond {
                            message: "你好！".to_owned(),
                        },
                        raw_content: None,
                        usage: None,
                    })
                }
            }
        }));
        let workspace_id = WorkspaceId::new();
        let service = Arc::new(
            InProcessAgentService::new(workspace_id, store, artifacts, Arc::new(NoopRunScheduler))
                .with_run_provider_binding_source(Arc::new(
                    StaticRunProviderBindingSource::for_test(),
                ))
                .with_conversation_model(model, "delayed-test-model"),
        );
        let principal = Principal::local_operator("test-operator");
        let mut controller = TuiController::new(service, workspace_id, principal.clone());
        let mut app = TuiApp::for_principal(principal);
        app.doctor_report = Some(DoctorReport {
            blocker_codes: Vec::new(),
            warning_codes: Vec::new(),
            ready_capabilities: vec![QueryCapability::AdHocRead],
            repairs: Vec::new(),
        });
        app.composer.set_text("你好");

        let handled = tokio::time::timeout(
            Duration::from_millis(100),
            handle_terminal_event(
                &mut app,
                &mut controller,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ),
        )
        .await
        .expect("Enter must not wait for the model")
        .expect("Enter handling");

        assert!(!handled);
        assert!(matches!(
            app.transcript.last(),
            Some(super::TranscriptItem::UserMessage(text)) if text == "你好"
        ));
        assert_eq!(app.runtime_status.as_deref(), Some("Thinking…"));
        let acknowledged = render_to_string(&app, 80, 20);
        assert!(acknowledged.contains("You"));
        assert!(acknowledged.contains("Thinking"));

        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        )
        .await
        .expect("typing while model is pending");
        assert_eq!(app.composer.text(), "x");

        handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        )
        .await
        .expect("duplicate Enter handling");
        assert_eq!(app.composer.text(), "x");
        assert!(matches!(
            app.transcript.last(),
            Some(super::TranscriptItem::Warning(text)) if text.contains("draft was kept")
        ));

        let detached = handle_terminal_event(
            &mut app,
            &mut controller,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        )
        .await
        .expect("Ctrl-C handling");
        assert!(detached);

        release_model.add_permits(1);
        let completion = take_submission(&mut controller)
            .await
            .expect("successful submission");
        controller.complete_submission(&mut app, completion);
        assert!(matches!(
            app.transcript.last(),
            Some(super::TranscriptItem::Answer(answer)) if answer.conclusion == "你好！"
        ));
        assert!(render_to_string(&app, 80, 20).contains("Ys-da"));
        assert_eq!(app.runtime_status.as_deref(), Some("Ready"));
    }

    #[tokio::test]
    async fn tui_submission_timeout_is_rendered_after_non_blocking_ack() {
        let directory = tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts = Arc::new(
            LocalArtifactStore::new(directory.path().join("artifacts")).expect("artifact store"),
        );
        let model = Arc::new(FakeModelProvider::new(move |_| async move {
            Err(ys_agent_core::CoreError::validation(
                "provider_timeout",
                "test timeout",
            ))
        }));
        let workspace_id = WorkspaceId::new();
        let service = Arc::new(
            InProcessAgentService::new(workspace_id, store, artifacts, Arc::new(NoopRunScheduler))
                .with_run_provider_binding_source(Arc::new(
                    StaticRunProviderBindingSource::for_test(),
                ))
                .with_conversation_model(model, "timeout-test-model"),
        );
        let principal = Principal::local_operator("test-operator");
        let mut controller = TuiController::new(service, workspace_id, principal.clone());
        let mut app = TuiApp::for_principal(principal);
        app.doctor_report = Some(DoctorReport {
            blocker_codes: Vec::new(),
            warning_codes: Vec::new(),
            ready_capabilities: vec![QueryCapability::AdHocRead],
            repairs: Vec::new(),
        });
        app.composer.set_text("你好");

        tokio::time::timeout(
            Duration::from_millis(100),
            handle_terminal_event(
                &mut app,
                &mut controller,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ),
        )
        .await
        .expect("Enter must not wait for a failing model")
        .expect("Enter handling");
        assert_eq!(app.runtime_status.as_deref(), Some("Thinking…"));

        let error = match take_submission(&mut controller).await {
            Ok(_) => panic!("expected provider timeout"),
            Err(error) => error,
        };
        app.runtime_status = None;
        app.push_transcript(super::TranscriptItem::Error(user_readable_error(&error)));

        let rendered = render_to_string(&app, 80, 20);
        assert!(rendered.contains("provider_timeout"));
        assert!(app.runtime_status.is_none());
    }
}
