use std::path::Path;
use std::time::Instant;

use crate::domain::{AgentRun, RunErrorRecord, RunEvent, UserQuestion};
use crate::error::{AppError, AppResult};
use crate::executor::SqliteExecutor;
use crate::llm::LlmClient;
use crate::policy::SqlPolicy;
use crate::schema::SqliteCatalog;
use crate::trace::TraceRecorder;

pub struct QueryAgent {
    llm: LlmClient,
    traces: TraceRecorder,
    max_rows: usize,
}

impl QueryAgent {
    pub fn new(llm: LlmClient, traces: TraceRecorder, max_rows: usize) -> Self {
        Self {
            llm,
            traces,
            max_rows,
        }
    }

    pub async fn run(&self, database: &Path, question: UserQuestion) -> AppResult<AgentRun> {
        let started = Instant::now();
        let mut run = AgentRun::new(question);

        let schema = match SqliteCatalog::inspect(database) {
            Ok(schema) => schema,
            Err(error) => return self.finish_failure(run, "schema", started, error),
        };
        run.events.push(event(
            "schema",
            started,
            format!("inspected {} table(s)", schema.tables.len()),
        ));
        run.schema = Some(schema.clone());

        let generated = match self.llm.generate(&run.question, &schema).await {
            Ok(generated) => generated,
            Err(error) => return self.finish_failure(run, "llm", started, error),
        };
        run.events
            .push(event("llm", started, "received structed query".to_owned()));

        run.generated_query = Some(generated.clone());

        let decision = match SqlPolicy::evaluate(&generated.sql) {
            Ok(decision) => decision,
            Err(error) => return self.finish_failure(run, "policy", started, error),
        };
        run.policy_decision = Some(decision.clone());
        if !decision.allowed {
            let error = AppError::UnsafeSql(decision.reasons.join(";"));
            return self.finish_failure(run, "policy", started, error);
        }
        run.events
            .push(event("policy", started, "read-only SQL allowed".to_owned()));
        let result = match SqliteExecutor::execute(database, &generated.sql, self.max_rows) {
            Ok(result) => result,
            Err(error) => return self.finish_failure(run, "execute", started, error),
        };

        run.events.push(event(
            "execute",
            started,
            format!("returned {} row(s)", result.row_count),
        ));
        run.result = Some(result);
        self.traces.save(&run)?;
        Ok(run)
    }

    fn finish_failure(
        &self,
        mut run: AgentRun,
        stage: &str,
        started: Instant,
        error: AppError,
    ) -> AppResult<AgentRun> {
        run.events
            .push(event(stage, started, "stage failed".to_owned()));
        run.error = Some(RunErrorRecord {
            category: error.category().to_owned(),
            message: error.to_string(),
        });
        self.traces.save(&run)?;
        Ok(run)
    }
}

fn event(stage: &str, started: Instant, message: String) -> RunEvent {
    RunEvent {
        stage: stage.to_owned(),
        elapsed_ms: started.elapsed().as_millis(),
        message,
    }
}
