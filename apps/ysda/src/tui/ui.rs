use super::{
    TransientView, TuiApp,
    app::{DetailKind, TranscriptItem},
    artifact, model_selection,
    navigation::ContentRoute,
    palette::{command_catalog, command_hint},
    timeline,
};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    TooSmall,
    Compact,
    Standard,
    Wide,
}

impl LayoutMode {
    pub fn resolve(area: Rect) -> Self {
        if area.width < 60 || area.height < 12 {
            Self::TooSmall
        } else if area.width < 80 || area.height < 20 {
            Self::Compact
        } else if area.width >= 120 && area.height >= 30 {
            Self::Wide
        } else {
            Self::Standard
        }
    }
}

pub fn render(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    let mode = LayoutMode::resolve(area);
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.text)),
        area,
    );
    if mode == LayoutMode::TooSmall {
        app.timeline_state.result_card_hit_region = None;
        render_too_small(frame, app, area);
        return;
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        area,
    );
    let shell = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(bottom_panel_height(app, shell)),
        ])
        .split(shell);
    render_header(frame, app, regions[0], mode);
    let body_margin = match mode {
        LayoutMode::Wide => 8,
        LayoutMode::Standard => 4,
        LayoutMode::Compact | LayoutMode::TooSmall => 1,
    };
    let body = regions[1].inner(Margin {
        horizontal: body_margin,
        vertical: u16::from(mode != LayoutMode::Compact),
    });
    render_body(frame, app, body, mode);
    render_bottom(frame, app, regions[2], mode);
}

pub fn bottom_panel_height(app: &TuiApp, area: Rect) -> u16 {
    match app.transient {
        Some(TransientView::SlashPalette) => (3_u16
            .saturating_add(app.slash_palette.visible_row_count() as u16))
        .min(area.height.saturating_sub(2)),
        Some(TransientView::ModePicker | TransientView::ThemePicker) => {
            10_u16.min(area.height.saturating_sub(2))
        }
        _ if matches!(
            app.navigation.current(),
            ContentRoute::ModelSelection | ContentRoute::ProviderManagement
        ) =>
        {
            16_u16.min(area.height.saturating_sub(4))
        }
        _ => 4_u16.min(area.height.saturating_sub(2)),
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, mode: LayoutMode) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let header = app.header_view();
    let value_width = match mode {
        LayoutMode::Wide => 28,
        LayoutMode::Standard => 20,
        LayoutMode::Compact => 10,
        LayoutMode::TooSmall => 8,
    };
    let mut left = vec![
        Span::styled(
            "YS·DA",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  ", Style::default().fg(theme.border)),
        Span::styled(
            safe_width(header.datasource, value_width),
            Style::default().fg(theme.muted),
        ),
    ];
    if mode == LayoutMode::Compact {
        left.extend([
            Span::styled("  │  ", Style::default().fg(theme.border)),
            Span::styled(mode_label(app), Style::default().fg(theme.accent)),
        ]);
    } else {
        left.extend([
            Span::styled("  │  ", Style::default().fg(theme.border)),
            Span::styled(
                format!(" {} ", mode_label(app)),
                Style::default().fg(theme.accent),
            ),
            Span::styled("  │  ", Style::default().fg(theme.border)),
            Span::styled(
                safe_width(header.current_model, value_width),
                Style::default().fg(theme.muted),
            ),
        ]);
    }
    let (access, access_color) = if header.read_only.eq_ignore_ascii_case("read-only") {
        ("◆ READ ONLY".to_owned(), theme.success)
    } else {
        ("◆ SETUP REQUIRED".to_owned(), theme.warning)
    };
    if header.context_unavailable && header.read_only.eq_ignore_ascii_case("read-only") {
        left.push(Span::styled(
            "  ·  STATUS UNAVAILABLE",
            Style::default().fg(theme.warning),
        ));
    }
    let left_width = Line::from(left.clone()).width();
    let access_width = access.chars().count();
    let padding = usize::from(area.width).saturating_sub(left_width + access_width);
    left.push(Span::raw(" ".repeat(padding)));
    left.push(Span::styled(access, Style::default().fg(access_color)));
    frame.render_widget(
        Paragraph::new(Line::from(left))
            .style(Style::default().bg(theme.header))
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(theme.border)),
            ),
        area,
    );
}

