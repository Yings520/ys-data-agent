use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::sync::Mutex;
use ys_agent_adapters::credential::keyring::InMemoryCredentialVault;
use ys_agent_core::{
    AgentAction, ArtifactAccessContext, ArtifactAccessPurpose, ArtifactKind, ArtifactMetadata,
    ArtifactStore, CellValue, CommandId, CommandReceipt, CommandResultKind, CoreError, CoreResult,
    CredentialVault, EventActor, ModelResponse, PendingRunEvent, Principal,
    ProtectedCredentialWrite, ProviderCredentialReference, ProviderResult, PutArtifact,
    QueryResult, RunEventKind, RunId, RunProviderBinding, RunProviderBindingRepository,
    RunProviderBindingSource, RunStatus, RuntimeCommandBatch, RuntimeStore, SecretValue,
    Sensitivity, StepId, TaskId, TaskStatus, WorkspaceId,
};
use ys_agent_runtime::{
    ActiveRunProviderBindingSource, AgentServiceApi, CoordinationDecision, Coordinator,
    CreateTaskRequest, DatasourceDisplayState, InProcessAgentService, QueryDisplayState,
    QueryNonSuccessReason, QueryResultPreviewView, RuleBasedCoordinator, RunScheduler,
    SendMessageRequest, ServiceReply, StaticRunProviderBindingSource, TuiDisplayContext,
    TuiDisplayContextInput, TuiDisplayContextSource,
};
use ys_agent_store::{LocalArtifactStore, SqliteRuntimeStore};

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

#[derive(Default)]
struct CountingScheduler {
    scheduled: Mutex<Vec<RunId>>,
}

impl CountingScheduler {
    async fn count(&self) -> usize {
        self.scheduled.lock().await.len()
    }
}

#[async_trait]
impl RunScheduler for CountingScheduler {
    async fn schedule(&self, run_id: RunId) -> CoreResult<()> {
        let mut scheduled = self.scheduled.lock().await;
        if !scheduled.contains(&run_id) {
            scheduled.push(run_id);
        }
        Ok(())
    }
}

async fn seed_active_vault(
    active: ys_agent_core::ActiveProviderSnapshot,
) -> Arc<InMemoryCredentialVault> {
    let vault = Arc::new(InMemoryCredentialVault::new());
    let generation = RunProviderBinding::from_active(RunId::new(), active)
        .expect("active snapshot creates a binding")
        .credential_generation();
    vault
        .write_generation(ProtectedCredentialWrite {
            reference: ProviderCredentialReference {
                profile_id: generation.profile_id(),
                generation,
            },
            secret: SecretValue::from_utf8("service-test-secret".to_owned()),
        })
        .await
        .expect("seed protected active Credential");
    vault
}

struct SwitchActiveAfterFirstBinding {
    source: ActiveRunProviderBindingSource,
    database: std::path::PathBuf,
    switched: AtomicBool,
}

struct FakeTuiDisplayContextSource {
    input: TuiDisplayContextInput,
    calls: AtomicUsize,
    _dsn_canary: String,
    _credential_canary: String,
    _internal_id_canary: String,
    _event_payload_canary: String,
    _business_row_canary: String,
}

#[async_trait]
impl TuiDisplayContextSource for FakeTuiDisplayContextSource {
    async fn load(&self) -> CoreResult<TuiDisplayContextInput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.input.clone())
    }
}

#[async_trait]
impl RunProviderBindingSource for SwitchActiveAfterFirstBinding {
    async fn bind_new_run(&self, run_id: RunId) -> ProviderResult<RunProviderBinding> {
        let binding = self.source.bind_new_run(run_id).await?;
        if !self.switched.swap(true, Ordering::SeqCst) {
            rusqlite::Connection::open(&self.database)
                .expect("open database to simulate a concurrent active activation")
                .execute(
                    "UPDATE active_provider
                     SET activation_revision = activation_revision + 1
                     WHERE singleton = 1",
                    [],
                )
                .expect("advance the active activation revision");
        }
        Ok(binding)
    }
}

