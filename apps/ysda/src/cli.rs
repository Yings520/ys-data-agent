use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

use crate::agent::QueryAgent;
use crate::domain::UserQuestion;
use crate::error::{AppError, AppResult};
use crate::llm::{LlmClient, LlmConfig};
use crate::output::{render_run, render_schema};
use crate::schema::SqliteCatalog;
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
    },
    Ask {
        #[arg(long)]
        database: PathBuf,
        question: String,
        #[arg(long, default_value = ".ysda/traces")]
        trace_dir: PathBuf,
    },
    Trace {
        run_id: Uuid,
        #[arg(long, default_value = ".ysda/traces")]
        trace_dir: PathBuf,
    },
}

pub async fn dispatch(cli: Cli) -> AppResult<()> {
    match cli.command {
        AgentCommand::Schema { database } => {
            let schema = SqliteCatalog::inspect(&database)?;
            println!("{}", render_schema(&schema));
            Ok(())
        }

        AgentCommand::Ask {
            database,
            question,
            trace_dir,
        } => {
            let config = LlmConfig::from_env()?;
            let agent = QueryAgent::new(
                LlmClient::new(config),
                TraceRecorder::new(trace_dir),
                DEFAULT_MAX_ROWS,
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