fn render_body(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect, _mode: LayoutMode) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let text =
        match app.transient {
            Some(TransientView::Help) => help_lines(app),
            Some(TransientView::Repair) => repair_lines(app),
            _ => match app.navigation.current() {
                ContentRoute::Timeline => match app.transient {
                    Some(TransientView::Detail(_)) => detail_lines(app),
                    _ => {
                        let timeline_lines = timeline::render_lines(&app.timeline_state);
                        if app.transcript.is_empty()
                            && timeline_lines.is_empty()
                            && app.safe_warning.is_none()
                        {
                            return render_text_body(frame, app, area, welcome_lines(app));
                        }
                        let mut text = transcript_lines(app);
                        let timeline_tone = app.timeline_state.view().status.tone();
                        text.lines.extend(timeline_lines.into_iter().map(|line| {
                            Line::from(Span::styled(
                                safe(&line),
                                Style::default().fg(timeline_color(timeline_tone, theme)),
                            ))
                        }));
                        text
                    }
                },
                ContentRoute::Artifact => Text::from(
                    artifact::render_lines(&app.artifact_workspace)
                        .into_iter()
                        .map(|line| {
                            let color = if line.contains("restricted")
                                || line.contains("missing")
                                || line.contains("unavailable")
                            {
                                theme.error
                            } else if line.contains("Warning") || line.contains("preview limited") {
                                theme.warning
                            } else if line.contains("Verification · Verified") {
                                theme.success
                            } else {
                                theme.text
                            };
                            Line::from(Span::styled(safe(&line), Style::default().fg(color)))
                        })
                        .collect::<Vec<_>>(),
                ),
                ContentRoute::ModelSelection | ContentRoute::ProviderManagement => {
                    let mut text = transcript_lines(app);
                    let timeline_tone = app.timeline_state.view().status.tone();
                    text.lines
                        .extend(timeline::render_lines(&app.timeline_state).into_iter().map(
                            |line| {
                                Line::from(Span::styled(
                                    safe(&line),
                                    Style::default().fg(timeline_color(timeline_tone, theme)),
                                ))
                            },
                        ));
                    text
                }
                ContentRoute::Diagnostics => detail_lines(app),
            },
        };
    render_text_body(frame, app, area, text);
}

fn render_text_body(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect, text: Text<'static>) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let rendered_height = text
        .lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(usize::from(area.width.max(1))))
        .sum::<usize>();
    let visible_height = area.height.saturating_sub(0);
    let scroll = if app.scroll == u16::MAX {
        rendered_height
            .saturating_sub(usize::from(visible_height))
            .try_into()
            .unwrap_or(u16::MAX)
    } else {
        app.scroll
    };
    let result_hit_region = if app.navigation.current() == ContentRoute::Timeline
        && app.timeline_state.view().result_card.is_some()
    {
        text.lines
            .iter()
            .position(|line| line.to_string() == "Results")
            .and_then(|result_index| {
                let width = usize::from(area.width.max(1));
                let rows_before = text.lines[..result_index]
                    .iter()
                    .map(|line| line.width().max(1).div_ceil(width))
                    .sum::<usize>();
                let card_rows = text.lines[result_index..]
                    .iter()
                    .map(|line| line.width().max(1).div_ceil(width))
                    .sum::<usize>();
                let scroll = usize::from(scroll);
                let visible_height = usize::from(visible_height);
                (rows_before < scroll.saturating_add(visible_height)).then(|| {
                    let visible_start = rows_before.saturating_sub(scroll);
                    let clipped_top = scroll.saturating_sub(rows_before).min(card_rows);
                    let height = card_rows
                        .saturating_sub(clipped_top)
                        .min(visible_height.saturating_sub(visible_start));
                    timeline::HitRegion::new(
                        area.x,
                        area.y.saturating_add(visible_start as u16),
                        area.width,
                        height as u16,
                    )
                })
            })
    } else {
        None
    };
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(theme.text).bg(theme.background))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
    app.timeline_state.result_card_hit_region = result_hit_region;
}

