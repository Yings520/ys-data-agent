use chrono::Utc;
use ratatui::{Terminal, backend::TestBackend, layout::Rect, style::Color};
use ys_agent_core::{
    ActiveProviderView, ArtifactId, EventActor, EventEnvelope, EventId, PolicyDecision, Principal,
    ProfileId, ProviderId, ProviderModelId, ProviderParameters, RunEventKind, RunId, TaskId,
    ToolCallId, VersionedRunEvent, WorkspaceId,
};

use ys_agent_runtime::{
    DatasourceDisplayState, QueryDisplayState, TuiDisplayContext, TuiDisplayContextInput,
};
use ysda::tui::{
    ArtifactWorkspaceState, AsyncChannel, AsyncResultGuard, ColorSpec, ContentRoute, DetailKind,
    DetailView, FocusTarget, HitRegion, InputAction, InputLayer, LayoutMode, ModePickerAction,
    ModePickerOutcome, ModePickerState, ModelSelectionState, NavigationState, Overlay,
    ThemeRegistry, TimelineState, TransientView, TuiApp, TuiQueryMode, UiPreferences,
    bottom_panel_height, parse_input, render_to_string,
};

fn timeline_event(sequence: u64, event: RunEventKind) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId::new(),
        workspace_id: WorkspaceId::new(),
        task_id: TaskId::new(),
        run_id: RunId::new(),
        sequence,
        occurred_at: Utc::now(),
        actor: EventActor::System,
        event: VersionedRunEvent::v1(event),
    }
}

fn rendered_color(app: &TuiApp, width: u16, height: u16, needle: &str) -> Color {
    let mut app = app.clone();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| ysda::tui::render(frame, &mut app))
        .expect("render production TUI");
    let buffer = terminal.backend().buffer();
    for row in 0..height {
        let line = (0..width)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>();
        if let Some(byte_offset) = line.find(needle) {
            let column = line[..byte_offset].chars().count() as u16;
            return buffer[(column, row)].fg;
        }
    }
    panic!("rendered output did not contain {needle:?}");
}

#[test]
fn welcome_is_minimal_and_shows_safe_header_labels() {
    let app = TuiApp::test_home(
        "ecommerce",
        "postgres-prod",
        "read-only",
        "Provider Profile",
    );
    let rendered = render_to_string(&app, 100, 28);

    assert!(rendered.contains("Agent"));
    assert!(rendered.contains("ecommerce"));
    assert!(rendered.contains("postgres-prod"));
    assert!(rendered.contains("read-only"));
    assert!(rendered.contains("Ask a governed data question"));
    assert!(!rendered.contains("Recent work"));
    assert!(!rendered.contains("Recent tasks"));
    assert!(!rendered.contains("Artifact"));
}

#[test]
fn slash_mode_is_a_typed_product_command() {
    assert_eq!(parse_input("/mode"), Ok(InputAction::Mode));
}

#[test]
fn retired_commands_have_no_hidden_parser_path() {
    for command in ["/new", "/quit", "/providers", "/doctor", "/artifact"] {
        assert!(parse_input(command).is_err(), "retired command: {command}");
    }
}

#[test]
fn model_command_uses_the_single_model_selection_route() {
    assert_eq!(
        parse_input("/model").expect("Model Selection command"),
        InputAction::Model
    );
}

#[test]
fn v02_tui_does_not_offer_unimplemented_modes() {
    let app = TuiApp::for_principal(Principal::local_operator("ysc"));
    let rendered = render_to_string(&app, 100, 28);
    assert!(!rendered.contains("Build mode"));
    assert!(!rendered.contains("Analysis mode"));
}

#[test]
fn mode_picker_confirms_or_cancels_without_leaking_partial_state() {
    let mut picker = ModePickerState::new(TuiQueryMode::Auto, "draft question".to_owned());
    assert_eq!(picker.options(), [TuiQueryMode::Auto, TuiQueryMode::Query]);
    assert_eq!(
        picker.reduce(ModePickerAction::MoveDown),
        ModePickerOutcome::Open
    );
    assert_eq!(
        picker.reduce(ModePickerAction::Confirm),
        ModePickerOutcome::Confirmed(TuiQueryMode::Query)
    );
    assert_eq!(
        TuiQueryMode::Auto.workflow(),
        TuiQueryMode::Query.workflow()
    );

    let mut cancelled = ModePickerState::new(TuiQueryMode::Query, "kept draft".to_owned());
    assert_eq!(
        cancelled.reduce(ModePickerAction::Cancel),
        ModePickerOutcome::Cancelled {
            mode: TuiQueryMode::Query,
            composer: "kept draft".to_owned(),
        }
    );
}

