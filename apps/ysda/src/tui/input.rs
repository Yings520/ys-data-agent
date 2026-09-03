use std::{fmt, str::FromStr};

use ys_agent_core::{ArtifactId, ExportFormat, RunId, TaskId};

use super::theme::{ColorSpec, ThemeToken};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailRequest {
    Metrics,
    Query,
    Checks,
    Artifact(Option<ArtifactId>),
    Sql,
    Diagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Empty,
    NewSession,
    ListTasks,
    NewTask(String),
    ResumeTask {
        task_id: TaskId,
    },
    CancelRun {
        run_id: RunId,
    },
    ShowDetail(DetailRequest),
    ExportArtifact {
        artifact_id: ArtifactId,
        format: ExportFormat,
    },
    Doctor,
    OpenThemePicker,
    SetThemeColor {
        token: ThemeToken,
        color: ColorSpec,
    },
    ResetTheme,
    Connections,
    Providers,
    Model,
    Help,
    Quit,
    SendMessage(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputError(String);

impl InputError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for InputError {}

pub fn parse_input(raw: &str) -> Result<InputAction, InputError> {
    let input = raw.trim();
    if input.is_empty() {
        return Ok(InputAction::Empty);
    }
    if !input.starts_with('/') {
        return Ok(InputAction::SendMessage(input.to_owned()));
    }
    let mut words = input.split_whitespace();
    let command = words.next().expect("non-empty command");
    match command {
        "/new" => exact_no_args(words, InputAction::NewSession),
        "/tasks" => exact_no_args(words, InputAction::ListTasks),
        "/doctor" => exact_no_args(words, InputAction::Doctor),
        "/metrics" => exact_no_args(words, InputAction::ShowDetail(DetailRequest::Metrics)),
        "/query" => exact_no_args(words, InputAction::ShowDetail(DetailRequest::Query)),
        "/checks" => exact_no_args(words, InputAction::ShowDetail(DetailRequest::Checks)),
        "/sql" => exact_no_args(words, InputAction::ShowDetail(DetailRequest::Sql)),
        "/details" => exact_no_args(words, InputAction::ShowDetail(DetailRequest::Diagnostics)),
        "/connections" => exact_no_args(words, InputAction::Connections),
        "/providers" => exact_no_args(words, InputAction::Providers),
        "/model" => exact_no_args(words, InputAction::Model),
        "/help" => exact_no_args(words, InputAction::Help),
        "/quit" => exact_no_args(words, InputAction::Quit),
        "/theme" => parse_theme(words.collect()),
        "/task" => parse_task(words.collect()),
        "/resume" => parse_one_id(words.collect(), "TASK_ID", |task_id| {
            InputAction::ResumeTask { task_id }
        }),
        "/cancel" => parse_one_id(words.collect(), "RUN_ID", |run_id| InputAction::CancelRun {
            run_id,
        }),
        "/artifact" => parse_optional_artifact(words.collect()),
        "/export" => parse_export(words.collect()),
        _ => Err(InputError::new(format!(
            "unknown command {command}; / starts commands, delete the leading / to send chat, or type /help"
        ))),
    }
}

fn exact_no_args<'a>(
    mut words: impl Iterator<Item = &'a str>,
    action: InputAction,
) -> Result<InputAction, InputError> {
    if words.next().is_some() {
        Err(InputError::new("this command accepts no arguments"))
    } else {
        Ok(action)
    }
}

fn parse_task(words: Vec<&str>) -> Result<InputAction, InputError> {
    match words.as_slice() {
        ["new", text @ ..] if !text.is_empty() => Ok(InputAction::NewTask(text.join(" "))),
        _ => Err(InputError::new("usage: /task new TEXT")),
    }
}

fn parse_optional_artifact(words: Vec<&str>) -> Result<InputAction, InputError> {
    match words.as_slice() {
        [] => Ok(InputAction::ShowDetail(DetailRequest::Artifact(None))),
        [raw] => raw
            .parse::<ArtifactId>()
            .map(|artifact_id| InputAction::ShowDetail(DetailRequest::Artifact(Some(artifact_id))))
            .map_err(|_| InputError::new("invalid ARTIFACT_ID")),
        _ => Err(InputError::new("usage: /artifact [ARTIFACT_ID]")),
    }
}

fn parse_theme(words: Vec<&str>) -> Result<InputAction, InputError> {
    match words.as_slice() {
        [] => Ok(InputAction::OpenThemePicker),
        ["reset"] => Ok(InputAction::ResetTheme),
        ["set", raw_token, raw_color] => Ok(InputAction::SetThemeColor {
            token: raw_token
                .parse::<ThemeToken>()
                .map_err(|error| InputError::new(error.to_string()))?,
            color: ColorSpec::parse(raw_color)
                .map_err(|error| InputError::new(error.to_string()))?,
        }),
        _ => Err(InputError::new(
            "usage: /theme | /theme set TOKEN COLOR | /theme reset",
        )),
    }
}

fn parse_one_id<T, F>(words: Vec<&str>, label: &str, build: F) -> Result<InputAction, InputError>
where
    T: FromStr,
    F: FnOnce(T) -> InputAction,
{
    let [raw] = words.as_slice() else {
        return Err(InputError::new(format!("expected exactly one {label}")));
    };
    raw.parse()
        .map(build)
        .map_err(|_| InputError::new(format!("invalid {label}")))
}

fn parse_export(words: Vec<&str>) -> Result<InputAction, InputError> {
    let [raw_id, raw_format] = words.as_slice() else {
        return Err(InputError::new(
            "usage: /export ARTIFACT_ID json|csv|markdown",
        ));
    };
    let artifact_id = raw_id
        .parse()
        .map_err(|_| InputError::new("invalid ARTIFACT_ID"))?;
    let format = match *raw_format {
        "json" => ExportFormat::Json,
        "csv" => ExportFormat::Csv,
        "markdown" => ExportFormat::Markdown,
        _ => return Err(InputError::new("format must be json, csv, or markdown")),
    };
    Ok(InputAction::ExportArtifact {
        artifact_id,
        format,
    })
}
