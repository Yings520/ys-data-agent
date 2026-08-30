use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::time::Instant;
use ys_agent_core::{CoreResult, RunId, RunSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopBudget {
    pub max_steps: u32,
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_total_tokens: u32,
    pub deadline: Duration,
}

impl Default for LoopBudget {
    fn default() -> Self {
        Self {
            max_steps: 24,
            max_model_calls: 12,
            max_tool_calls: 16,
            max_total_tokens: 64_000,
            deadline: Duration::from_secs(10 * 60),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepAccounting {
    pub model_calls: u32,
    pub tool_calls: u32,
    pub tokens: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopUsage {
    pub steps: u32,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone)]
pub enum StepOutcome {
    Continue {
        snapshot: RunSnapshot,
        accounting: StepAccounting,
    },
    Wait {
        snapshot: RunSnapshot,
        accounting: StepAccounting,
    },
    Terminal {
        snapshot: RunSnapshot,
        accounting: StepAccounting,
    },
}

impl StepOutcome {
    fn accounting(&self) -> StepAccounting {
        match self {
            Self::Continue { accounting, .. }
            | Self::Wait { accounting, .. }
            | Self::Terminal { accounting, .. } => *accounting,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub snapshot: RunSnapshot,
    pub usage: LoopUsage,
}

#[async_trait]
pub trait HarnessStep: Send + Sync {
    async fn step(&self, run_id: &RunId) -> CoreResult<StepOutcome>;

    async fn fail_terminal(
        &self,
        run_id: &RunId,
        code: &'static str,
        message: &'static str,
    ) -> CoreResult<RunSnapshot>;
}

pub struct LoopDriver {
    harness: Arc<dyn HarnessStep>,
    budget: LoopBudget,
}

impl LoopDriver {
    pub fn new(harness: Arc<dyn HarnessStep>, budget: LoopBudget) -> Self {
        Self { harness, budget }
    }

    pub fn with_defaults(harness: Arc<dyn HarnessStep>) -> Self {
        Self::new(harness, LoopBudget::default())
    }

    pub async fn run(&self, run_id: &RunId) -> CoreResult<LoopResult> {
        let deadline = Instant::now() + self.budget.deadline;
        let mut usage = LoopUsage::default();

        loop {
            if usage.steps >= self.budget.max_steps {
                return self
                    .fail(
                        run_id,
                        usage,
                        "loop_step_budget_exceeded",
                        "Loop step budget exceeded",
                    )
                    .await;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return self
                    .fail(
                        run_id,
                        usage,
                        "loop_deadline_exceeded",
                        "Loop deadline exceeded",
                    )
                    .await;
            }

            let outcome = match tokio::time::timeout(remaining, self.harness.step(run_id)).await {
                Ok(result) => result?,
                Err(_) => {
                    return self
                        .fail(
                            run_id,
                            usage,
                            "loop_deadline_exceeded",
                            "Loop deadline exceeded during a step",
                        )
                        .await;
                }
            };
            usage.steps += 1;
            let accounting = outcome.accounting();
            usage.model_calls = usage.model_calls.saturating_add(accounting.model_calls);
            usage.tool_calls = usage.tool_calls.saturating_add(accounting.tool_calls);
            usage.total_tokens = usage.total_tokens.saturating_add(accounting.tokens);

            if usage.model_calls > self.budget.max_model_calls {
                return self
                    .fail(
                        run_id,
                        usage,
                        "loop_model_call_budget_exceeded",
                        "Model call budget exceeded",
                    )
                    .await;
            }
            if usage.tool_calls > self.budget.max_tool_calls {
                return self
                    .fail(
                        run_id,
                        usage,
                        "loop_tool_call_budget_exceeded",
                        "Tool call budget exceeded",
                    )
                    .await;
            }
            if usage.total_tokens > self.budget.max_total_tokens {
                return self
                    .fail(
                        run_id,
                        usage,
                        "loop_token_budget_exceeded",
                        "Model token budget exceeded",
                    )
                    .await;
            }

            match outcome {
                StepOutcome::Continue { .. } => {}
                StepOutcome::Wait { snapshot, .. } | StepOutcome::Terminal { snapshot, .. } => {
                    return Ok(LoopResult { snapshot, usage });
                }
            }
        }
    }

    async fn fail(
        &self,
        run_id: &RunId,
        usage: LoopUsage,
        code: &'static str,
        message: &'static str,
    ) -> CoreResult<LoopResult> {
        let snapshot = self.harness.fail_terminal(run_id, code, message).await?;
        Ok(LoopResult { snapshot, usage })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Arc};

    use async_trait::async_trait;
    use tokio::sync::Mutex;
    use ys_agent_core::{CoreResult, RunId, RunSnapshot, RunStatus, TaskId, WorkflowKind};

    use super::{HarnessStep, LoopBudget, LoopDriver, StepAccounting, StepOutcome};

    struct ScriptedHarness {
        outcomes: Mutex<VecDeque<StepOutcome>>,
    }

    fn snapshot(status: RunStatus) -> RunSnapshot {
        RunSnapshot {
            run_id: RunId::new(),
            task_id: TaskId::new(),
            workflow: WorkflowKind::Query,
            status,
            attempt: 1,
            retry_of_run_id: None,
            version: 1,
            workflow_state: serde_json::json!({}),
            pending_wait_metadata: None,
            primary_artifact_id: None,
            last_completed_step_id: None,
        }
    }

    #[async_trait]
    impl HarnessStep for ScriptedHarness {
        async fn step(&self, _run_id: &RunId) -> CoreResult<StepOutcome> {
            Ok(self
                .outcomes
                .lock()
                .await
                .pop_front()
                .expect("scripted step"))
        }

        async fn fail_terminal(
            &self,
            _run_id: &RunId,
            _code: &'static str,
            _message: &'static str,
        ) -> CoreResult<RunSnapshot> {
            Ok(snapshot(RunStatus::Failed))
        }
    }

    #[tokio::test]
    async fn waiting_stops_without_consuming_another_step() {
        let waiting = snapshot(RunStatus::WaitingForInput);
        let harness = Arc::new(ScriptedHarness {
            outcomes: Mutex::new(VecDeque::from([StepOutcome::Wait {
                snapshot: waiting,
                accounting: StepAccounting {
                    model_calls: 1,
                    tool_calls: 0,
                    tokens: 10,
                },
            }])),
        });
        let run_id = RunId::new();
        let result = LoopDriver::with_defaults(harness)
            .run(&run_id)
            .await
            .unwrap();

        assert_eq!(result.snapshot.status, RunStatus::WaitingForInput);
        assert_eq!(result.usage.steps, 1);
        assert_eq!(result.usage.model_calls, 1);
    }

    #[tokio::test]
    async fn token_limit_produces_a_failed_snapshot() {
        let running = snapshot(RunStatus::Running);
        let harness = Arc::new(ScriptedHarness {
            outcomes: Mutex::new(VecDeque::from([StepOutcome::Continue {
                snapshot: running,
                accounting: StepAccounting {
                    model_calls: 1,
                    tool_calls: 0,
                    tokens: 11,
                },
            }])),
        });
        let budget = LoopBudget {
            max_total_tokens: 10,
            ..LoopBudget::default()
        };
        let result = LoopDriver::new(harness, budget)
            .run(&RunId::new())
            .await
            .unwrap();

        assert_eq!(result.snapshot.status, RunStatus::Failed);
    }
}