#[test]
fn cancelling_mode_picker_restores_mode_composer_and_page() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.query_mode = TuiQueryMode::Query;
    app.composer.set_text("kept question");
    app.show_detail(
        DetailKind::Sql,
        DetailView {
            title: "SQL".to_owned(),
            lines: vec!["select 1".to_owned()],
        },
    );

    app.open_mode_picker();
    assert_eq!(app.transient, Some(TransientView::ModePicker));
    let rendered = render_to_string(&app, 100, 28);
    assert!(rendered.contains("Auto"));
    assert!(rendered.contains("Query"));
    assert!(!rendered.contains("Build"));
    assert!(!rendered.contains("Analysis"));
    app.close_transient();

    assert_eq!(app.query_mode, TuiQueryMode::Query);
    assert_eq!(app.composer.text(), "kept question");
    assert_eq!(app.transient, Some(TransientView::Detail(DetailKind::Sql)));
    assert_eq!(app.detail.as_ref().expect("restored detail").title, "SQL");
}

#[test]
fn route_overlay_and_focus_state_is_local_reversible_and_independent() {
    let mut navigation = NavigationState::new();
    assert_eq!(navigation.routes(), [ContentRoute::Timeline]);
    assert_eq!(navigation.current(), ContentRoute::Timeline);
    assert_eq!(navigation.input_layer(), InputLayer::View);

    navigation.push(ContentRoute::Artifact);
    assert_eq!(navigation.current(), ContentRoute::Artifact);
    assert!(navigation.open_overlay(Overlay::CommandPalette));
    assert!(!navigation.open_overlay(Overlay::Help));
    assert_eq!(navigation.input_layer(), InputLayer::Overlay);
    assert_eq!(navigation.close_overlay(), Some(Overlay::CommandPalette));
    assert_eq!(navigation.pop(), Some(ContentRoute::Artifact));
    assert_eq!(navigation.current(), ContentRoute::Timeline);
    assert_eq!(
        NavigationState::input_priority(),
        [InputLayer::Overlay, InputLayer::View, InputLayer::Composer]
    );

    let mut artifact = ArtifactWorkspaceState::default();
    artifact.search = "revenue".to_owned();
    artifact.highlighted = Some(3);
    artifact.scroll = 9;
    artifact.focus = FocusTarget::ArtifactContent;
    let mut models = ModelSelectionState::default();
    models.search = "deepseek".to_owned();
    models.highlighted = Some(1);
    models.scroll = 4;
    models.focus = FocusTarget::ModelSelectionList;
    assert_ne!(artifact.search, models.search);
    assert_ne!(artifact.highlighted, models.highlighted);

    let mut timeline = TimelineState::default();
    timeline.focus_result_card(HitRegion::new(2, 3, 20, 4));
    assert_eq!(timeline.focus, FocusTarget::TimelineResultCard);
    assert!(
        timeline
            .result_card_hit_region
            .expect("hit region")
            .contains(4, 5)
    );
    assert!(
        !timeline
            .result_card_hit_region
            .expect("hit region")
            .contains(40, 5)
    );

    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.artifact_workspace = artifact;
    app.model_selection_state = models;
    app.push_route(ContentRoute::Artifact);
    assert!(render_to_string(&app, 100, 28).contains("Search · revenue"));
    assert_eq!(app.pop_route(), Some(ContentRoute::Artifact));
    app.push_route(ContentRoute::ModelSelection);
    assert!(render_to_string(&app, 100, 28).contains("Search · deepseek"));
}

