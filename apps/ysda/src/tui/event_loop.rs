use std::{io, path::PathBuf, time::Duration};

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
use tokio::time;
use ys_agent_core::{CoreError, CoreResult};

use crate::bootstrap::AppDependencies;

use super::{
    TranscriptItem, TransientView, TuiApp, TuiController, UiPreferenceStore, UiPreferences,
    parse_input, render,
};

type RealTerminal = Terminal<CrosstermBackend<io::Stdout>>;

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
                app.close_transient();
                return Ok(false);
            }
            if app.transient == Some(TransientView::ThemePicker) {
                handle_theme_picker_key(app, key)?;
                return Ok(false);
            }
            if app.transient == Some(TransientView::SlashPalette) {
                handle_palette_key(app, controller, key).await?;
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
            app.palette_draft = None;
            app.transient = None;
            if requires_arguments {
                app.composer.set_text(&format!("{command} "));
            } else if key.code == KeyCode::Tab {
                app.composer.set_text(&command);
            } else {
                app.composer.clear();
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
        "What happened: {}. Retry safety and required action are recorded in the Run evidence; use /details and /artifact for diagnostics.",
        error.code()
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;
    use tokio::sync::Semaphore;
    use ys_agent_adapters::model::FakeModelProvider;
    use ys_agent_core::{AgentAction, ModelResponse, Principal, WorkspaceId};
    use ys_agent_runtime::{
        InProcessAgentService, NoopRunScheduler, StaticRunProviderBindingSource,
        doctor::{DoctorReport, QueryCapability},
    };
    use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

    use crate::tui::render_to_string;

    use super::{TuiApp, TuiController, handle_terminal_event, user_readable_error};

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
                if text.contains("delete the leading /") && text.contains("/help")
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