struct ServiceFixture {
    _directory: TempDir,
    store: Arc<SqliteRuntimeStore>,
    artifacts: Arc<LocalArtifactStore>,
    service: Arc<InProcessAgentService>,
    coordinator: RuleBasedCoordinator,
    scheduler: Arc<CountingScheduler>,
    workspace_id: WorkspaceId,
    principal: Principal,
    session_id: ys_agent_core::SessionId,
}

impl ServiceFixture {
    async fn new() -> Self {
        Self::build(None, true).await
    }

    async fn with_conversation_model(model: Arc<dyn ys_agent_core::ModelProvider>) -> Self {
        Self::build(Some(model), true).await
    }

    async fn without_provider_binding() -> Self {
        Self::build(None, false).await
    }

    async fn build(model: Option<Arc<dyn ys_agent_core::ModelProvider>>, bind_runs: bool) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = Arc::new(
            SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let artifacts =
            Arc::new(LocalArtifactStore::new(directory.path()).expect("local artifact store"));
        let scheduler = Arc::new(CountingScheduler::default());
        let workspace_id = WorkspaceId::new();
        let mut service = InProcessAgentService::with_event_capacity(
            workspace_id,
            store.clone(),
            artifacts.clone(),
            scheduler.clone(),
            2,
        );
        if bind_runs {
            service = service.with_run_provider_binding_source(Arc::new(
                StaticRunProviderBindingSource::from_active(
                    provider_fixture::persisted_test_active_provider(store.as_ref()).await,
                ),
            ));
        } else {
            service = service.with_run_provider_binding_source(Arc::new(
                ActiveRunProviderBindingSource::new(
                    Arc::new(store.provider_repository()),
                    Arc::new(store.run_binding_repository()),
                    Arc::new(InMemoryCredentialVault::new()),
                ),
            ));
        }
        if let Some(model) = model {
            service = service.with_conversation_model(model, "test-model");
        }
        let service = Arc::new(service);
        let principal = Principal::local_operator("Data Engineer");
        let session = service
            .create_session(CommandId::new(), principal.clone())
            .await
            .expect("default session");
        Self {
            _directory: directory,
            store,
            artifacts,
            service,
            coordinator: RuleBasedCoordinator,
            scheduler,
            workspace_id,
            principal,
            session_id: session.id,
        }
    }

    fn principal(&self) -> Principal {
        self.principal.clone()
    }

    fn session_id(&self) -> ys_agent_core::SessionId {
        self.session_id
    }

    async fn created_run_count(&self) -> u64 {
        self.store.run_count().await.expect("count runs")
    }

    async fn persist_internal_artifact(
        &self,
        kind: ArtifactKind,
        bytes: Vec<u8>,
    ) -> ArtifactMetadata {
        let task_id = TaskId::new();
        let run_id = RunId::new();
        let metadata = self
            .artifacts
            .put(PutArtifact {
                workspace_id: self.workspace_id,
                task_id,
                run_id,
                kind,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: Some(self.principal.id),
                retention_policy: None,
                expires_at: None,
                producer_step_id: None,
            })
            .await
            .expect("persist Artifact bytes");
        let command_id = CommandId::new();
        self.store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: format!("seed-artifact-{}", metadata.id),
                receipt: CommandReceipt {
                    command_id,
                    command_fingerprint: format!("seed-artifact-{}", metadata.id),
                    result_kind: CommandResultKind::NoopReplay,
                    session_id: None,
                    task_id: Some(task_id),
                    run_id: Some(run_id),
                    artifact_id: Some(metadata.id),
                    message: None,
                    capability: None,
                },
                new_session: None,
                new_task: None,
                create_run: None,
                new_artifact: Some(metadata.clone()),
                pending_events: Vec::new(),
                snapshot_update: None,
            })
            .await
            .expect("index Artifact metadata");
        metadata
    }
}

#[tokio::test]
async fn provider_management_is_available_only_through_the_service_boundary() {
    let fixture = ServiceFixture::new().await;

    let error = fixture
        .service
        .provider_catalog()
        .await
        .expect_err("an uncomposed service must not expose a repository or Vault fallback");

    assert_eq!(error.code(), "provider.internal");
}