fn welcome_lines(app: &TuiApp) -> Text<'static> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    Text::from(vec![
        Line::from(Span::styled(
            "Welcome to YS·DA",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Trustworthy answers from your data.",
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Get started",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("1  ", Style::default().fg(theme.muted)),
            Span::raw("Choose an LLM provider and model  "),
            Span::styled("/model", Style::default().fg(theme.accent)),
        ]),
        Line::from(vec![
            Span::styled("2  ", Style::default().fg(theme.muted)),
            Span::raw("Connect a read-only datasource"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Type / for commands.",
            Style::default().fg(theme.muted),
        )),
    ])
}

fn timeline_color(
    tone: timeline::TimelineTone,
    theme: &super::theme::YsdaTheme,
) -> ratatui::style::Color {
    match tone {
        timeline::TimelineTone::Neutral => theme.text,
        timeline::TimelineTone::Warning => theme.warning,
        timeline::TimelineTone::Danger => theme.error,
        timeline::TimelineTone::Success => theme.success,
    }
}

fn transcript_lines(app: &TuiApp) -> Text<'static> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    if app.transcript.is_empty() {
        let mut lines = Vec::new();
        if let Some(code) = &app.safe_warning {
            lines.push(Line::from(Span::styled(
                format!("Warning  {}", safe(code)),
                Style::default().fg(theme.warning),
            )));
        }
        return Text::from(lines);
    }
    let mut lines = Vec::new();
    for item in &app.transcript {
        match item {
            TranscriptItem::UserMessage(text) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "You",
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("  {}", safe(text))),
                ]));
                lines.push(Line::from(""));
            }
            TranscriptItem::Answer(answer) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "Ys-da",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", safe(&answer.state)),
                        Style::default().fg(theme.muted),
                    ),
                ]));
                lines.push(Line::from(format!("       {}", safe(&answer.conclusion))));
                let values = answer
                    .key_values
                    .iter()
                    .flatten()
                    .map(|value| safe(value))
                    .collect::<Vec<_>>()
                    .join(" · ");
                if !values.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("       {values}"),
                        Style::default()
                            .fg(theme.success)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
                if let Some(explanation) = &answer.explanation {
                    lines.push(Line::from(Span::styled(
                        format!("       {}", safe(explanation)),
                        Style::default().fg(theme.muted),
                    )));
                }
            }
            TranscriptItem::Clarification {
                question,
                recommended_default,
            } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "Ys-da",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  needs clarification", Style::default().fg(theme.warning)),
                ]));
                lines.push(Line::from(format!("       {}", safe(question))));
                if let Some(value) = recommended_default {
                    lines.push(Line::from(Span::styled(
                        format!("       Recommended: {}", safe(value)),
                        Style::default().fg(theme.muted),
                    )));
                }
            }
            TranscriptItem::Warning(text) => lines.push(Line::from(Span::styled(
                format!("Warning  {}", safe(text)),
                Style::default().fg(theme.warning),
            ))),
            TranscriptItem::Error(text) => lines.push(Line::from(Span::styled(
                format!("Error  {}", safe(text)),
                Style::default().fg(theme.error),
            ))),
        }
    }
    if let Some(status) = &app.runtime_status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("◦ {}", safe(status)),
            Style::default().fg(theme.muted),
        )));
    }
    if let Some(code) = &app.safe_warning {
        lines.push(Line::from(Span::styled(
            format!("Warning  {}", safe(code)),
            Style::default().fg(theme.warning),
        )));
    }
    Text::from(lines)
}

