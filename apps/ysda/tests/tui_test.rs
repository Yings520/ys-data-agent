use ratatui::layout::Rect;
use ys_agent_core::Principal;
use ysda::tui::{
    ColorSpec, DetailKind, DetailView, InputAction, LayoutMode, ModePickerAction,
    ModePickerOutcome, ModePickerState, ThemeRegistry, TransientView, TuiApp, TuiQueryMode,
    UiPreferences, bottom_panel_height, parse_input, render_to_string,
};

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