#[tokio::test]
async fn tui_display_context_is_composed_from_a_safe_authoritative_snapshot() {
    let fixture = ServiceFixture::new().await;
    let source = Arc::new(FakeTuiDisplayContextSource {
        input: TuiDisplayContextInput::new(
            "analytics",
            DatasourceDisplayState::active("warehouse").expect("safe display name"),
            true,
            QueryDisplayState::WaitingForInput,
        )
        .expect("valid display context input"),
        calls: AtomicUsize::new(0),
        _dsn_canary: "postgres://admin:dsn-canary@production".to_owned(),
        _credential_canary: "credential-canary".to_owned(),
        _internal_id_canary: "internal-id-canary".to_owned(),
        _event_payload_canary: "event-payload-canary".to_owned(),
        _business_row_canary: "business-row-canary".to_owned(),
    });
    let service = InProcessAgentService::new(
        fixture.workspace_id,
        fixture.store.clone(),
        fixture.artifacts.clone(),
        fixture.scheduler.clone(),
    )
    .with_tui_display_context_source(source.clone());

    let view: TuiDisplayContext = service
        .tui_display_context()
        .await
        .expect("read safe display context");

    assert_eq!(view.workspace_display_name(), "analytics");
    assert_eq!(
        view.datasource(),
        &DatasourceDisplayState::active("warehouse").expect("safe display name")
    );
    assert!(view.read_only());
    assert_eq!(view.query_state(), QueryDisplayState::WaitingForInput);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);

    let rendered = serde_json::to_string(&view).expect("display context serializes");
    for forbidden in [
        "dsn-canary",
        "credential-canary",
        "internal-id-canary",
        "event-payload-canary",
        "business-row-canary",
        "workspace_id",
        "source_id",
        "phase",
        "acl",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "leaked display field: {forbidden}"
        );
    }
}

#[tokio::test]
async fn tui_display_context_fails_closed_without_an_authoritative_source() {
    let fixture = ServiceFixture::new().await;

    let error = fixture
        .service
        .tui_display_context()
        .await
        .expect_err("service must not infer display state from local configuration");

    assert_eq!(error.code(), "tui_display_context_unavailable");
}

#[test]
fn query_display_state_maps_runtime_terminal_states_without_false_success() {
    assert_eq!(
        QueryDisplayState::from(RunStatus::Queued),
        QueryDisplayState::Ready
    );
    assert_eq!(
        QueryDisplayState::from(RunStatus::Running),
        QueryDisplayState::Running
    );
    assert_eq!(
        QueryDisplayState::from(RunStatus::WaitingForInput),
        QueryDisplayState::WaitingForInput
    );
    assert_eq!(
        QueryDisplayState::from(RunStatus::Succeeded),
        QueryDisplayState::Completed
    );
    assert_eq!(
        QueryDisplayState::from(RunStatus::Failed),
        QueryDisplayState::NonSuccess {
            reason: QueryNonSuccessReason::Failed,
        }
    );
    assert_eq!(
        QueryDisplayState::from(RunStatus::Cancelled),
        QueryDisplayState::NonSuccess {
            reason: QueryNonSuccessReason::Cancelled,
        }
    );

    let states = [
        DatasourceDisplayState::NotConfigured,
        DatasourceDisplayState::Unavailable {
            reason: ys_agent_runtime::DatasourceUnavailableReason::StatusUnavailable,
        },
    ];
    let rendered = serde_json::to_string(&states).expect("stable datasource states serialize");
    assert!(rendered.contains("not_configured"));
    assert!(rendered.contains("status_unavailable"));
    assert!(
        TuiDisplayContextInput::new(
            "workspace\nspoofed",
            DatasourceDisplayState::NotConfigured,
            true,
            QueryDisplayState::NonSuccess {
                reason: QueryNonSuccessReason::Rejected,
            },
        )
        .is_err()
    );
}

