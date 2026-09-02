use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;
use ys_agent_core::{
    AgentAction, CoreError, CoreResult, CredentialGeneration, CredentialViewStatus,
    ModelCapabilities, ModelProvider, ModelRequest, ModelResponse, ModelRole,
    OAuthConnectionStatus, ProfileId, ProfileName, ProfileRevision, ProviderId, ProviderModelId,
    ProviderParameters, ToolCall, ToolCallId, ValidationVersions,
};
use ys_agent_runtime::provider::{
    catalog::GovernedProviderCatalog,
    validation::{
        COMPATIBILITY_PROBE_SCHEMA_VERSION, CompatibilityProbeRequest, CompatibilityValidator,
        LocalProfileValidation, LocalProfileValidationRequest, LocalProfileValidator,
        ModelContextLimit,
    },
};

const PROVIDER_CALL_ID: &str = "compatibility-provider-call-id";

struct RecordingProvider {
    capabilities: ModelCapabilities,
    responses: Mutex<Vec<CoreResult<ModelResponse>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingProvider {
    fn new(capabilities: ModelCapabilities, responses: Vec<CoreResult<ModelResponse>>) -> Self {
        Self {
            capabilities,
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    async fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().await.clone()
    }
}

#[async_trait]
impl ModelProvider for RecordingProvider {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
        self.requests.lock().await.push(request);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            return Err(CoreError::validation(
                "provider.protocol.invalid_response",
                "unexpected additional compatibility probe call",
            ));
        }
        responses.remove(0)
    }
}

struct ProbeFixture {
    catalog: GovernedProviderCatalog,
    revision: ProfileRevision,
    local: LocalProfileValidation,
    versions: ValidationVersions,
}

fn fixture(provider: ProviderId) -> ProbeFixture {
    let catalog = GovernedProviderCatalog::default();
    let profile_id = ProfileId::new();
    let model = ProviderModelId::new(provider, format!("{}probe-model", provider.model_prefix()))
        .expect("governed model");
    let generation = CredentialGeneration::new(profile_id, 1, provider.required_credential_kind())
        .expect("matching generation");
    let revision = ProfileRevision::draft(
        profile_id,
        1,
        provider,
        model,
        ProviderParameters::default(),
        Some(generation),
    )
    .expect("valid Draft");
    let name = ProfileName::new("compatibility probe").expect("valid name");
    let local =
        LocalProfileValidator::new(catalog.clone()).validate_local(LocalProfileValidationRequest {
            profile_id,
            name: &name,
            provider,
            model: revision.model(),
            parameters: revision.parameters(),
            credential_status: CredentialViewStatus::Saved,
            credential_generation: Some(generation),
            existing_names: &[],
        });
    assert!(local.is_valid(), "fixture must pass local validation");
    let versions = ValidationVersions::new(
        catalog.digest(),
        COMPATIBILITY_PROBE_SCHEMA_VERSION,
        "1.19.1",
        "codec-v1",
    );
    ProbeFixture {
        catalog,
        revision,
        local,
        versions,
    }
}

fn capabilities() -> ModelCapabilities {
    ModelCapabilities {
        tool_calling: true,
        structured_outputs: true,
        max_context_tokens: 0,
        parallel_tool_calls: false,
        streaming: false,
    }
}

fn tool_call_response(id: &str) -> ModelResponse {
    ModelResponse {
        action: AgentAction::CallTool {
            call: ToolCall {
                id: ToolCallId::new(),
                provider_call_id: Some(id.to_owned()),
                name: "ysda_compatibility_probe".to_owned(),
                arguments: json!({}),
                version: "v1".to_owned(),
            },
        },
        raw_content: None,
        usage: None,
    }
}

fn continuation_response() -> ModelResponse {
    ModelResponse {
        action: AgentAction::Respond {
            message: "probe complete".to_owned(),
        },
        raw_content: None,
        usage: None,
    }
}

