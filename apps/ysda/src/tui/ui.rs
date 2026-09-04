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

/// The visual prototype deliberately uses a bounded terminal shell on a large viewport.  Without
/// this cap, every hierarchy in the conversation becomes excessively wide and the response card
/// loses the dense, deliberate shape of the approved TUI.
const WIDE_SHELL_MAX_WIDTH: u16 = 128;

enum BodySection {
    Text(Text<'static>),
    Card {
        heading: Option<Text<'static>>,
        content: Text<'static>,
        interactive: bool,
    },
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
    let shell_area = centered_shell_area(area, mode);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        shell_area,
    );
    let shell = shell_area.inner(Margin {
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

fn centered_shell_area(area: Rect, mode: LayoutMode) -> Rect {
    if mode != LayoutMode::Wide || area.width <= WIDE_SHELL_MAX_WIDTH {
        return area;
    }
    let width = WIDE_SHELL_MAX_WIDTH;
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y,
        width,
        height: area.height,
    }
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
    let (access, access_style) = if header.read_only.eq_ignore_ascii_case("read-only") {
        (
            "◆ READ ONLY".to_owned(),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::SLOW_BLINK),
        )
    } else if header.active_model_available {
        (
            "◆ CHAT ONLY".to_owned(),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::SLOW_BLINK),
        )
    } else {
        (
            "◆ SETUP REQUIRED".to_owned(),
            Style::default().fg(theme.warning),
        )
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
    left.push(Span::styled(access, access_style));
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
                        if has_governed_timeline(&app.timeline_state) {
                            return render_governed_timeline_body(frame, app, area);
                        } else if app.transcript.is_empty() && app.safe_warning.is_none() {
                            return render_text_body(frame, app, area, welcome_lines(app));
                        } else if app
                            .transcript
                            .iter()
                            .any(|item| matches!(item, TranscriptItem::Answer(_)))
                        {
                            return render_transcript_body(frame, app, area);
                        } else {
                            transcript_lines(app)
                        }
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

fn render_governed_timeline_body(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let sections = governed_timeline_sections(app);
    render_body_sections(frame, app, area, sections);
}

fn render_transcript_body(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let sections = transcript_sections(app);
    render_body_sections(frame, app, area, sections);
}

fn render_body_sections(
    frame: &mut Frame<'_>,
    app: &mut TuiApp,
    area: Rect,
    sections: Vec<BodySection>,
) {
    let total_height = sections
        .iter()
        .map(|section| body_section_height(section, area.width))
        .sum::<usize>();
    let scroll = resolved_body_scroll(app, total_height, area.height);
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let mut top = 0_usize;
    app.timeline_state.result_card_hit_region = None;

    for section in sections {
        match section {
            BodySection::Text(text) => {
                let height = text_height(&text, area.width);
                render_flow_text(frame, area, top, height, scroll, text, theme);
                top = top.saturating_add(height);
            }
            BodySection::Card {
                heading,
                content,
                interactive,
            } => {
                if let Some(heading) = heading {
                    let height = text_height(&heading, area.width);
                    render_flow_text(frame, area, top, height, scroll, heading, theme);
                    top = top.saturating_add(height);
                }
                let height = card_height(&content, area.width);
                if let Some(card_area) = visible_flow_area(area, top, height, scroll) {
                    render_result_card(frame, card_area, content, theme);
                    if interactive {
                        app.timeline_state.result_card_hit_region = Some(timeline::HitRegion::new(
                            card_area.x,
                            card_area.y,
                            card_area.width,
                            card_area.height,
                        ));
                    }
                }
                top = top.saturating_add(height);
            }
        }
    }
}

fn body_section_height(section: &BodySection, width: u16) -> usize {
    match section {
        BodySection::Text(text) => text_height(text, width),
        BodySection::Card {
            heading, content, ..
        } => heading
            .as_ref()
            .map(|heading| text_height(heading, width))
            .unwrap_or_default()
            .saturating_add(card_height(content, width)),
    }
}

fn text_height(text: &Text<'_>, width: u16) -> usize {
    let width = usize::from(width.max(1));
    text.lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
}

fn card_height(content: &Text<'_>, width: u16) -> usize {
    let content_width = width.saturating_sub(4).max(1);
    text_height(content, content_width).saturating_add(2)
}

fn resolved_body_scroll(app: &TuiApp, total_height: usize, visible_height: u16) -> usize {
    let maximum = total_height.saturating_sub(usize::from(visible_height));
    if app.scroll == u16::MAX {
        maximum
    } else {
        usize::from(app.scroll).min(maximum)
    }
}

fn visible_flow_area(area: Rect, top: usize, height: usize, scroll: usize) -> Option<Rect> {
    let viewport_end = scroll.saturating_add(usize::from(area.height));
    let bottom = top.saturating_add(height);
    let visible_top = top.max(scroll);
    let visible_bottom = bottom.min(viewport_end);
    if visible_top >= visible_bottom {
        return None;
    }
    Some(Rect {
        x: area.x,
        y: area
            .y
            .saturating_add(u16::try_from(visible_top.saturating_sub(scroll)).unwrap_or(u16::MAX)),
        width: area.width,
        height: u16::try_from(visible_bottom.saturating_sub(visible_top)).unwrap_or(u16::MAX),
    })
}

fn render_flow_text(
    frame: &mut Frame<'_>,
    area: Rect,
    top: usize,
    height: usize,
    scroll: usize,
    text: Text<'static>,
    theme: &super::theme::YsdaTheme,
) {
    let Some(visible_area) = visible_flow_area(area, top, height, scroll) else {
        return;
    };
    let skipped_rows = scroll.saturating_sub(top).min(usize::from(u16::MAX));
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(theme.text).bg(theme.background))
            .wrap(Wrap { trim: false })
            .scroll((skipped_rows as u16, 0)),
        visible_area,
    );
}