#[tokio::test]
async fn production_run_creation_rejects_missing_provider_binding_before_persistence() {
    let fixture = ServiceFixture::without_provider_binding().await;

    let error = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect_err("production Run requires a Provider binding");

    assert!(matches!(
        error,
        CoreError::Validation {
            code: "provider.no_active_profile",
            ..
        }
    ));
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn production_run_creation_retries_with_the_committed_active_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = Arc::new(
        SqliteRuntimeStore::open(&database)
            .await
            .expect("runtime store"),
    );
    let active = provider_fixture::persisted_test_active_provider(store.as_ref()).await;
    let profiles = store.provider_repository();
    let bindings = store.run_binding_repository();
    let source = Arc::new(SwitchActiveAfterFirstBinding {
        source: ActiveRunProviderBindingSource::new(
            Arc::new(profiles.clone()),
            Arc::new(bindings.clone()),
            seed_active_vault(active).await,
        ),
        database,
        switched: AtomicBool::new(false),
    });
    let artifacts =
        Arc::new(LocalArtifactStore::new(directory.path()).expect("local artifact store"));
    let scheduler = Arc::new(CountingScheduler::default());
    let workspace_id = WorkspaceId::new();
    let service = InProcessAgentService::with_event_capacity(
        workspace_id,
        store.clone(),
        artifacts,
        scheduler.clone(),
        2,
    )
    .with_run_provider_binding_source(source);
    let session = service
        .create_session(CommandId::new(), Principal::local_operator("Data Engineer"))
        .await
        .expect("create session");

    let reply = service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            "Query GMV",
        ))
        .await
        .expect("retry Run creation after an active-snapshot race");
    let run_id = reply.run_id().expect("scheduled Query Run");
    let active = profiles
        .active()
        .await
        .expect("read final active snapshot")
        .expect("active Provider remains configured");
    let binding = bindings
        .load_run_binding(run_id)
        .await
        .expect("load immutable Run binding");

    let expected = RunProviderBinding::from_active(run_id, active)
        .expect("final active snapshot creates a Run binding");
    assert_eq!(binding, expected);
    assert_eq!(
        store
            .load_events(&run_id, 0)
            .await
            .expect("load atomically-created events")
            .len(),
        3
    );
    assert_eq!(scheduler.count().await, 1);
}

#[tokio::test]
async fn production_run_creation_rejects_an_unresolvable_active_generation_before_persistence() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("runtime.db");
    let store = Arc::new(
        SqliteRuntimeStore::open(&database)
            .await
            .expect("runtime store"),
    );
    provider_fixture::persisted_test_active_provider(store.as_ref()).await;
    let source = Arc::new(ActiveRunProviderBindingSource::new(
        Arc::new(store.provider_repository()),
        Arc::new(store.run_binding_repository()),
        Arc::new(InMemoryCredentialVault::new()),
    ));
    let artifacts =
        Arc::new(LocalArtifactStore::new(directory.path()).expect("local artifact store"));
    let scheduler = Arc::new(CountingScheduler::default());
    let service = InProcessAgentService::with_event_capacity(
        WorkspaceId::new(),
        store.clone(),
        artifacts,
        scheduler.clone(),
        2,
    )
    .with_run_provider_binding_source(source);
    let session = service
        .create_session(CommandId::new(), Principal::local_operator("Data Engineer"))
        .await
        .expect("create session");

    let error = service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            session.id,
            "Query GMV",
        ))
        .await
        .expect_err("unresolvable active generation must prevent Run creation");

    assert!(matches!(
        error,
        CoreError::Validation {
            code: "provider.credential.missing",
            ..
        }
    ));
    assert_eq!(store.run_count().await.expect("count Runs"), 0);
    assert_eq!(scheduler.count().await, 0);
}