fn detail_lines(app: &TuiApp) -> Text<'static> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let Some(detail) = &app.detail else {
        return Text::from("No detail available");
    };
    let mut lines = vec![Line::from(Span::styled(
        safe(&detail.title),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(detail.lines.iter().map(|line| Line::from(safe(line))));
    Text::from(lines)
}

fn help_lines(app: &TuiApp) -> Text<'static> {
    let mut lines = vec![Line::from("Commands")];
    lines.extend(
        command_catalog()
            .iter()
            .map(|command| Line::from(format!("/{} · {}", command.name, command.description))),
    );
    lines.push(Line::from(format!("Keys · {}", context_key_hint(app))));
    Text::from(lines)
}

fn repair_lines(app: &TuiApp) -> Text<'static> {
    let mut lines = vec![Line::from("Workspace readiness needs repair")];
    if let Some(report) = &app.doctor_report {
        lines.extend(
            report
                .blocker_codes
                .iter()
                .map(|code| Line::from(format!("Blocker  {code}"))),
        );
        lines.extend(report.repairs.iter().map(|repair| Line::from(safe(repair))));
    }
    Text::from(lines)
}

fn render_bottom(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, _mode: LayoutMode) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    match app.transient {
        Some(TransientView::SlashPalette) => {
            frame.render_widget(Clear, area);
            let regions = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)])
                .split(area);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("› {}", safe(&app.composer.text())),
                    Style::default().fg(theme.text),
                )))
                .style(Style::default().bg(theme.surface))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.accent)),
                ),
                regions[0],
            );
            let rows = app
                .slash_palette
                .rows()
                .map(|(selected, command)| {
                    let marker = if selected { "›" } else { " " };
                    Line::from(vec![
                        Span::styled(
                            format!("{marker} /{:<14}", command.name),
                            Style::default().fg(if selected { theme.accent } else { theme.text }),
                        ),
                        Span::styled(
                            format!("  {}", command.description),
                            Style::default().fg(theme.muted),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(Text::from(rows)).style(Style::default().bg(theme.surface)),
                regions[1],
            );
        }
        Some(TransientView::ModePicker) => {
            frame.render_widget(Clear, area);
            let mut lines = vec![Line::from(Span::styled(
                "Mode",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))];
            if let Some(picker) = &app.mode_picker {
                lines.push(Line::from(format!("Search modes  {}", picker.query())));
                lines.extend(picker.rows().map(|(selected, mode)| {
                    let marker = if selected { "›" } else { " " };
                    Line::from(Span::styled(
                        format!("{marker} {}", mode.label()),
                        Style::default().fg(if selected { theme.accent } else { theme.text }),
                    ))
                }));
            }
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .style(Style::default().bg(theme.surface))
                    .block(
                        Block::default()
                            .borders(Borders::TOP)
                            .border_style(Style::default().fg(theme.border)),
                    ),
                area,
            );
        }
        Some(TransientView::ThemePicker) => {
            let lines = app
                .theme_names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let marker = if index == app.theme_selected {
                        "›"
                    } else {
                        " "
                    };
                    Line::from(Span::styled(
                        format!("{marker} {name}"),
                        Style::default().fg(if index == app.theme_selected {
                            theme.accent
                        } else {
                            theme.text
                        }),
                    ))
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .style(Style::default().bg(theme.surface))
                    .block(
                        Block::default()
                            .title("Themes")
                            .borders(Borders::TOP)
                            .border_style(Style::default().fg(theme.border)),
                    ),
                area,
            );
        }
        _ => {
            if matches!(
                app.navigation.current(),
                ContentRoute::ModelSelection | ContentRoute::ProviderManagement
            ) {
                render_composer_and_model_panel(frame, app, area);
                return;
            }
            let hint = footer_hint(app);
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Length(1)])
                .split(area);
            let composer = if app.composer.text().is_empty() {
                Span::styled(
                    "› Ask a governed data question…",
                    Style::default().fg(theme.muted),
                )
            } else {
                Span::styled(
                    format!("› {}", safe(&app.composer.text())),
                    Style::default().fg(theme.text),
                )
            };
            frame.render_widget(
                Paragraph::new(Line::from(composer))
                    .style(Style::default().bg(theme.surface))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(theme.accent)),
                    ),
                rows[0],
            );
            frame.render_widget(
                Paragraph::new(Line::from(hint))
                    .style(Style::default().fg(theme.muted).bg(theme.background)),
                rows[1],
            );
        }
    }
}

