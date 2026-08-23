use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;
use ys_agent_adapters::ResultPolicy;
use ys_agent_core::SourceId;

use crate::agent::QueryAgent;
use crate::domain::UserQuestion;
use crate::error::{AppError, AppResult};
use crate::llm::{LlmClient, LlmConfig};
use crate::output::{render_run, render_schema};
use crate::trace::TraceRecorder;

const DEFAULT_MAX_ROWS: usize = 100;

#[derive(Debug, Parser)]
#[command(
    name = "ysda",
    version,
    about = "YS Data Agent:  a safe Rust Data Query Agent"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    Schema {
        #[arg(long)]
        database: PathBuf,
        #[arg(long, default_value = "fixtures/policy/query-policy.json")]
        policy: PathBuf,
        #[arg(long, default_value = "sqlite_demo")]
        source_id: String,
    },
    Ask {
        #[arg(long)]
        database: PathBuf,
        question: String,
        #[arg(long, default_value = ".ysda/traces")]
        trace_dir: PathBuf,
        #[arg(long, default_value = "fixtures/policy/query-policy.json")]
        policy: PathBuf,
        #[arg(long, default_value = "sqlite_demo")]
        source_id: String,
    },
    Trace {
        run_id: Uuid,
        #[arg(long, default_value = ".ysda/traces")]
        trace_dir: PathBuf,
    },
}

pub async fn dispatch(cli: Cli) -> AppResult<()> {
    match cli.command {
        AgentCommand::Schema {
            database,
            policy,
            source_id,
        } => {
            let policy = load_policy(&policy)?;
            let schema = QueryAgent::inspect(&database, SourceId::new(source_id), policy).await?;
            println!("{}", render_schema(&schema));
            Ok(())
        }

        AgentCommand::Ask {
            database,
            question,
            trace_dir,
            policy,
            source_id,
        } => {
            let config = LlmConfig::from_env()?;
            let policy = load_policy(&policy)?;
            let agent = QueryAgent::new(
                LlmClient::new(config),
                TraceRecorder::new(trace_dir),
                DEFAULT_MAX_ROWS,
                SourceId::new(source_id),
                policy,
            );
            let run = agent.run(&database, UserQuestion::new(question)).await?;
            println!("{}", render_run(&run));
            if let Some(error) = &run.error {
                return Err(AppError::AgentRunFailed {
                    category: error.category.clone(),
                    message: error.message.clone(),
                });
            }
            Ok(())
        }

        AgentCommand::Trace { run_id, trace_dir } => {
            let recorder = TraceRecorder::new(trace_dir);
            let run = recorder.load(run_id)?;
            println!("{}", render_run(&run));
            Ok(())
        }
    }
}

fn load_policy(path: &std::path::Path) -> AppResult<ResultPolicy> {
    let bytes = fs::read(path).map_err(|error| {
        AppError::Configuration(format!(
            "cannot read query policy {}: {error}",
            path.display()
        ))
    })?;
    ResultPolicy::from_json_bytes(&bytes)
        .map_err(|error| AppError::Configuration(error.to_string()))
}