#[tokio::test]
async fn tui_preview_reads_complete_internal_query_artifacts_only() {
    let fixture = ServiceFixture::new().await;
    let query_bytes = serde_json::to_vec(&serde_json::json!({
        "answer_summary": "2026年8月13日至15日的GMV查询已完成。",
        "verification_padding": "x".repeat(4_096),
    }))
    .expect("serialize oversized Query Artifact");
    assert!(query_bytes.len() > 4_096);
    let query = fixture
        .persist_internal_artifact(ArtifactKind::Query, query_bytes.clone())
        .await;

    let query_view = fixture
        .service
        .get_artifact(
            &query.id,
            ArtifactAccessContext {
                workspace_id: fixture.workspace_id,
                principal_id: fixture.principal.id,
                purpose: ArtifactAccessPurpose::TuiPreview,
                max_sensitivity: Sensitivity::Internal,
            },
        )
        .await
        .expect("read TUI Query Artifact preview");

    assert!(!query_view.truncated);
    assert_eq!(query_view.preview, query_bytes);

    let result_bytes = vec![b'x'; 4_097];
    let result = fixture
        .persist_internal_artifact(ArtifactKind::QueryResult, result_bytes)
        .await;
    let result_view = fixture
        .service
        .get_artifact(
            &result.id,
            ArtifactAccessContext {
                workspace_id: fixture.workspace_id,
                principal_id: fixture.principal.id,
                purpose: ArtifactAccessPurpose::TuiPreview,
                max_sensitivity: Sensitivity::Internal,
            },
        )
        .await
        .expect("read TUI result Artifact preview");

    assert!(result_view.truncated);
    assert_eq!(result_view.preview.len(), 4_096);
}

#[tokio::test]
async fn query_result_preview_is_policy_first_bounded_and_query_truncation_independent() {
    let fixture = ServiceFixture::new().await;
    let rows = (0..150)
        .map(|index| vec![CellValue::Integer(index), CellValue::Text("x".repeat(300))])
        .collect::<Vec<_>>();
    let result = QueryResult {
        columns: vec!["id".to_owned(), "payload".to_owned()],
        rows,
        truncated: false,
        remote_query_id: Some("must-not-leak".to_owned()),
        row_count: 150,
        serialized_bytes: 100_000,
        warning_codes: vec!["query_warning_must_remain_in_artifact".to_owned()],
        model_preview: "must-not-be-reused".to_owned(),
    };
    let metadata = fixture
        .persist_internal_artifact(
            ArtifactKind::QueryResult,
            serde_json::to_vec(&serde_json::json!({ "result": result }))
                .expect("serialize persisted result envelope"),
        )
        .await;
    let access = ArtifactAccessContext {
        workspace_id: fixture.workspace_id,
        principal_id: fixture.principal.id,
        purpose: ArtifactAccessPurpose::TuiPreview,
        max_sensitivity: Sensitivity::Internal,
    };

    let preview: QueryResultPreviewView = fixture
        .service
        .query_result_preview(&metadata.id, access.clone())
        .await
        .expect("build bounded preview");

    assert_eq!(preview.persisted_row_count(), 150);
    assert_eq!(preview.returned_row_count(), 100);
    assert!(preview.truncated());
    assert_eq!(
        preview.rows()[0][1]
            .as_str()
            .expect("text cell")
            .chars()
            .count(),
        256
    );
    assert!(
        serde_json::to_vec(&preview)
            .expect("serialize preview")
            .len()
            <= 64 * 1024
    );
    let rendered = serde_json::to_string(&preview).expect("serialize safe preview");
    for forbidden in [
        "must-not-leak",
        "query_warning_must_remain_in_artifact",
        "must-not-be-reused",
    ] {
        assert!(!rendered.contains(forbidden));
    }

    let query_truncated_only = QueryResult {
        columns: vec!["value".to_owned()],
        rows: vec![vec![CellValue::Integer(1)]],
        truncated: true,
        remote_query_id: None,
        row_count: 1,
        serialized_bytes: 1,
        warning_codes: vec!["max_rows_exceeded".to_owned()],
        model_preview: String::new(),
    };
    let metadata = fixture
        .persist_internal_artifact(
            ArtifactKind::QueryResult,
            serde_json::to_vec(&serde_json::json!({ "result": query_truncated_only }))
                .expect("serialize query-truncated result"),
        )
        .await;
    let preview = fixture
        .service
        .query_result_preview(&metadata.id, access)
        .await
        .expect("preview query-truncated result");
    assert!(
        !preview.truncated(),
        "query truncation is not UI truncation"
    );
}

