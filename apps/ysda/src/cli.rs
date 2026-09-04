use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactId, CommandId, CoreError, CoreResult,
    ExportFormat, RunId, RunStatus, Sensitivity, TaskId,
};
use ys_agent_runtime::{AgentServiceApi, SendMessageRequest, ServiceReply};

use crate::{bootstrap::AppDependencies, tui::run_tui};

#[derive(Debug, Parser)]
#[command(name = "ysda", version, about = "YS Data Agent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Doctor,
    Run {
        question: String,
    },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Artifact {
        artifact_id: ArtifactId,
        #[arg(long, value_enum)]
        format: Option<ExportFormatArg>,
    },
    Schema {
        source_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    List,
    Resume { task_id: TaskId },
    Cancel { run_id: RunId },
    Answer { run_id: RunId, answer: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormatArg {
    Json,
    Csv,
    Markdown,
}

impl From<ExportFormatArg> for ExportFormat {
    fn from(value: ExportFormatArg) -> Self {
        match value {
            ExportFormatArg::Json => Self::Json,
            ExportFormatArg::Csv => Self::Csv,
            ExportFormatArg::Markdown => Self::Markdown,
        }
    }
}

pub async fn dispatch(cli: Cli, dependencies: AppDependencies) -> CoreResult<()> {
    match cli.command {
        None => run_tui(dependencies).await,
        Some(Command::Doctor) => {
            println!("{}", dependencies.service.doctor().await?);
            Ok(())
        }
        Some(Command::Run { question }) => dispatch_run(&dependencies, question).await,
        Some(Command::Task { command }) => dispatch_task(&dependencies, command).await,
        Some(Command::Artifact {
            artifact_id,
            format,
        }) => dispatch_artifact(&dependencies, artifact_id, format).await,
        Some(Command::Schema { source_id }) => dispatch_schema(&dependencies, source_id).await,
    }
}

async fn dispatch_run(dependencies: &AppDependencies, question: String) -> CoreResult<()> {
    let session = dependencies
        .service
        .create_session(CommandId::new(), dependencies.principal.clone())
        .await?;
    match dependencies
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            question,
        ))
        .await?
    {
        ServiceReply::Conversation { message } => println!("{message}"),
        ServiceReply::RunScheduled { run_id, .. } => {
            println!("Run scheduled: {run_id}");
            wait_for_terminal_run(dependencies.service.as_ref(), &run_id).await?;
        }
        ServiceReply::ClarificationRequired { question, .. } => {
            println!("Clarification required: {question}")
        }
        ServiceReply::UnsupportedCapability { message, .. } => println!("{message}"),
    }
    Ok(())
}

async fn wait_for_terminal_run(service: &dyn AgentServiceApi, run_id: &RunId) -> CoreResult<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let snapshot = service.get_run(run_id).await?;
        match snapshot.status {
            RunStatus::Succeeded => {
                println!("Run succeeded: {run_id}");
                if let Some(artifact_id) = snapshot.primary_artifact_id {
                    println!("Primary artifact: {artifact_id}");
                }
                return Ok(());
            }
            RunStatus::Failed => {
                println!("Run failed: {run_id}");
                return Err(CoreError::validation(
                    "run_failed",
                    "the Query Run ended in Failed",
                ));
            }
            RunStatus::Cancelled => {
                println!("Run cancelled: {run_id}");
                return Err(CoreError::validation(
                    "run_cancelled",
                    "the Query Run was cancelled",
                ));
            }
            RunStatus::WaitingForInput => {
                println!("Run waiting for input: {run_id}");
                return Ok(());
            }
            RunStatus::Queued | RunStatus::Running => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(CoreError::validation(
                        "run_wait_timeout",
                        "timed out waiting for the Query Run to finish",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn dispatch_task(dependencies: &AppDependencies, command: TaskCommand) -> CoreResult<()> {
    match command {
        TaskCommand::List => {
            for task in dependencies
                .service
                .list_tasks(&dependencies.workspace_id)
                .await?
            {
                println!("{}\t{:?}\t{}", task.id, task.status, task.goal);
            }
            Ok(())
        }
        TaskCommand::Resume { task_id } => {
            ensure_query_ready(dependencies, "Task resume").await?;
            let run_id = dependencies
                .service
                .resume_task(CommandId::new(), &task_id)
                .await?;
            println!("Run scheduled: {run_id}");
            Ok(())
        }
        TaskCommand::Cancel { run_id } => {
            dependencies
                .service
                .cancel_run(CommandId::new(), &run_id, "cancelled from CLI".to_owned())
                .await?;
            println!("Cancelled: {run_id}");
            Ok(())
        }
        TaskCommand::Answer { run_id, answer } => {
            ensure_query_ready(dependencies, "clarification answer").await?;
            dependencies
                .service
                .answer_clarification(CommandId::new(), &run_id, answer)
                .await?;
            println!("Clarification accepted; resuming the same Run: {run_id}");
            wait_for_terminal_run(dependencies.service.as_ref(), &run_id).await
        }
    }
}

async fn dispatch_artifact(
    dependencies: &AppDependencies,
    artifact_id: ArtifactId,
    format: Option<ExportFormatArg>,
) -> CoreResult<()> {
    let access = ArtifactAccessContext {
        workspace_id: dependencies.workspace_id,
        principal_id: dependencies.principal.id,
        purpose: if format.is_some() {
            ArtifactAccessPurpose::Export
        } else {
            ArtifactAccessPurpose::TuiPreview
        },
        max_sensitivity: Sensitivity::Internal,
    };
    if let Some(format) = format {
        let metadata = dependencies
            .service
            .export_artifact(CommandId::new(), &artifact_id, format.into(), access)
            .await?;
        println!("{}", metadata.storage_uri);
    } else {
        let view = dependencies
            .service
            .get_artifact(&artifact_id, access)
            .await?;
        println!("{:?}\t{}", view.metadata.kind, view.metadata.content_hash);
        println!("{}", String::from_utf8_lossy(&view.preview));
        if view.truncated {
            println!("[preview truncated]");
        }
    }
    Ok(())
}

async fn dispatch_schema(dependencies: &AppDependencies, source_id: String) -> CoreResult<()> {
    ensure_query_ready(dependencies, "Metadata submission").await?;
    let session = dependencies
        .service
        .create_session(CommandId::new(), dependencies.principal.clone())
        .await?;
    let reply = dependencies
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            format!("What relations and columns are available in {source_id}?"),
        ))
        .await?;
    println!("{reply:?}");
    Ok(())
}

async fn ensure_query_ready(dependencies: &AppDependencies, operation: &str) -> CoreResult<()> {
    let report = dependencies.service.doctor().await?;
    if report.allows_query_submission() {
        return Ok(());
    }
    println!("{report}");
    Err(CoreError::validation(
        "workspace_not_ready",
        format!("Doctor blockers disable {operation}"),
    ))
}