fn render_composer_and_model_panel(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    let composer = if app.composer.text().is_empty() {
        Span::styled(
            "› Ask a governed data question…",
            Style::default().fg(theme.muted),
        )
    } else {
        Span::styled(
            format!("› {}", safe(&app.composer.text())),
            Style::default().fg(theme.text),
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(composer))
            .style(Style::default().bg(theme.surface))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent)),
            ),
        rows[0],
    );

    let (base_title, raw_lines, hint) = match app.navigation.current() {
        ContentRoute::ModelSelection => (
            " Model Selection ",
            model_selection::render_lines(&app.model_selection_state),
            "↑↓ navigate  Enter select  e edit credentials  Tab/←→ switch  Esc back  Ctrl+C cancel",
        ),
        ContentRoute::ProviderManagement => (
            " Model Setup ",
            app.detail
                .as_ref()
                .map(|detail| detail.lines.clone())
                .unwrap_or_else(|| vec!["Provider setup unavailable".to_owned()]),
            "↑↓ navigate  Enter select  Esc back  Ctrl+C cancel",
        ),
        _ => unreachable!("model panel is rendered only on model routes"),
    };
    let content_capacity = usize::from(rows[1].height.saturating_sub(2));
    let title = if content_capacity <= 1 {
        match app.navigation.current() {
            ContentRoute::ModelSelection => " Model Selection · Esc ",
            ContentRoute::ProviderManagement => " Model Setup · Enter/Esc ",
            _ => base_title,
        }
    } else {
        base_title
    };
    let fitted_lines = fit_panel_lines(
        raw_lines
            .into_iter()
            .filter(|line| line != "Model Selection")
            .collect(),
        hint,
        content_capacity,
    );
    let lines = fitted_lines
        .into_iter()
        .map(|line| {
            let color = if line == hint {
                theme.muted
            } else if line.starts_with('→') || line.contains("← current") {
                theme.accent
            } else if line.contains("[needs setup]") || line.contains("Needs validation") {
                theme.warning
            } else if line.contains("Unavailable") || line.contains("Could not continue") {
                theme.error
            } else {
                theme.text
            };
            Line::from(Span::styled(
                safe_panel_line(&line),
                Style::default().fg(color),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, rows[1]);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::default().bg(theme.surface))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .border_style(Style::default().fg(theme.border)),
            ),
        rows[1],
    );
}

fn fit_panel_lines(raw_lines: Vec<String>, hint: &str, capacity: usize) -> Vec<String> {
    if capacity == 0 {
        return Vec::new();
    }
    let selected = raw_lines.iter().position(|line| line.starts_with('→'));
    if capacity == 1 {
        let essential = raw_lines.iter().position(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("code     ")
                || line.contains("waiting for browser")
                || line.contains("saved · enter")
                || line.contains("type or paste")
                || line.contains("enter ")
                || line.contains("unavailable")
                || line.contains("could not")
                || line.contains("sign-in")
                || line.contains("working")
        });
        return raw_lines
            .get(selected.or(essential).unwrap_or_default())
            .cloned()
            .into_iter()
            .collect();
    }

    let body_capacity = capacity.saturating_sub(1);
    let mut body = if raw_lines.len() <= body_capacity {
        raw_lines
    } else if body_capacity == 1 {
        vec![
            raw_lines
                .get(selected.unwrap_or_default())
                .cloned()
                .unwrap_or_default(),
        ]
    } else {
        let mut fitted = vec![raw_lines.first().cloned().unwrap_or_default()];
        let selected = selected.unwrap_or(1).max(1);
        let window = body_capacity - 1;
        let start = selected
            .saturating_sub(window - 1)
            .max(1)
            .min(raw_lines.len().saturating_sub(window));
        fitted.extend(raw_lines.into_iter().skip(start).take(window));
        fitted
    };
    body.push(hint.to_owned());
    body
}