#[tokio::test]
async fn query_result_preview_reports_policy_missing_and_malformed_failures_without_parsing_early()
{
    let fixture = ServiceFixture::new().await;
    let malformed = fixture
        .persist_internal_artifact(
            ArtifactKind::QueryResult,
            b"not-json-secret-canary".to_vec(),
        )
        .await;
    let denied = fixture
        .service
        .query_result_preview(
            &malformed.id,
            ArtifactAccessContext {
                workspace_id: fixture.workspace_id,
                principal_id: fixture.principal.id,
                purpose: ArtifactAccessPurpose::TuiPreview,
                max_sensitivity: Sensitivity::Public,
            },
        )
        .await
        .expect_err("Policy must reject before malformed content is parsed");
    assert_eq!(denied.code(), "artifact_access_denied");

    let malformed_error = fixture
        .service
        .query_result_preview(
            &malformed.id,
            ArtifactAccessContext {
                workspace_id: fixture.workspace_id,
                principal_id: fixture.principal.id,
                purpose: ArtifactAccessPurpose::TuiPreview,
                max_sensitivity: Sensitivity::Internal,
            },
        )
        .await
        .expect_err("authorized malformed content must stay an explicit failure");
    assert_eq!(malformed_error.code(), "malformed_query_result_artifact");

    let missing = fixture
        .service
        .query_result_preview(
            &ys_agent_core::ArtifactId::new(),
            ArtifactAccessContext {
                workspace_id: fixture.workspace_id,
                principal_id: fixture.principal.id,
                purpose: ArtifactAccessPurpose::TuiPreview,
                max_sensitivity: Sensitivity::Internal,
            },
        )
        .await
        .expect_err("missing metadata must remain an explicit failure");
    assert_eq!(missing.code(), "not_found");
}

