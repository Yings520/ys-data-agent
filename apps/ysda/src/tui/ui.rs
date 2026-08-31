use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::{
    TransientView, TuiApp,
    app::{DetailKind, TranscriptItem},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Compact,
    Standard,
    Wide,
}

impl LayoutMode {
    pub fn resolve(area: Rect) -> Self {
        if area.width < 80 || area.height < 20 {
            Self::Compact
        } else if area.width >= 130 {
            Self::Wide
        } else {
            Self::Standard
        }
    }
}

pub fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = frame.area();
    let mode = LayoutMode::resolve(area);
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.text)),
        area,
    );
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(bottom_panel_height(app, area)),
        ])
        .split(area);
    render_header(frame, app, regions[0], mode);
    let body = if mode == LayoutMode::Wide {
        regions[1].inner(Margin {
            horizontal: 4,
            vertical: 0,
        })
    } else {
        regions[1]
    };
    render_body(frame, app, body, mode);
    render_bottom(frame, app, regions[2], mode);
}

pub fn bottom_panel_height(app: &TuiApp, area: Rect) -> u16 {
    match app.transient {
        Some(TransientView::SlashPalette | TransientView::ThemePicker) => {
            10_u16.min(area.height.saturating_sub(2))
        }
        _ => 4_u16.min(area.height.saturating_sub(2)),
    }
}

fn render_header(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, mode: LayoutMode) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let readiness = match &app.doctor_report {
        Some(report) if report.allows_query_submission() => ("ready", theme.success),
        Some(_) => ("blocked", theme.warning),
        None => ("not checked", theme.muted),
    };
    let mut spans = vec![
        Span::styled(
            "Agent",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(safe(&app.workspace_name), Style::default().fg(theme.text)),
        Span::styled(
            format!(" · {}", readiness.0),
            Style::default().fg(readiness.1),
        ),
    ];
    if mode != LayoutMode::Compact {
        spans.extend([
            Span::styled(
                format!(" · {}", safe(&app.model_label)),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!(" · {}", safe(&app.connection_label)),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!(" · {}", safe(&app.permission_label)),
                Style::default().fg(theme.muted),
            ),
        ]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.header)),
        area,
    );
}

fn render_body(frame: &mut Frame<'_>, app: &TuiApp, area: Rect, mode: LayoutMode) {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    let text = match app.transient {
        Some(TransientView::Detail(_)) => detail_lines(app),
        Some(TransientView::Help) => help_lines(),
        Some(TransientView::Repair) => repair_lines(app),
        _ => transcript_lines(app),
    };
    let borders = if mode == LayoutMode::Compact {
        Borders::NONE
    } else {
        Borders::BOTTOM
    };
    let rendered_height = text
        .lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(usize::from(area.width.max(1))))
        .sum::<usize>();
    let visible_height = area
        .height
        .saturating_sub(u16::from(mode != LayoutMode::Compact));
    let scroll = if app.scroll == u16::MAX {
        rendered_height
            .saturating_sub(usize::from(visible_height))
            .try_into()
            .unwrap_or(u16::MAX)
    } else {
        app.scroll
    };
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(theme.text).bg(theme.background))
        .block(
            Block::default()
                .borders(borders)
                .border_style(Style::default().fg(theme.border)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

fn transcript_lines(app: &TuiApp) -> Text<'static> {
    let theme = app.preview_theme.as_ref().unwrap_or(&app.active_theme);
    if app.transcript.is_empty() {
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Ask a governed data question.",
                Style::default().fg(theme.muted),
            )),
        ];
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
        detail.title.clone(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(detail.lines.iter().map(|line| Line::from(safe(line))));
    Text::from(lines)
}

fn help_lines() -> Text<'static> {
    Text::from(vec![
        Line::from("Commands"),
        Line::from("/new · /tasks · /task new TEXT · /resume TASK_ID · /cancel RUN_ID"),
        Line::from("/metrics · /query · /checks · /artifact [ARTIFACT_ID] · /sql · /details"),
        Line::from("/export ARTIFACT_ID json|csv|markdown · /doctor · /theme · /quit"),
    ])
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
            let rows = app
                .slash_palette
                .rows()
                .map(|(selected, command)| {
                    let marker = if selected { "›" } else { " " };
                    Line::from(vec![
                        Span::styled(
                            format!("{marker} /{}", command.name),
                            Style::default().fg(if selected { theme.accent } else { theme.text }),
                        ),
                        Span::styled(
                            format!("  {}", command.description),
                            Style::default().fg(theme.muted),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            let mut lines = vec![
                Line::from(Span::styled(
                    "Commands",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from("Search commands"),
            ];
            lines.extend(rows);
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
            let hint = if app.query_submission_enabled() {
                "Enter submit · / commands · Ctrl-C detach"
            } else {
                "Run /doctor to check readiness · Ctrl-C detach"
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(safe(&app.composer.text())),
                    Line::from(hint),
                ])
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme.border)),
                ),
                area,
            );
        }
    }
}

fn safe(value: &str) -> String {
    value.replace('\u{1b}', "?")
}

pub fn render_to_string(app: &TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, app))
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