#[tokio::test]
async fn model_probe_uses_only_fixed_synthetic_tool_round_trip_and_model_context_evidence() {
    let fixture = fixture(ProviderId::DeepSeek);
    let client = Arc::new(RecordingProvider::new(
        capabilities(),
        vec![
            Ok(tool_call_response(PROVIDER_CALL_ID)),
            Ok(continuation_response()),
        ],
    ));
    let validator = CompatibilityValidator::new(fixture.catalog.clone());

    let evidence = validator
        .probe_model(
            CompatibilityProbeRequest {
                revision: &fixture.revision,
                local_validation: &fixture.local,
                oauth_status: None,
                observed_context_limit: Some(ModelContextLimit::from_directory(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect("safe two-round probe passes");

    assert_eq!(evidence.context_limit(), 64);
    assert!(
        evidence
            .compatibility()
            .matches(&fixture.revision.validation_inputs(fixture.versions.clone()))
    );
    let requests = client.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].model, fixture.revision.model().as_str());
    assert_eq!(requests[0].temperature, None);
    assert!(requests[0].context_manifest.included.is_empty());
    assert!(requests[0].context_manifest.summaries.is_empty());
    assert_eq!(
        requests[0].messages[0].content,
        "Perform the fixed compatibility probe tool call only."
    );
    assert_eq!(
        requests[0].messages[1].content,
        "Call ysda_compatibility_probe with an empty object."
    );
    assert_eq!(requests[0].tools.len(), 1);
    assert_eq!(requests[0].tools[0].name, "ysda_compatibility_probe");
    assert_eq!(requests[1].messages.len(), 3);
    assert_eq!(requests[1].messages[1].role, ModelRole::Assistant);
    assert_eq!(
        requests[1].messages[1]
            .assistant_tool_call
            .as_ref()
            .expect("assistant call")
            .provider_call_id,
        PROVIDER_CALL_ID
    );
    assert_eq!(requests[1].messages[2].role, ModelRole::Tool);
    assert_eq!(
        requests[1].messages[2].tool_call_id.as_deref(),
        Some(PROVIDER_CALL_ID)
    );
    assert_eq!(
        requests[1].messages[2].name.as_deref(),
        Some("ysda_compatibility_probe")
    );
    assert_eq!(requests[1].messages[2].content, r#"{"status":"ok"}"#);
    assert!(
        !evidence
            .compatibility()
            .matches(&fixture.revision.validation_inputs(ValidationVersions::new(
                fixture.catalog.digest(),
                COMPATIBILITY_PROBE_SCHEMA_VERSION,
                "1.19.1",
                "codec-v2",
            ),))
    );
}

#[tokio::test]
async fn unknown_context_missing_capability_or_chatgpt_oauth_blocks_before_transport() {
    let deepseek = fixture(ProviderId::DeepSeek);
    let client = Arc::new(RecordingProvider::new(capabilities(), Vec::new()));
    let error = CompatibilityValidator::new(deepseek.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &deepseek.revision,
                local_validation: &deepseek.local,
                oauth_status: None,
                observed_context_limit: None,
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("unknown model context is not compatible");
    assert_eq!(error.code(), "provider.model.incompatible");
    assert!(client.requests().await.is_empty());

    let invalid_local = LocalProfileValidator::new(deepseek.catalog.clone()).validate_local(
        LocalProfileValidationRequest {
            profile_id: deepseek.revision.profile_id(),
            name: &ProfileName::new("missing credential").expect("valid name"),
            provider: deepseek.revision.provider(),
            model: deepseek.revision.model(),
            parameters: deepseek.revision.parameters(),
            credential_status: CredentialViewStatus::Missing,
            credential_generation: None,
            existing_names: &[],
        },
    );
    let client = Arc::new(RecordingProvider::new(capabilities(), Vec::new()));
    let error = CompatibilityValidator::new(deepseek.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &deepseek.revision,
                local_validation: &invalid_local,
                oauth_status: None,
                observed_context_limit: Some(ModelContextLimit::from_probe_response(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("local violations prevent network probing");
    assert_eq!(error.code(), "provider.credential.missing");
    assert!(client.requests().await.is_empty());

    let no_tools = ModelCapabilities {
        tool_calling: false,
        ..capabilities()
    };
    let client = Arc::new(RecordingProvider::new(no_tools, Vec::new()));
    let error = CompatibilityValidator::new(deepseek.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &deepseek.revision,
                local_validation: &deepseek.local,
                oauth_status: None,
                observed_context_limit: Some(ModelContextLimit::from_approved_evidence(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("tool support is required");
    assert_eq!(error.code(), "provider.model.incompatible");
    assert!(client.requests().await.is_empty());

    let chatgpt = fixture(ProviderId::ChatGptSubscription);
    let client = Arc::new(RecordingProvider::new(capabilities(), Vec::new()));
    let error = CompatibilityValidator::new(chatgpt.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &chatgpt.revision,
                local_validation: &chatgpt.local,
                oauth_status: Some(OAuthConnectionStatus::Expired),
                observed_context_limit: Some(ModelContextLimit::from_directory(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("a disconnected ChatGPT subscription cannot probe");
    assert_eq!(error.code(), "provider.oauth.not_connected");
    assert!(client.requests().await.is_empty());
}

#[tokio::test]
async fn protocol_id_and_continuation_errors_are_rejected_without_a_business_call() {
    let fixture = fixture(ProviderId::DeepSeek);
    let client = Arc::new(RecordingProvider::new(
        capabilities(),
        vec![Ok(tool_call_response(""))],
    ));
    let error = CompatibilityValidator::new(fixture.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &fixture.revision,
                local_validation: &fixture.local,
                oauth_status: None,
                observed_context_limit: Some(ModelContextLimit::from_directory(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("empty provider tool ID must fail closed");
    assert_eq!(error.code(), "provider.protocol.invalid_tool_call_id");
    assert_eq!(client.requests().await.len(), 1);

    let invalid_continuation = ModelResponse {
        action: AgentAction::StartQuery,
        raw_content: Some("customer query must never be preserved".to_owned()),
        usage: None,
    };
    let client = Arc::new(RecordingProvider::new(
        capabilities(),
        vec![
            Ok(tool_call_response(PROVIDER_CALL_ID)),
            Ok(invalid_continuation),
        ],
    ));
    let error = CompatibilityValidator::new(fixture.catalog.clone())
        .probe_model(
            CompatibilityProbeRequest {
                revision: &fixture.revision,
                local_validation: &fixture.local,
                oauth_status: None,
                observed_context_limit: Some(ModelContextLimit::from_directory(64)),
                codec_version: "codec-v1",
            },
            client.as_ref(),
        )
        .await
        .expect_err("anything other than a continuation response is invalid");
    assert_eq!(error.code(), "provider.protocol.invalid_response");
    assert_eq!(client.requests().await.len(), 2);
}

#[tokio::test]
async fn upstream_failures_are_normalized_without_retaining_raw_error_text() {
    let fixture = fixture(ProviderId::DeepSeek);
    let canary = "provider response secret canary";
    let cases = [
        ("provider.auth.invalid", "provider.auth.invalid"),
        ("provider.model.not_found", "provider.model.not_found"),
        ("provider.model.incompatible", "provider.model.incompatible"),
        ("provider.rate_limited", "provider.rate_limited"),
        ("provider.timeout", "provider.timeout"),
        ("provider.network", "provider.network"),
        ("provider.server", "provider.server"),
        (
            "provider.protocol.invalid_response",
            "provider.protocol.invalid_response",
        ),
        (
            "provider.operation.cancelled",
            "provider.operation.cancelled",
        ),
        ("provider.internal", "provider.internal"),
        (
            "unrecognized-provider-failure",
            "provider.protocol.invalid_response",
        ),
    ];

    for (source_code, expected_code) in cases {
        let client = Arc::new(RecordingProvider::new(
            capabilities(),
            vec![Err(CoreError::validation(source_code, canary))],
        ));
        let error = CompatibilityValidator::new(fixture.catalog.clone())
            .probe_model(
                CompatibilityProbeRequest {
                    revision: &fixture.revision,
                    local_validation: &fixture.local,
                    oauth_status: None,
                    observed_context_limit: Some(ModelContextLimit::from_directory(64)),
                    codec_version: "codec-v1",
                },
                client.as_ref(),
            )
            .await
            .expect_err("transport failures must surface as closed provider errors");
        assert_eq!(error.code(), expected_code);
        assert!(!error.to_string().contains(canary));
        assert!(!format!("{error:?}").contains(canary));
        assert_eq!(client.requests().await.len(), 1);
    }
}