fn front_door_model(
    calls: Arc<AtomicUsize>,
    action: AgentAction,
) -> Arc<ys_agent_adapters::model::FakeModelProvider> {
    Arc::new(ys_agent_adapters::model::FakeModelProvider::new({
        move |request| {
            let calls = calls.clone();
            let action = action.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert!(request.tools.is_empty());
                assert!(request.messages.iter().any(|message| {
                    message.content.contains("front-door agent")
                        && message.content.contains(r#""type":"respond""#)
                        && message.content.contains(r#""type":"start_query""#)
                        && message
                            .content
                            .contains(r#""type":"unsupported_capability""#)
                }));
                Ok(ModelResponse {
                    action,
                    raw_content: None,
                    usage: None,
                })
            }
        }
    }))
}

#[tokio::test]
async fn greeting_calls_the_model_once_without_starting_a_query_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = front_door_model(
        calls.clone(),
        AgentAction::Respond {
            message: "你好！我可以帮你查询已配置的数据源。".to_owned(),
        },
    );
    let fixture = ServiceFixture::with_conversation_model(model).await;
    let command_id = CommandId::new();
    let request = SendMessageRequest::new(command_id, fixture.session_id(), "你好，介绍一下你自己");

    let first = fixture
        .service
        .send_message(request.clone())
        .await
        .expect("first greeting");
    let replay = fixture
        .service
        .send_message(request)
        .await
        .expect("idempotent greeting replay");

    assert!(matches!(
        first,
        ServiceReply::Conversation { ref message }
            if message == "你好！我可以帮你查询已配置的数据源。"
    ));
    assert_eq!(first, replay);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn non_keyword_chat_is_classified_as_respond_without_a_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = front_door_model(
        calls.clone(),
        AgentAction::Respond {
            message: "我是 Ys-da，可以回答已配置数据源上的事实问题。".to_owned(),
        },
    );
    let fixture = ServiceFixture::with_conversation_model(model).await;

    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "你今天怎么样，能聊聊吗？",
        ))
        .await
        .expect("chat reply");

    assert!(matches!(reply, ServiceReply::Conversation { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn data_request_start_query_creates_exactly_one_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = front_door_model(calls.clone(), AgentAction::StartQuery);
    let fixture = ServiceFixture::with_conversation_model(model).await;
    let command_id = CommandId::new();
    let request = SendMessageRequest::new(
        command_id,
        fixture.session_id(),
        "GMV from 2026-08-12 through 2026-08-15 UTC",
    );

    let first = fixture
        .service
        .send_message(request.clone())
        .await
        .expect("start query");
    let replay = fixture
        .service
        .send_message(request)
        .await
        .expect("replay start query");

    assert_eq!(first.run_id(), replay.run_id());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.created_run_count().await, 1);
}

#[tokio::test]
async fn front_door_unsupported_capability_creates_no_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = front_door_model(
        calls.clone(),
        AgentAction::UnsupportedCapability {
            capability: "analysis".to_owned(),
            message: "Forecasting is not executable in v0.2; no Run was created.".to_owned(),
        },
    );
    let fixture = ServiceFixture::with_conversation_model(model).await;

    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Forecast next quarter GMV",
        ))
        .await
        .expect("unsupported reply");

    assert!(matches!(reply, ServiceReply::UnsupportedCapability { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn new_session_does_not_cancel_existing_tasks() {
    let fixture = ServiceFixture::new().await;
    let session_one = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("first session");
    let task = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: session_one.id,
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("create task");

    let session_two = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("second session");

    assert_ne!(session_one.id, session_two.id);
    assert_eq!(
        fixture
            .service
            .get_task(&task.id)
            .await
            .expect("load task")
            .status,
        TaskStatus::Open
    );
}

#[tokio::test]
async fn unrelated_input_creates_a_new_task() {
    let fixture = ServiceFixture::new().await;
    let session = fixture
        .service
        .create_session(CommandId::new(), fixture.principal())
        .await
        .expect("session");
    let gmv = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: session.id,
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("task");

    let decision = fixture
        .coordinator
        .route(&session, Some(&gmv), "Query yesterday's DAU")
        .await
        .expect("route");

    assert!(matches!(decision, CoordinationDecision::FrontDoor { .. }));
}

#[tokio::test]
async fn unsupported_workflow_is_explicit_and_creates_no_run() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            focused_task_id: None,
            text: "Explain why GMV fell and change the dbt model".to_owned(),
        })
        .await
        .expect("unsupported reply");

    assert!(matches!(reply, ServiceReply::UnsupportedCapability { .. }));
    assert_eq!(fixture.created_run_count().await, 0);
}

#[tokio::test]
async fn repeated_send_message_command_creates_only_one_run() {
    let fixture = ServiceFixture::new().await;
    let command_id = CommandId::new();
    let request = SendMessageRequest::new(
        command_id,
        fixture.session_id(),
        "GMV for the last seven complete days",
    );

    let first = fixture
        .service
        .send_message(request.clone())
        .await
        .expect("first message");
    let second = fixture
        .service
        .send_message(request)
        .await
        .expect("replayed message");

    assert_eq!(first.run_id(), second.run_id());
    assert_eq!(fixture.created_run_count().await, 1);
    assert_eq!(fixture.scheduler.count().await, 1);
}

#[tokio::test]
async fn repeated_send_message_with_a_different_focus_is_an_idempotency_conflict() {
    let fixture = ServiceFixture::new().await;
    let first_task = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("first task");
    let second_task = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            goal: "Query DAU".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("second task");
    let command_id = CommandId::new();

    fixture
        .service
        .send_message(SendMessageRequest {
            command_id,
            session_id: fixture.session_id(),
            focused_task_id: Some(first_task.id),
            text: "same range".to_owned(),
        })
        .await
        .expect("first message");
    let error = fixture
        .service
        .send_message(SendMessageRequest {
            command_id,
            session_id: fixture.session_id(),
            focused_task_id: Some(second_task.id),
            text: "same range".to_owned(),
        })
        .await
        .expect_err("changing focus must conflict with the original command");

    assert!(matches!(error, CoreError::IdempotencyConflict { .. }));
}