#[test]
fn async_results_are_accepted_only_for_the_current_route_and_operation() {
    let mut navigation = NavigationState::new();
    navigation.push(ContentRoute::ModelSelection);
    let model_route = navigation.route_key();
    let mut guard = AsyncResultGuard::default();

    let old_catalog = guard
        .start(AsyncChannel::Catalog, model_route)
        .expect("catalog read starts");
    let current_catalog = guard
        .start(AsyncChannel::Catalog, model_route)
        .expect("a newer catalog read supersedes the old one");
    assert!(!guard.accept_completion(old_catalog, model_route));
    assert!(guard.accept_completion(current_catalog, model_route));

    let late_catalog = guard
        .start(AsyncChannel::Catalog, model_route)
        .expect("catalog refresh starts");
    navigation.pop();
    assert!(!guard.accept_completion(late_catalog, navigation.route_key()));
    assert!(
        guard.active(AsyncChannel::Catalog).is_none(),
        "discarding a completed stale operation must release its lane"
    );
    navigation.push(ContentRoute::ModelSelection);
    assert_ne!(
        navigation.route_key(),
        model_route,
        "re-entering the same page creates a new route visit"
    );
}

#[test]
fn async_channels_are_independent_and_provider_mutation_is_single_flight() {
    let navigation = NavigationState::new();
    let route = navigation.route_key();
    let mut guard = AsyncResultGuard::default();

    let display = guard
        .start(AsyncChannel::DisplayContext, route)
        .expect("display context read starts");
    let catalog = guard
        .start(AsyncChannel::Catalog, route)
        .expect("catalog read starts independently");
    let artifact = guard
        .start(AsyncChannel::Artifact, route)
        .expect("artifact read starts independently");
    let provider = guard
        .start(AsyncChannel::ProviderMutation, route)
        .expect("first Provider mutation starts");
    assert!(
        guard.start(AsyncChannel::ProviderMutation, route).is_err(),
        "at most one Provider mutation may be active"
    );

    let mut app = TuiApp::for_principal(Principal::local_operator("async-cancel-test"));
    app.model_label = "active-before-cancel".to_owned();
    app.composer.set_text("keep this nonsensitive draft");
    assert!(guard.cancel(provider));
    assert_eq!(app.model_label, "active-before-cancel");
    assert_eq!(app.composer.text(), "keep this nonsensitive draft");
    assert!(guard.accept_completion(display, route));
    assert!(guard.accept_completion(catalog, route));
    assert!(guard.accept_completion(artifact, route));
    assert!(!guard.accept_completion(provider, route));
}

#[test]
fn default_view_hides_internal_runtime_identifiers() {
    let app = TuiApp::test_home(
        "ecommerce",
        "postgres-prod",
        "read-only",
        "Provider Profile",
    );
    let rendered = render_to_string(&app, 100, 28);

    assert!(!rendered.contains("RunId"));
    assert!(!rendered.contains("QueryPhase"));
    assert!(!rendered.contains("StepId"));
}

#[test]
fn answer_is_full_width_concise_and_uses_ys_da_role() {
    let app = TuiApp::test_answer(
        "GMV for the last seven complete days",
        "GMV increased 12.4% week over week.",
        [Some("$2.84M"), Some("+$313K")],
        Some("Growth was concentrated in APAC."),
    );
    let rendered = render_to_string(&app, 100, 28);

    assert!(rendered.contains("Ys-da"));
    assert!(!rendered.contains("Assistant"));
    assert!(!rendered.contains("Recent work"));
    assert!(!rendered.contains("Query"));
    assert!(!rendered.contains("Checks"));
    assert!(!rendered.contains("Artifact"));
}

#[test]
fn chat_reply_renders_as_a_ys_da_answer_without_starting_a_query_view() {
    let mut app = TuiApp::test_home("ecommerce", "fixture", "read-only", "fixture-model");
    app.transcript.push(ysda::tui::TranscriptItem::UserMessage(
        "你好，介绍一下你自己".to_owned(),
    ));
    app.transcript
        .push(ysda::tui::TranscriptItem::Answer(ysda::tui::AnswerView {
            state: "Chat".to_owned(),
            conclusion: "I am Ys-da. I answer chat without starting a Query Run.".to_owned(),
            key_values: [None, None],
            explanation: None,
        }));
    let rendered = render_to_string(&app, 100, 28);

    assert!(rendered.contains("Ys-da"));
    assert!(rendered.contains("Chat"));
    assert!(rendered.contains("I am Ys-da. I answer chat without starting a Query Run."));
    assert!(!rendered.contains("Query scheduled"));
}

