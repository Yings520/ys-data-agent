use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::timeout;
use ys_agent_core::{RunId, ToolCallId};

const DEFAULT_SINK_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryEvent {
    RunLatency {
        run_id: RunId,
        milliseconds: u64,
    },
    ModelUsage {
        run_id: RunId,
        model_call_id: String,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        milliseconds: u64,
    },
    ToolLatency {
        run_id: RunId,
        tool_call_id: ToolCallId,
        tool_name: String,
        milliseconds: u64,
        outcome: String,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TelemetryError {
    #[error("telemetry sink unavailable")]
    Unavailable,
    #[error("telemetry encoding failed: {0}")]
    Encoding(String),
    #[error("telemetry sink timed out")]
    Timeout,
}

#[async_trait]
pub trait TelemetrySink: Send + Sync {
    async fn emit(&self, event: TelemetryEvent) -> Result<(), TelemetryError>;
}

#[derive(Debug, Default)]
pub struct NoopTelemetrySink;

#[async_trait]
impl TelemetrySink for NoopTelemetrySink {
    async fn emit(&self, _event: TelemetryEvent) -> Result<(), TelemetryError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct TracingTelemetrySink;

#[async_trait]
impl TelemetrySink for TracingTelemetrySink {
    async fn emit(&self, event: TelemetryEvent) -> Result<(), TelemetryError> {
        match event {
            TelemetryEvent::RunLatency {
                run_id,
                milliseconds,
            } => tracing::info!(
                telemetry_type = "run_latency",
                run_id = %run_id,
                milliseconds,
            ),
            TelemetryEvent::ModelUsage {
                run_id,
                model_call_id,
                prompt_tokens,
                completion_tokens,
                milliseconds,
            } => tracing::info!(
                telemetry_type = "model_usage",
                run_id = %run_id,
                model_call_id,
                prompt_tokens = ?prompt_tokens,
                completion_tokens = ?completion_tokens,
                milliseconds,
            ),
            TelemetryEvent::ToolLatency {
                run_id,
                tool_call_id,
                tool_name,
                milliseconds,
                outcome,
            } => tracing::info!(
                telemetry_type = "tool_latency",
                run_id = %run_id,
                tool_call_id = %tool_call_id,
                tool_name,
                milliseconds,
                outcome,
            ),
        }
        Ok(())
    }
}

pub struct TelemetryDispatcher {
    sink: Arc<dyn TelemetrySink>,
    timeout: Duration,
    failures: AtomicU64,
}

impl TelemetryDispatcher {
    pub fn new(sink: Arc<dyn TelemetrySink>) -> Self {
        Self::with_timeout(sink, DEFAULT_SINK_TIMEOUT)
    }

    pub fn with_timeout(sink: Arc<dyn TelemetrySink>, timeout: Duration) -> Self {
        Self {
            sink,
            timeout,
            failures: AtomicU64::new(0),
        }
    }

    pub async fn emit_after_commit(&self, event: TelemetryEvent) {
        let outcome = timeout(self.timeout, self.sink.emit(event)).await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    telemetry_error = %error,
                    "telemetry emission failed after runtime commit"
                );
            }
            Err(_) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    telemetry_error = %TelemetryError::Timeout,
                    "telemetry emission timed out after runtime commit"
                );
            }
        }
    }

    pub fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}

impl Default for TelemetryDispatcher {
    fn default() -> Self {
        Self::new(Arc::new(NoopTelemetrySink))
    }
}
