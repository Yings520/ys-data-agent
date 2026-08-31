use clap::Parser;
use ysda::cli::{Cli, Command, ExportFormatArg, TaskCommand};

const EXAMPLE_ID: &str = "3d315500-ec47-4ce3-83ee-4284ec34cdbc";

#[test]
fn no_subcommand_selects_interactive_tui() {
    let cli = Cli::try_parse_from(["ysda"]).expect("no-argument CLI");
    assert!(cli.command.is_none());
}

#[test]
fn parses_non_interactive_run() {
    let cli = Cli::try_parse_from(["ysda", "run", "last seven days GMV"]).expect("run command");

    assert!(matches!(
        cli.command,
        Some(Command::Run { question }) if question == "last seven days GMV"
    ));
}

#[test]
fn parses_task_resume() {
    let cli =
        Cli::try_parse_from(["ysda", "task", "resume", EXAMPLE_ID]).expect("task resume command");

    assert!(matches!(
        cli.command,
        Some(Command::Task {
            command: TaskCommand::Resume { .. }
        })
    ));
}

#[test]
fn parses_doctor_and_safe_export() {
    let doctor = Cli::try_parse_from(["ysda", "doctor"]).expect("doctor command");
    assert!(matches!(doctor.command, Some(Command::Doctor)));

    let export = Cli::try_parse_from(["ysda", "artifact", EXAMPLE_ID, "--format", "markdown"])
        .expect("artifact export command");

    assert!(matches!(
        export.command,
        Some(Command::Artifact {
            format: Some(ExportFormatArg::Markdown),
            ..
        })
    ));
}