#[test]
fn slash_palette_replaces_composer_and_keeps_one_input_surface() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("/");
    app.sync_slash_palette();

    let rendered = render_to_string(&app, 100, 28);
    assert!(rendered.contains("Commands"));
    assert_eq!(rendered.matches("Search commands").count(), 1);
    assert!(!rendered.contains("Ask a governed data question…"));
    assert_eq!(bottom_panel_height(&app, Rect::new(0, 0, 100, 28)), 10);

    app.composer.set_text("/model");
    app.sync_slash_palette();
    assert_eq!(bottom_panel_height(&app, Rect::new(0, 0, 100, 28)), 10);
}

#[test]
fn slash_palette_keeps_no_match_open_for_continued_editing() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("/model");
    app.sync_slash_palette();
    assert_eq!(app.transient, Some(TransientView::SlashPalette));

    app.composer.set_text("/model unknown");
    app.sync_slash_palette();

    assert_eq!(app.transient, Some(TransientView::SlashPalette));
    assert_eq!(app.composer.text(), "/model unknown");
}

#[test]
fn slash_prefix_with_only_whitespace_keeps_the_palette_open() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("  /");
    app.sync_slash_palette();

    assert_eq!(app.transient, Some(TransientView::SlashPalette));
    app.close_transient();
    assert_eq!(app.composer.text(), "  ");
}

#[test]
fn slash_argument_does_not_replace_an_open_theme_picker() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.transient = Some(TransientView::ThemePicker);
    app.composer.set_text("/model unknown");
    app.sync_slash_palette();

    assert_eq!(app.transient, Some(TransientView::ThemePicker));
    assert_eq!(app.composer.text(), "/model unknown");
}

#[test]
fn supported_terminal_sizes_render_without_panicking() {
    let app = TuiApp::for_principal(Principal::local_operator("ysc"));
    for (width, height) in [(60, 12), (80, 20), (100, 28), (150, 40)] {
        let _ = render_to_string(&app, width, height);
    }
}

#[test]
fn responsive_renderer_goldens_cover_shell_artifact_and_outcome_matrix() {
    let mut shell = TuiApp::test_home(
        "governed-workspace",
        "orders-warehouse",
        "read-only",
        "deepseek/governed-chat",
    );
    shell.composer.set_text("kept governed question");
    for (width, height) in [(150, 40), (100, 28), (60, 12)] {
        let rendered = render_to_string(&shell, width, height);
        for required in [
            "Agent",
            "Ask a governed data question",
            "kept governed question",
            "/mode  /model",
        ] {
            assert!(
                rendered.contains(required),
                "{width}×{height} omitted {required}"
            );
        }
        assert!(!rendered.contains("Terminal too small"));

        let mut artifact = shell.clone();
        artifact.push_route(ContentRoute::Artifact);
        let rendered = render_to_string(&artifact, width, height);
        for required in [
            "Artifact", "Summary", "Results", "SQL", "Schema", "Evidence", "Esc back",
        ] {
            assert!(
                rendered.contains(required),
                "{width}×{height} omitted {required}"
            );
        }
    }

    assert_eq!(
        render_to_string(&shell, 50, 8),
        "Agent\nTerminal too small · resize to at least 60×12\nComposer · kept governed question\nCtrl-C detach"
    );

    let cases = [
        (
            RunEventKind::RunWaiting {
                reason: "Need a governed date range".to_owned(),
            },
            "Status · Waiting for input",
            "Need a governed date range",
            shell.active_theme.warning,
        ),
        (
            RunEventKind::PolicyEvaluated {
                call_id: ToolCallId::new(),
                decision: PolicyDecision::Deny {
                    code: "policy.read_denied".to_owned(),
                    message: "provider detail must stay hidden".to_owned(),
                },
            },
            "Status · Denied",
            "policy.read_denied",
            shell.active_theme.error,
        ),
        (
            RunEventKind::RunFailed {
                code: "query.execution_failed".to_owned(),
                message: "transport body must stay hidden".to_owned(),
            },
            "Status · Failed",
            "query.execution_failed",
            shell.active_theme.error,
        ),
        (
            RunEventKind::RunCancelled {
                reason: "Cancelled by operator".to_owned(),
            },
            "Status · Cancelled",
            "Cancelled by operator",
            shell.active_theme.warning,
        ),
        (
            RunEventKind::RunCompleted {
                primary_artifact_id: ArtifactId::new(),
            },
            "Status · Succeeded",
            "Status · Succeeded",
            shell.active_theme.success,
        ),
    ];
    for (event, status, reason, expected_color) in cases {
        let mut app = shell.clone();
        app.timeline_state.apply_event(&timeline_event(1, event));
        for (width, height) in [(150, 40), (100, 28), (60, 12)] {
            let rendered = render_to_string(&app, width, height);
            assert!(
                rendered.contains(status),
                "{width}×{height} omitted {status}"
            );
            assert!(
                rendered.contains(reason),
                "{width}×{height} omitted {reason}"
            );
            if !status.ends_with("Succeeded") {
                assert!(!rendered.contains("Verified"));
            }
        }
        assert_eq!(rendered_color(&app, 100, 28, status), expected_color);
    }

    let mut warning = shell;
    warning.safe_warning = Some("query.preview_limited".to_owned());
    let rendered = render_to_string(&warning, 100, 28);
    assert!(rendered.contains("Warning  query.preview_limited"));
    assert_eq!(
        rendered_color(&warning, 100, 28, "Warning  query.preview_limited"),
        warning.active_theme.warning
    );
    assert!(!rendered.contains("Verified"));
}

