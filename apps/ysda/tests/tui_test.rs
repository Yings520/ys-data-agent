use ratatui::layout::Rect;
use ys_agent_core::{ArtifactId, ExportFormat, Principal};
use ysda::tui::{
    ColorSpec, DetailKind, DetailRequest, DetailView, InputAction, LayoutMode, ThemeRegistry,
    ThemeToken, TransientView, TuiApp, UiPreferences, bottom_panel_height, parse_input,
    render_to_string,
};

#[test]
fn welcome_is_minimal_and_shows_safe_header_labels() {
    let app = TuiApp::test_home(
        "ecommerce",
        "postgres-prod",
        "read-only",
        "openai-compatible/test-model",
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
fn slash_new_creates_a_session_command_not_a_cancel_command() {
    let action = parse_input("/new").expect("valid command");
    assert_eq!(action, InputAction::NewSession);
    assert!(!matches!(action, InputAction::CancelRun { .. }));
}

#[test]
fn v02_tui_does_not_offer_unimplemented_modes() {
    let app = TuiApp::for_principal(Principal::local_operator("ysc"));
    let rendered = render_to_string(&app, 100, 28);
    assert!(!rendered.contains("Build mode"));
    assert!(!rendered.contains("Analysis mode"));
}

#[test]
fn default_view_hides_internal_runtime_identifiers() {
    let app = TuiApp::test_home(
        "ecommerce",
        "postgres-prod",
        "read-only",
        "openai-compatible/test-model",
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

    app.composer.set_text("/sql");
    app.sync_slash_palette();
    assert_eq!(bottom_panel_height(&app, Rect::new(0, 0, 100, 28)), 10);
}

#[test]
fn slash_palette_closes_after_a_command_argument_without_changing_the_composer() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("/resume");
    app.sync_slash_palette();
    assert_eq!(app.transient, Some(TransientView::SlashPalette));

    app.composer.set_text("/resume task");
    app.sync_slash_palette();

    assert_eq!(app.transient, None);
    assert_eq!(app.composer.text(), "/resume task");
}

#[test]
fn slash_prefix_with_only_whitespace_keeps_the_palette_open() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.composer.set_text("/ ");
    app.sync_slash_palette();

    assert_eq!(app.transient, Some(TransientView::SlashPalette));
    assert_eq!(app.composer.text(), "/ ");
}

#[test]
fn slash_argument_does_not_replace_an_open_theme_picker() {
    let mut app = TuiApp::for_principal(Principal::local_operator("ysc"));
    app.transient = Some(TransientView::ThemePicker);
    app.composer.set_text("/resume task");
    app.sync_slash_palette();

    assert_eq!(app.transient, Some(TransientView::ThemePicker));
    assert_eq!(app.composer.text(), "/resume task");
}

#[test]
fn supported_terminal_sizes_render_without_panicking() {
    let app = TuiApp::for_principal(Principal::local_operator("ysc"));
    for (width, height) in [(60, 12), (80, 20), (100, 28), (150, 40)] {
        let _ = render_to_string(&app, width, height);
    }
}

#[test]
fn information_and_theme_commands_are_typed_before_dispatch() {
    for (raw, expected) in [
        ("/metrics", DetailRequest::Metrics),
        ("/query", DetailRequest::Query),
        ("/checks", DetailRequest::Checks),
        ("/sql", DetailRequest::Sql),
        ("/details", DetailRequest::Diagnostics),
    ] {
        assert_eq!(
            parse_input(raw).expect("focused information command"),
            InputAction::ShowDetail(expected),
        );
    }
    assert_eq!(
        parse_input("/artifact").expect("current Artifact command"),
        InputAction::ShowDetail(DetailRequest::Artifact(None)),
    );
    let artifact_id = ArtifactId::new();
    assert_eq!(
        parse_input(&format!("/artifact {artifact_id}")).expect("specific Artifact command"),
        InputAction::ShowDetail(DetailRequest::Artifact(Some(artifact_id))),
    );
    assert_eq!(
        parse_input("/theme set accent #4389E6").expect("theme override"),
        InputAction::SetThemeColor {
            token: ThemeToken::Accent,
            color: ColorSpec::Rgb(0x43, 0x89, 0xE6),
        },
    );
    assert_eq!(
        parse_input("/theme").expect("theme picker"),
        InputAction::OpenThemePicker,
    );
    assert_eq!(
        parse_input("/theme reset").expect("theme reset"),
        InputAction::ResetTheme,
    );
}

#[test]
fn parser_keeps_data_commands_out_of_model_input() {
    let task_id = "3d315500-ec47-4ce3-83ee-4284ec34cdbc";
    let artifact_id = "1e0b9c5d-5dc3-4ee8-a939-17ea1c0cf58f";

    assert_eq!(
        parse_input("/task new investigate delayed orders").expect("new task"),
        InputAction::NewTask("investigate delayed orders".to_owned())
    );
    assert!(matches!(
        parse_input(&format!("/resume {task_id}")).expect("resume"),
        InputAction::ResumeTask { .. }
    ));
    assert!(matches!(
        parse_input(&format!("/cancel {task_id}")).expect("cancel"),
        InputAction::CancelRun { .. }
    ));
    assert_eq!(
        parse_input(&format!("/export {artifact_id} csv")).expect("export"),
        InputAction::ExportArtifact {
            artifact_id: artifact_id.parse().expect("artifact id"),
            format: ExportFormat::Csv,
        }
    );
    assert_eq!(
        parse_input("free text is a governed question").expect("message"),
        InputAction::SendMessage("free text is a governed question".to_owned())
    );
    assert!(parse_input("/export not-an-id parquet").is_err());
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
        LayoutMode::resolve(Rect::new(0, 0, 60, 12)),
        LayoutMode::Compact
    );
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 100, 28)),
        LayoutMode::Standard
    );
    assert_eq!(
        LayoutMode::resolve(Rect::new(0, 0, 150, 40)),
        LayoutMode::Wide
    );
}