#[tokio::test]
async fn replay_conflict_is_detected_before_loading_a_different_focus() {
    let fixture = ServiceFixture::new().await;
    let command_id = CommandId::new();

    fixture
        .service
        .send_message(SendMessageRequest::new(
            command_id,
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect("first message");
    let error = fixture
        .service
        .send_message(SendMessageRequest {
            command_id,
            session_id: fixture.session_id(),
            focused_task_id: Some(ys_agent_core::TaskId::new()),
            text: "Query GMV".to_owned(),
        })
        .await
        .expect_err("a conflicting replay must fail before loading its focus");

    assert!(matches!(error, CoreError::IdempotencyConflict { .. }));
}

#[tokio::test]
async fn mutating_run_commands_accept_borrowed_ids() {
    let fixture = ServiceFixture::new().await;
    let task = fixture
        .service
        .create_task(CreateTaskRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .expect("task");

    let resumed_run_id = fixture
        .service
        .resume_task(CommandId::new(), &task.id)
        .await
        .expect("resume task");
    fixture
        .service
        .cancel_run(
            CommandId::new(),
            &resumed_run_id,
            "operator request".to_owned(),
        )
        .await
        .expect("cancel run");
    assert_eq!(
        fixture
            .service
            .get_run(&resumed_run_id)
            .await
            .expect("cancelled run")
            .status,
        RunStatus::Cancelled
    );

    let clarification = fixture
        .service
        .send_message(SendMessageRequest {
            command_id: CommandId::new(),
            session_id: fixture.session_id(),
            focused_task_id: Some(task.id),
            text: "change it".to_owned(),
        })
        .await
        .expect("clarification request");
    let clarification_run_id = clarification.run_id().expect("clarification run");
    fixture
        .service
        .answer_clarification(
            CommandId::new(),
            &clarification_run_id,
            "Use the previous range".to_owned(),
        )
        .await
        .expect("answer clarification");

    assert_eq!(
        fixture
            .service
            .get_run(&clarification_run_id)
            .await
            .expect("resumed run")
            .status,
        RunStatus::Running
    );
}

#[tokio::test]
async fn subscription_reads_durable_events_before_live_notifications() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect("send message");
    let run_id = reply.run_id().expect("scheduled run");

    let mut subscription = fixture
        .service
        .subscribe_events(&run_id, 0)
        .await
        .expect("subscribe");
    let event = subscription.next().await.expect("durable event");

    assert_eq!(event.sequence, 1);
    assert!(matches!(
        event.event.kind,
        RunEventKind::ProviderBound { .. }
    ));
    assert!(matches!(
        subscription
            .next()
            .await
            .expect("RunStarted event")
            .event
            .kind,
        RunEventKind::RunStarted
    ));
}

#[tokio::test]
async fn lagged_subscription_reloads_from_the_durable_sequence() {
    let fixture = ServiceFixture::new().await;
    let reply = fixture
        .service
        .send_message(SendMessageRequest::new(
            CommandId::new(),
            fixture.session_id(),
            "Query GMV",
        ))
        .await
        .expect("send message");
    let run_id = reply.run_id().expect("scheduled run");
    let mut subscription = fixture
        .service
        .subscribe_events(&run_id, 1)
        .await
        .expect("subscribe after RunStarted");

    let current = fixture.store.load_run(&run_id).await.expect("load run");
    let mut next = current.clone();
    next.version += 1;
    fixture
        .store
        .append(
            &run_id,
            current.version,
            vec![],
            vec![
                PendingRunEvent {
                    actor: EventActor::System,
                    kind: RunEventKind::StepStarted {
                        step_id: StepId::new(),
                        label: "plan".to_owned(),
                    },
                },
                PendingRunEvent {
                    actor: EventActor::System,
                    kind: RunEventKind::StepStarted {
                        step_id: StepId::new(),
                        label: "verify".to_owned(),
                    },
                },
            ],
            &next,
        )
        .await
        .expect("append durable events");

    let publisher = fixture.service.event_publisher();
    for sequence in 2..10 {
        publisher.notify(run_id, sequence);
    }
    let recovered = subscription.next().await.expect("reloaded event");

    assert_eq!(recovered.sequence, 2);
    assert_eq!(subscription.last_sequence(), 2);
}
