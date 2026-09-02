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
use ys_agent_core::{ProviderFingerprint, ProviderId, RunId, ToolCallId};

const DEFAULT_SINK_TIMEOUT: Duration = Duration::from_millis(100);

/// The only textual values allowed to cross the telemetry boundary. Provider/OAuth/Vault errors
/// already use the core's closed `ProviderManagementError` surface; arbitrary transport text,
/// request bodies, tool arguments, and model output must never become telemetry labels.
pub struct SecretSanitizer;

impl SecretSanitizer {
    pub const REDACTED: &str = "[REDACTED]";

    pub fn model_call_id(value: &str) -> String {
        if value
            .strip_prefix("model-")
            .is_some_and(Self::is_safe_identifier)
        {
            value.to_owned()
        } else {
            Self::REDACTED.to_owned()
        }
    }

    pub fn tool_name(value: &str) -> String {
        match value {
            "inspect_schema" | "query_data" | "read_freshness" | "resolve_metric" => {
                value.to_owned()
            }
            _ => Self::REDACTED.to_owned(),
        }
    }

    pub fn tool_outcome(value: &str) -> String {
        match value {
            "succeeded" | "rejected" | "failed" | "indeterminate" => value.to_owned(),
            _ => Self::REDACTED.to_owned(),
        }
    }

    pub fn fingerprint_sha256(fingerprint: &ProviderFingerprint) -> String {
        format!("sha256:{}", fingerprint.digest())
    }

    fn sanitized_fingerprint_hash(value: &str) -> String {
        let Some(digest) = value.strip_prefix("sha256:") else {
            return Self::REDACTED.to_owned();
        };
        if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            value.to_owned()
        } else {
            Self::REDACTED.to_owned()
        }
    }

    fn is_safe_identifier(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    }
}

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
    /// Provider telemetry contains the governed enum plus a hash of the non-sensitive binding;
    /// it deliberately has no model name, credential locator, request, response, or arguments.
    ProviderCall {
        provider: ProviderId,
        fingerprint_sha256: String,
        milliseconds: u64,
        retry_count: u32,
        outcome: ProviderTelemetryOutcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTelemetryOutcome {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

impl TelemetryEvent {
    pub fn provider_call(
        provider: ProviderId,
        fingerprint: &ProviderFingerprint,
        milliseconds: u64,
        retry_count: u32,
        outcome: ProviderTelemetryOutcome,
    ) -> Self {
        Self::ProviderCall {
            provider,
            fingerprint_sha256: SecretSanitizer::fingerprint_sha256(fingerprint),
            milliseconds,
            retry_count,
            outcome,
        }
    }

    pub fn sanitized(self) -> Self {
        match self {
            Self::ModelUsage {
                run_id,
                model_call_id,
                prompt_tokens,
                completion_tokens,
                milliseconds,
            } => Self::ModelUsage {
                run_id,
                model_call_id: SecretSanitizer::model_call_id(&model_call_id),
                prompt_tokens,
                completion_tokens,
                milliseconds,
            },
            Self::ToolLatency {
                run_id,
                tool_call_id,
                tool_name,
                milliseconds,
                outcome,
            } => Self::ToolLatency {
                run_id,
                tool_call_id,
                tool_name: SecretSanitizer::tool_name(&tool_name),
                milliseconds,
                outcome: SecretSanitizer::tool_outcome(&outcome),
            },
            Self::ProviderCall {
                provider,
                fingerprint_sha256,
                milliseconds,
                retry_count,
                outcome,
            } => Self::ProviderCall {
                provider,
                fingerprint_sha256: SecretSanitizer::sanitized_fingerprint_hash(
                    &fingerprint_sha256,
                ),
                milliseconds,
                retry_count,
                outcome,
            },
            event @ Self::RunLatency { .. } => event,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TelemetryError {
    #[error("telemetry sink unavailable")]
    Unavailable,
    #[error("telemetry encoding failed")]
    Encoding,
    #[error("telemetry sink timed out")]
    Timeout,
}

impl TelemetryError {
    const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "telemetry_unavailable",
            Self::Encoding => "telemetry_encoding_failed",
            Self::Timeout => "telemetry_timeout",
        }
    }
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
        match event.sanitized() {
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
            TelemetryEvent::ProviderCall {
                provider,
                fingerprint_sha256,
                milliseconds,
                retry_count,
                outcome,
            } => tracing::info!(
                telemetry_type = "provider_call",
                provider = ?provider,
                fingerprint_sha256,
                milliseconds,
                retry_count,
                outcome = ?outcome,
                "provider call completed"
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
        let outcome = timeout(self.timeout, self.sink.emit(event.sanitized())).await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    telemetry_error = error.code(),
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