#[test]
fn responsive_shell_keeps_product_content_composer_and_context_keys_visible() {
    let mut app = TuiApp::test_home(
        "workspace\ncontrol",
        "warehouse\tdatasource",
        "read-only",
        "deepseek/chat",
    );
    app.query_mode = TuiQueryMode::Query;
    app.composer.set_text("kept question");

    for (width, height) in [(60, 12), (80, 20), (120, 30), (150, 40)] {
        let rendered = render_to_string(&app, width, height);
        assert!(rendered.contains("Agent"));
        assert!(rendered.contains("Ask a governed data question"));
        assert!(rendered.contains("kept question"));
        assert!(rendered.contains("/mode  /model"));
        assert!(!rendered.contains('\n') || !rendered.contains("workspace\ncontrol"));
    }
    let standard = render_to_string(&app, 100, 28);
    assert!(standard.contains("QUERY"));
    assert!(standard.contains("deepseek/chat"));
    assert!(!standard.contains('\t'));
}

#[test]
fn header_reads_typed_display_context_mode_and_authoritative_active_model() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.workspace_name = "spoofed-local-workspace".to_owned();
    app.model_label = "spoofed-local-model".to_owned();
    app.apply_display_context(TuiDisplayContext::from(
        TuiDisplayContextInput::new(
            "Governed Workspace",
            DatasourceDisplayState::active("Orders Warehouse").expect("safe datasource label"),
            true,
            QueryDisplayState::Running,
        )
        .expect("safe display context"),
    ));
    app.apply_active_provider_view(Some(&ActiveProviderView {
        activation_revision: 1,
        profile_id: ProfileId::new(),
        profile_revision: 1,
        provider: ProviderId::DeepSeek,
        model: ProviderModelId::new(ProviderId::DeepSeek, "deepseek/governed-chat")
            .expect("valid model"),
        parameters: ProviderParameters::default(),
    }));
    app.query_mode = TuiQueryMode::Auto;

    let rendered = render_to_string(&app, 150, 40);
    assert!(rendered.contains("Governed Workspace"));
    assert!(rendered.contains("Orders Warehouse"));
    assert!(rendered.contains("AUTO › QUERY"));
    assert!(rendered.contains("deepseek/governed-chat"));
    assert!(rendered.contains("read-only"));
    assert!(rendered.contains("query running"));
    assert!(!rendered.contains("spoofed-local"));

    app.mark_display_context_unavailable();
    let unavailable = render_to_string(&app, 150, 40);
    assert!(unavailable.contains("Governed Workspace"));
    assert!(unavailable.contains("status unavailable"));
}