fn render_result_card(
    frame: &mut Frame<'_>,
    area: Rect,
    content: Text<'static>,
    theme: &super::theme::YsdaTheme,
) {
    if area.width < 3 || area.height < 3 {
        frame.render_widget(
            Paragraph::new(content)
                .style(Style::default().fg(theme.text).bg(theme.background))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    frame.render_widget(
        Block::default()
            .style(Style::default().bg(theme.background))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
        area,
    );
    // The rail is intentionally an interior layer: keeping the corner glyphs intact makes this
    // a proper panel while the bright left edge identifies a result without a decorative
    // text-art pseudo-card.
    let rail_height = area.height.saturating_sub(2);
    let rail_area = Rect {
        x: area.x,
        y: area.y.saturating_add(1),
        width: 1,
        height: rail_height,
    };
    let rail = (0..rail_height)
        .map(|_| Line::from(Span::styled("│", Style::default().fg(theme.accent))))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(rail)), rail_area);
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    if inner.width > 0 && inner.height > 0 {
        frame.render_widget(
            Paragraph::new(content)
                .style(Style::default().fg(theme.text).bg(theme.background))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
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
            "Chat now; query your data when it is connected.",
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
            Span::raw("Optionally connect a read-only datasource for data questions"),
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

/// A timeline is evidence for a governed Run, not a decorative treatment for every chat reply.
/// Plain conversations retain their compact transcript cards; typed Run state and events earn the
/// richer execution view below.
fn has_governed_timeline(state: &timeline::TimelineState) -> bool {
    let view = state.view();
    view.status != timeline::TimelineStatus::Idle
        || !view.stages.is_empty()
        || view.notice.is_some()
        || view.result_card.is_some()
}

fn governed_timeline_sections(app: &TuiApp) -> Vec<BodySection> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let view = app.timeline_state.view();
    let mut sections = Vec::new();

    if let Some(question) = view.question {
        sections.push(BodySection::Text(Text::from(vec![
            Line::from(Span::styled(
                "You",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            )),
            Line::from(Span::styled(
                safe(question),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )),
        ])));
        sections.push(BodySection::Text(Text::from("")));
    }

    let mut lines = Vec::new();
    let status_color = timeline_color(view.status.tone(), theme);
    lines.push(Line::from(vec![
        Span::styled(
            "● ",
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "Ys-da",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!("  {}", timeline_completion_label(view.status)),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ),
    ]));

    if view.stages.is_empty()
        && matches!(
            view.status,
            timeline::TimelineStatus::Scheduled | timeline::TimelineStatus::Running
        )
    {
        lines.push(timeline_stage_line("Preparing governed workflow…", theme));
    } else {
        lines.extend(
            view.stages
                .iter()
                .map(|stage| timeline_stage_line(&stage.label, theme)),
        );
    }

    if let Some(notice) = view.notice {
        let notice_color = timeline_color(notice.tone, theme);
        lines.push(Line::from(vec![
            Span::styled("│  ", Style::default().fg(theme.border)),
            Span::styled("! ", Style::default().fg(notice_color)),
            Span::styled(safe(&notice.reason), Style::default().fg(notice_color)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("│  ", Style::default().fg(theme.border)),
            Span::styled("→ ", Style::default().fg(theme.accent)),
            Span::styled(safe(&notice.next_action), Style::default().fg(theme.muted)),
        ]));
    }

    sections.push(BodySection::Text(Text::from(lines)));
    if let Some(card) = view.result_card {
        sections.push(BodySection::Text(Text::from("")));
        sections.push(BodySection::Card {
            heading: None,
            content: query_result_card_content(card, theme),
            interactive: true,
        });
    }

    sections
}

fn query_result_card_content(
    card: &timeline::TimelineResultCard,
    theme: &super::theme::YsdaTheme,
) -> Text<'static> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "QUERY ARTIFACT INTERNAL",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            ),
            Span::styled("  ✓ VERIFIED", Style::default().fg(theme.success)),
        ]),
        Line::from(Span::styled(
            safe(&card.answer_summary),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "Verification · ",
                Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
            ),
            Span::styled(safe(&card.verification), Style::default().fg(theme.success)),
        ]),
    ];
    lines.extend(card.warnings.iter().map(|warning| {
        Line::from(vec![
            Span::styled("Warning · ", Style::default().fg(theme.warning)),
            Span::styled(safe(warning), Style::default().fg(theme.warning)),
        ])
    }));
    lines.push(Line::from(vec![
        Span::styled("↳ ", Style::default().fg(theme.accent)),
        Span::styled("Open results · Enter", Style::default().fg(theme.accent)),
    ]));
    Text::from(lines)
}

