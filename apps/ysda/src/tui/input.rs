use std::fmt;

use ys_agent_core::{ArtifactId, ExportFormat, RunId, TaskId};

use super::{
    palette::{CommandKind, command_catalog, command_hint},
    theme::{ColorSpec, ThemeToken},
};

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
    Datasource,
    Providers,
    Mode,
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
    let name = command.strip_prefix('/').expect("slash command");
    let Some(spec) = command_catalog().iter().find(|spec| spec.name == name) else {
        return Err(InputError::new(format!(
            "unknown command {command}; available commands: {}",
            command_hint()
        )));
    };
    if words.next().is_some() {
        return Err(InputError::new("this command accepts no arguments"));
    }
    Ok(match spec.kind {
        CommandKind::Mode => InputAction::Mode,
        CommandKind::Model => InputAction::Model,
        CommandKind::Datasource => InputAction::Datasource,
        CommandKind::Exit => InputAction::Quit,
    })
}

#[cfg(test)]
mod tests {
    use super::{InputAction, parse_input};

    #[test]
    fn only_catalog_commands_have_parser_paths() {
        assert_eq!(parse_input("/mode"), Ok(InputAction::Mode));
        assert_eq!(parse_input("/model"), Ok(InputAction::Model));
        assert_eq!(parse_input("/datasource"), Ok(InputAction::Datasource));
        assert_eq!(parse_input("/connections"), Ok(InputAction::Datasource));
        assert_eq!(parse_input("/exit"), Ok(InputAction::Quit));

        for retired in [
            "/new",
            "/tasks",
            "/task new x",
            "/resume 00000000-0000-0000-0000-000000000000",
            "/cancel 00000000-0000-0000-0000-000000000000",
            "/doctor",
            "/metrics",
            "/query",
            "/checks",
            "/artifact",
            "/sql",
            "/details",
            "/providers",
            "/theme",
            "/help",
            "/quit",
            "/export 00000000-0000-0000-0000-000000000000 json",
        ] {
            assert!(
                parse_input(retired).is_err(),
                "retired parser path: {retired}"
            );
        }
    }
}