#[test]
fn undersized_shell_has_a_non_overlapping_recovery_view() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("draft");
    let rendered = render_to_string(&app, 50, 8);

    assert!(rendered.contains("Agent"));
    assert!(rendered.contains("Terminal too small"));
    assert!(rendered.contains("draft"));
    assert!(rendered.contains("Ctrl-C detach"));
    for (width, height) in [(1, 1), (20, 3), (59, 11)] {
        let _ = render_to_string(&app, width, height);
    }
}

#[test]
fn artifact_footer_uses_the_shared_command_catalog_and_back_hint() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.push_route(ContentRoute::Artifact);
    let rendered = render_to_string(&app, 100, 28);
    assert!(rendered.contains("/mode  /model  Esc back"));
}

#[test]
fn footer_and_palette_expose_only_the_product_catalog() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    let footer = render_to_string(&app, 100, 28);
    assert!(footer.contains("/mode  /model"));
    assert!(!footer.contains("/doctor"));
    assert!(!footer.contains("/providers"));

    app.composer.set_text("/");
    app.sync_slash_palette();
    let palette = render_to_string(&app, 100, 28);
    assert!(palette.contains("/mode"));
    assert!(palette.contains("/model"));
    assert!(!palette.contains("/theme"));

    app.close_transient();
    app.transient = Some(TransientView::Help);
    let help = render_to_string(&app, 100, 28);
    assert!(help.contains("/mode"));
    assert!(help.contains("/model"));
    assert!(!help.contains("/help"));
    assert!(!help.contains("/providers"));
}

#[test]
fn parser_keeps_free_text_out_of_command_routing() {
    assert_eq!(
        parse_input("free text is a governed question").expect("message"),
        InputAction::SendMessage("free text is a governed question".to_owned())
    );
    assert!(parse_input("/unknown").is_err());
}

#[test]
fn invalid_theme_color_is_stable_and_keeps_the_active_theme() {
    let registry = ThemeRegistry::default();
    let before = registry.resolve("deep-navy").expect("default preset");
    let error = ColorSpec::parse("#12ZZ00").expect_err("invalid color");

    assert_eq!(error.code(), "invalid_theme_color");
    assert_eq!(registry.resolve("deep-navy").expect("still valid"), before);

    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    let mut invalid = UiPreferences {
        theme: "custom".to_owned(),
        ..UiPreferences::default()
    };
    invalid
        .colors
        .insert("accent".to_owned(), "#12ZZ00".to_owned());
    app.apply_preferences(&invalid, false);
    assert_eq!(app.active_theme, before);
    assert_eq!(app.safe_warning.as_deref(), Some("invalid_theme_color"));
}

#[test]
fn focused_detail_replaces_the_body_and_escape_restores_the_answer() {
    let mut app = TuiApp::test_answer("Question", "Conclusion", [None, None], None);
    app.show_detail(
        DetailKind::Sql,
        DetailView {
            title: "SQL".to_owned(),
            lines: vec!["select 1".to_owned()],
        },
    );
    assert!(render_to_string(&app, 100, 28).contains("select 1"));

    app.close_transient();
    let restored = render_to_string(&app, 100, 28);
    assert!(restored.contains("Conclusion"));
    assert!(!restored.contains("select 1"));
}

#[test]
fn theme_preview_escape_restores_the_active_theme() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    let active = app.active_theme.clone();
    app.preview_theme = Some(app.theme_registry.resolve("nord").expect("nord"));
    app.transient = Some(TransientView::ThemePicker);
    app.close_transient();
    assert_eq!(app.active_theme, active);
    assert!(app.preview_theme.is_none());
}

#[test]
fn layout_modes_never_encode_a_sidebar_mode() {
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 59, 11)),
        LayoutMode::TooSmall
    );
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 60, 12)),
        LayoutMode::Compact
    );
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 100, 28)),
        LayoutMode::Standard
    );
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 120, 30)),
        LayoutMode::Wide
    );
}