fn timeline_completion_label(status: timeline::TimelineStatus) -> String {
    match status {
        timeline::TimelineStatus::Succeeded => "completed".to_owned(),
        _ => status.label().to_ascii_lowercase(),
    }
}

fn timeline_stage_line(label: &str, theme: &super::theme::YsdaTheme) -> Line<'static> {
    let tool_or_policy_step = label.starts_with("Checking ")
        || label.starts_with("Governed operation")
        || label.starts_with("Policy check");
    Line::from(vec![
        Span::styled("│  ", Style::default().fg(theme.border)),
        Span::styled(
            "● ",
            Style::default().fg(if tool_or_policy_step {
                theme.accent
            } else {
                theme.success
            }),
        ),
        Span::styled(
            safe(label),
            Style::default().fg(if tool_or_policy_step {
                theme.accent
            } else {
                theme.text
            }),
        ),
    ])
}

fn transcript_sections(app: &TuiApp) -> Vec<BodySection> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let mut sections = Vec::new();

    for item in &app.transcript {
        if !sections.is_empty() {
            sections.push(BodySection::Text(Text::from("")));
        }
        match item {
            TranscriptItem::UserMessage(text) => {
                sections.push(BodySection::Text(Text::from(vec![
                    Line::from(Span::styled(
                        "You",
                        Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                    )),
                    Line::from(Span::styled(
                        safe(text),
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    )),
                ])))
            }
            TranscriptItem::Answer(answer) => sections.push(BodySection::Card {
                heading: Some(answer_heading(answer, theme)),
                content: response_card_content(answer, theme),
                interactive: false,
            }),
            TranscriptItem::Clarification {
                question,
                recommended_default,
            } => {
                let mut lines = vec![Line::from(vec![
                    Span::styled(
                        "Ys-da",
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  needs clarification", Style::default().fg(theme.warning)),
                ])];
                lines.push(Line::from(Span::styled(
                    safe(question),
                    Style::default().fg(theme.text),
                )));
                if let Some(value) = recommended_default {
                    lines.push(Line::from(Span::styled(
                        format!("Recommended: {}", safe(value)),
                        Style::default().fg(theme.muted),
                    )));
                }
                sections.push(BodySection::Text(Text::from(lines)));
            }
            TranscriptItem::Warning(text) => {
                sections.push(BodySection::Text(Text::from(Line::from(Span::styled(
                    format!("Warning  {}", safe(text)),
                    Style::default().fg(theme.warning),
                )))));
            }
            TranscriptItem::Error(text) => {
                sections.push(BodySection::Text(Text::from(Line::from(Span::styled(
                    format!("Error  {}", safe(text)),
                    Style::default().fg(theme.error),
                )))));
            }
        }
    }

    if let Some(status) = app
        .runtime_status
        .as_deref()
        .filter(|status| *status != "Ready")
    {
        if !sections.is_empty() {
            sections.push(BodySection::Text(Text::from("")));
        }
        sections.push(BodySection::Text(Text::from(Line::from(Span::styled(
            format!("◦ {}", safe(status)),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        )))));
    }
    if let Some(code) = &app.safe_warning {
        if !sections.is_empty() {
            sections.push(BodySection::Text(Text::from("")));
        }
        sections.push(BodySection::Text(Text::from(Line::from(Span::styled(
            format!("Warning  {}", safe(code)),
            Style::default().fg(theme.warning),
        )))));
    }

    sections
}

