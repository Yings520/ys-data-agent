use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use ys_agent_core::{ProviderId, RunId, ToolCallId};
use ys_agent_runtime::telemetry::{
    ProviderTelemetryOutcome, SecretSanitizer, TelemetryDispatcher, TelemetryError, TelemetryEvent,
    TelemetrySink,
};

#[derive(Debug, Default)]
struct AlwaysFailTelemetrySink;

#[async_trait]
impl TelemetrySink for AlwaysFailTelemetrySink {
    async fn emit(&self, _event: TelemetryEvent) -> Result<(), TelemetryError> {
        Err(TelemetryError::Unavailable)
    }
}

#[derive(Debug, Clone, Default)]
struct RecordingTelemetrySink {
    serialized: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl TelemetrySink for RecordingTelemetrySink {
    async fn emit(&self, event: TelemetryEvent) -> Result<(), TelemetryError> {
        let serialized = serde_json::to_string(&event).map_err(|_| TelemetryError::Encoding)?;
        self.serialized
            .lock()
            .expect("recording telemetry mutex")
            .push(serialized);
        Ok(())
    }
}

impl RecordingTelemetrySink {
    fn all_text(&self) -> String {
        self.serialized
            .lock()
            .expect("recording telemetry mutex")
            .join("\n")
    }
}

#[tokio::test]
async fn telemetry_failure_never_rolls_back_a_persisted_event() {
    let persisted = AtomicBool::new(false);
    let dispatcher = TelemetryDispatcher::new(Arc::new(AlwaysFailTelemetrySink));

    persisted.store(true, Ordering::SeqCst);
    dispatcher
        .emit_after_commit(TelemetryEvent::RunLatency {
            run_id: RunId::new(),
            milliseconds: 12,
        })
        .await;

    assert!(persisted.load(Ordering::SeqCst));
    assert_eq!(dispatcher.failure_count(), 1);
}

#[tokio::test]
async fn telemetry_does_not_receive_query_result_rows() {
    let sink = RecordingTelemetrySink::default();
    let dispatcher = TelemetryDispatcher::new(Arc::new(sink.clone()));

    dispatcher
        .emit_after_commit(TelemetryEvent::ToolLatency {
            run_id: RunId::new(),
            tool_call_id: ToolCallId::new(),
            tool_name: "query_data".to_owned(),
            milliseconds: 4,
            outcome: "succeeded".to_owned(),
        })
        .await;

    assert!(!sink.all_text().contains("secret_customer_name"));
    assert!(!sink.all_text().contains("rows"));
}

#[tokio::test]
async fn secret_canaries_never_reach_telemetry() {
    let sink = RecordingTelemetrySink::default();
    let dispatcher = TelemetryDispatcher::new(Arc::new(sink.clone()));
    const CANARY: &str = "provider-telemetry-canary-must-not-leak";

    dispatcher
        .emit_after_commit(TelemetryEvent::ModelUsage {
            run_id: RunId::new(),
            model_call_id: CANARY.to_owned(),
            prompt_tokens: Some(100),
            completion_tokens: Some(20),
            milliseconds: 9,
        })
        .await;
    dispatcher
        .emit_after_commit(TelemetryEvent::ToolLatency {
            run_id: RunId::new(),
            tool_call_id: ToolCallId::new(),
            tool_name: CANARY.to_owned(),
            milliseconds: 3,
            outcome: CANARY.to_owned(),
        })
        .await;

    assert!(!sink.all_text().contains(CANARY));
    assert!(sink.all_text().contains("[REDACTED]"));
}

#[tokio::test]
async fn provider_telemetry_allows_only_a_fingerprint_hash() {
    let sink = RecordingTelemetrySink::default();
    let dispatcher = TelemetryDispatcher::new(Arc::new(sink.clone()));
    const CANARY: &str = "provider-request-body-canary-must-not-leak";

    dispatcher
        .emit_after_commit(TelemetryEvent::ProviderCall {
            provider: ProviderId::DeepSeek,
            fingerprint_sha256: CANARY.to_owned(),
            milliseconds: 17,
            retry_count: 1,
            outcome: ProviderTelemetryOutcome::Failed,
        })
        .await;
    let fingerprint_hash = format!("sha256:{}", "a".repeat(64));
    dispatcher
        .emit_after_commit(TelemetryEvent::ProviderCall {
            provider: ProviderId::DeepSeek,
            fingerprint_sha256: fingerprint_hash.clone(),
            milliseconds: 12,
            retry_count: 0,
            outcome: ProviderTelemetryOutcome::Succeeded,
        })
        .await;

    let serialized = sink.all_text();
    assert!(!serialized.contains(CANARY));
    assert!(serialized.contains(SecretSanitizer::REDACTED));
    assert!(serialized.contains(&fingerprint_hash));
    assert!(serialized.contains("deep_seek"));
    assert!(!serialized.contains("model"));
    assert!(!serialized.contains("credential"));
}