fn safe(value: &str) -> String {
    safe_width(value, 240)
}

fn safe_panel_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(240)
        .collect()
}

fn safe_width(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(max_chars)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn mode_label(app: &TuiApp) -> &'static str {
    match app.query_mode {
        super::TuiQueryMode::Auto => "AUTO › QUERY",
        super::TuiQueryMode::Query => "QUERY",
    }
}

fn footer_hint(app: &TuiApp) -> String {
    format!("{}  {}", command_hint(), context_key_hint(app))
}

fn context_key_hint(app: &TuiApp) -> &'static str {
    match app.navigation.current() {
        ContentRoute::Timeline => timeline_key_hint(
            app.timeline_state.view().result_card.is_some(),
            app.query_submission_enabled(),
        ),
        ContentRoute::Artifact => "Esc back",
        ContentRoute::ModelSelection => "Tab switch · Enter select · Esc back",
        ContentRoute::ProviderManagement => "Esc cancel/back",
        ContentRoute::Diagnostics => "Esc back",
    }
}

fn timeline_key_hint(has_result_card: bool, submission_enabled: bool) -> &'static str {
    if has_result_card {
        "Enter open results"
    } else if submission_enabled {
        "Enter submit · Ctrl-C detach"
    } else {
        "Set up /model to begin · Ctrl-C detach"
    }
}

fn render_too_small(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let key = if app.navigation.current() == ContentRoute::Timeline {
        "Ctrl-C detach"
    } else {
        "Esc back"
    };
    let text = Text::from(vec![
        Line::from(Span::styled(
            "YS·DA",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("Terminal too small · resize to at least 60×12"),
        Line::from(format!("Composer · {}", safe(&app.composer.text()))),
        Line::from(key),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(theme.text).bg(theme.background))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub fn render_to_string(app: &TuiApp, width: u16, height: u16) -> String {
    let mut app = app.clone();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("render test app");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
fn _detail_kind_is_used(kind: DetailKind) -> DetailKind {
    kind
}

#[cfg(test)]
mod tests {
    use super::{fit_panel_lines, timeline_key_hint};

    #[test]
    fn completed_timeline_footer_prioritizes_open_results() {
        assert_eq!(timeline_key_hint(true, true), "Enter open results");
        assert_eq!(timeline_key_hint(true, false), "Enter open results");
    }

    #[test]
    fn compact_panel_keeps_the_highlighted_row_visible() {
        let rows = vec![
            "[Providers]  Plans".to_owned(),
            "  first".to_owned(),
            "  second".to_owned(),
            "→ selected".to_owned(),
            "  fourth".to_owned(),
        ];
        assert_eq!(
            fit_panel_lines(rows.clone(), "Esc back", 1),
            vec!["→ selected"]
        );
        let fitted = fit_panel_lines(rows, "Esc back", 3);
        assert!(fitted.iter().any(|line| line == "→ selected"));
        assert_eq!(fitted.last().map(String::as_str), Some("Esc back"));
    }

    #[test]
    fn compact_provider_setup_keeps_the_actionable_authentication_line_visible() {
        let oauth = vec![
            "Configure ChatGPT Subscription".to_owned(),
            "Browser  https://example.invalid/device".to_owned(),
            "Code     ABCD-EFGH".to_owned(),
            "Waiting for browser sign-in… · Esc cancel".to_owned(),
        ];
        assert_eq!(
            fit_panel_lines(oauth, "Esc back", 1),
            vec!["Code     ABCD-EFGH"]
        );

        let api_key = vec![
            "Configure DeepSeek".to_owned(),
            "API Key".to_owned(),
            "Type or paste your API key".to_owned(),
            "Enter save and continue · Esc back".to_owned(),
        ];
        assert_eq!(
            fit_panel_lines(api_key, "Esc back", 1),
            vec!["Type or paste your API key"]
        );
    }
}