fn answer_heading(answer: &super::AnswerView, theme: &super::theme::YsdaTheme) -> Text<'static> {
    let mut spans = vec![
        Span::styled(
            "● ",
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "Ys-da",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ),
    ];
    if !answer.state.eq_ignore_ascii_case("chat") {
        spans.push(Span::styled(
            format!("  {}", safe(&answer.state)),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }
    Text::from(Line::from(spans))
}

fn response_card_content(
    answer: &super::AnswerView,
    theme: &super::theme::YsdaTheme,
) -> Text<'static> {
    let mut lines = vec![Line::from(Span::styled(
        safe(&answer.conclusion),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ))];
    let values = answer
        .key_values
        .iter()
        .flatten()
        .map(|value| safe(value))
        .collect::<Vec<_>>()
        .join(" · ");
    if !values.is_empty() {
        lines.push(Line::from(Span::styled(
            values,
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(explanation) = &answer.explanation {
        lines.push(Line::from(Span::styled(
            safe(explanation),
            Style::default().fg(theme.muted),
        )));
    }
    Text::from(lines)
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
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        match item {
            TranscriptItem::UserMessage(text) => {
                lines.push(Line::from(Span::styled(
                    "You",
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                )));
                lines.push(Line::from(Span::styled(
                    safe(text),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                )));
            }
            TranscriptItem::Answer(answer) => {
                lines.extend(answer_heading(answer, theme).lines);
                lines.extend(response_card_content(answer, theme).lines);
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
    if let Some(status) = app
        .runtime_status
        .as_deref()
        .filter(|status| *status != "Ready")
    {
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
                Span::styled("› Ask a question…", Style::default().fg(theme.muted))
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
        Span::styled("› Ask a question…", Style::default().fg(theme.muted))
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
    // Model activation runs asynchronously.  Its result must remain visible in the model panel,
    // where the user initiated it, instead of being written only to the hidden transcript body.
    let status_line = (app.navigation.current() == ContentRoute::ModelSelection)
        .then_some(app.runtime_status.as_deref())
        .flatten()
        .map(|status| format!("Status · {status}"));
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
    let mut fitted_lines = fit_panel_lines(
        raw_lines
            .into_iter()
            .filter(|line| line != "Model Selection")
            .collect(),
        hint,
        content_capacity.saturating_sub(usize::from(status_line.is_some())),
    );
    if let Some(status_line) = status_line {
        let insert_at = fitted_lines.len().saturating_sub(1);
        fitted_lines.insert(insert_at, status_line);
    }
    let lines = fitted_lines
        .into_iter()
        .map(|line| {
            let color = if line == hint {
                theme.muted
            } else if line.starts_with('→') || line.contains("← current") {
                theme.accent
            } else if line.contains("[needs setup]") || line.contains("Needs validation") {
                theme.warning
            } else if line.contains("Unavailable")
                || line.contains("Could not continue")
                || line.contains("activation failed")
            {
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
            app.conversation_submission_enabled(),
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
