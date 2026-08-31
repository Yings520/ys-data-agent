use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use ys_agent_adapters::model::{
    OpenAiCompatibleConfig, OpenAiCompatibleProvider, ReplayModelProvider, SecretString,
};
use ys_agent_adapters::{
    ConnectorRegistry, DbtManifestAdapter, FileMetricRegistry, InspectSchemaTool, MetricSqlDialect,
    PostgresConnector, PostgresConnectorConfig, QueryDataTool, ReadFreshnessTool,
    ResolveMetricTool, ResultPolicy, RuntimeArtifactLookup, SqlReadOnlyPolicy, SqliteConnector,
    SqliteConnectorConfig, SupportedDialect,
};
use ys_agent_core::{
    AgentAction, ArtifactId, ArtifactStore, CoreError, CoreResult, CredentialReference,
    MetricDefinition, MetricProvider, ModelCapabilities, ModelProvider, ModelRequest,
    ModelResponse, QueryBudget, QueryContextProvider, QueryExecutionPlan, RunId, RuntimeStore,
    SourceId, WorkspaceId,
};
use ys_agent_runtime::{
    AgentServiceApi, ContextAssembler, Harness, HarnessConfig, HarnessDependencies,
    InMemoryQueryContextProvider, InProcessAgentService, LoopDriver, PromptBuilder, RunScheduler,
    ServiceEventPublisher,
    doctor::{DoctorInputs, DoctorProbe, ModelReadiness, SourceReadiness, WorkspaceDoctor},
    export::{ArtifactExporter, DefaultExportPolicy, ExportWriter, WrittenExport},
    telemetry::{TelemetryDispatcher, TracingTelemetrySink},
    tools::{
        ConnectorToolAvailability, QueryPhase, ToolCatalog, ToolRuntime, ToolViewBuilder,
        WorkspaceToolPolicy,
    },
};

#[derive(Debug, Clone)]
pub struct DisplayMetadata {
    pub workspace_name: String,
    pub model_label: String,
    pub connection_label: String,
    pub permission_label: String,
}

pub struct AppDependencies {
    pub service: Arc<dyn AgentServiceApi>,
    pub workspace_id: WorkspaceId,
    pub principal: ys_agent_core::Principal,
    pub display: DisplayMetadata,
}

/// Deterministic inputs for the production-shaped Query Eval composition.
///
/// This entry point deliberately accepts paths instead of reading application environment
/// variables. It is only used by the release Eval and never selects a live model provider.
#[derive(Debug, Clone)]
pub struct DeterministicRuntimeConfig {
    pub runtime_path: PathBuf,
    pub artifact_path: PathBuf,
    pub sqlite_path: PathBuf,
    pub metric_registry_path: PathBuf,
    pub dbt_manifest_path: PathBuf,
    pub query_policy_path: PathBuf,
    pub timezone: Option<String>,
    pub replay: Vec<ModelResponse>,
    pub secret_canary: String,
}

pub struct DeterministicRuntimeAssembly {
    pub service: Arc<dyn AgentServiceApi>,
    pub workspace_id: WorkspaceId,
    pub principal: ys_agent_core::Principal,
    pub phase_tool_view_hashes: BTreeMap<String, String>,
}

#[derive(Clone)]
struct DeterministicReplayModelProvider {
    responses: Arc<AsyncMutex<VecDeque<ModelResponse>>>,
    current_run_id: Arc<AsyncMutex<Option<RunId>>>,
    store: Arc<dyn RuntimeStore>,
}

impl DeterministicReplayModelProvider {
    fn new(
        responses: Vec<ModelResponse>,
        current_run_id: Arc<AsyncMutex<Option<RunId>>>,
        store: Arc<dyn RuntimeStore>,
    ) -> Self {
        Self {
            responses: Arc::new(AsyncMutex::new(responses.into())),
            current_run_id,
            store,
        }
    }

    async fn set_current_run(&self, run_id: RunId) {
        *self.current_run_id.lock().await = Some(run_id);
    }

    async fn next_response(&self) -> CoreResult<ModelResponse> {
        let mut response = self
            .responses
            .lock()
            .await
            .pop_front()
            .ok_or(CoreError::ReplayExhausted)?;
        let run_id = self.current_run_id.lock().await.ok_or_else(|| {
            CoreError::validation(
                "deterministic_replay_run_missing",
                "Replay model was called before a Query Run was scheduled",
            )
        })?;
        let snapshot = self.store.load_run(&run_id).await?;
        let state = ys_agent_runtime::QueryWorkflowState::from_snapshot(snapshot.workflow_state)?;
        materialize_replay_sentinels(&mut response, &state)?;
        Ok(response)
    }
}

#[async_trait]
impl ModelProvider for DeterministicReplayModelProvider {
    fn capabilities(&self) -> ModelCapabilities {
        ReplayModelProvider::from_responses(Vec::new()).capabilities()
    }

    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
        let response = self.next_response().await?;
        ReplayModelProvider::from_responses(vec![response])
            .complete(request)
            .await
    }
}

fn materialize_replay_sentinels(
    response: &mut ModelResponse,
    state: &ys_agent_runtime::QueryWorkflowState,
) -> CoreResult<()> {
    match &mut response.action {
        AgentAction::ProposeQueryPlan { plan } => {
            let mut parsed: ys_agent_core::QueryPlan = serde_json::from_value(plan.clone())
                .map_err(|error| {
                    CoreError::validation("deterministic_replay_plan_invalid", error.to_string())
                })?;
            if let QueryExecutionPlan::AdHoc {
                assumption_refs, ..
            } = &mut parsed.execution
            {
                let evidence = state.schema_evidence.first().ok_or_else(|| {
                    CoreError::validation(
                        "deterministic_replay_schema_evidence_missing",
                        "AdHoc Replay plan needs persisted schema Evidence",
                    )
                })?;
                *assumption_refs = vec![evidence.id()];
            }
            *plan = serde_json::to_value(parsed).map_err(|error| {
                CoreError::validation("deterministic_replay_plan_serialize", error.to_string())
            })?;
        }
        AgentAction::CallTool { call } if call.name == "query_data" => {
            let arguments = call.arguments.as_object_mut().ok_or_else(|| {
                CoreError::validation(
                    "deterministic_replay_arguments_invalid",
                    "query_data Replay arguments must be an object",
                )
            })?;
            let plan = state.execution_plan.as_ref().ok_or_else(|| {
                CoreError::validation(
                    "deterministic_replay_plan_missing",
                    "query_data Replay needs a persisted QueryPlan",
                )
            })?;
            replace_replay_string(arguments, "plan_artifact_id", plan.id().to_string());
            replace_replay_string(arguments, "plan_hash", plan.metadata.content_hash.clone());
            if arguments.get("action").and_then(serde_json::Value::as_str) == Some("execute") {
                let preflight = state.preflight.as_ref().ok_or_else(|| {
                    CoreError::validation(
                        "deterministic_replay_preflight_missing",
                        "execute Replay needs persisted preflight Evidence",
                    )
                })?;
                replace_replay_string(
                    arguments,
                    "preflight_artifact_id",
                    preflight.id().to_string(),
                );
                replace_replay_string(
                    arguments,
                    "preflight_hash",
                    preflight.metadata.content_hash.clone(),
                );
            }
        }
        _ => {}
    }
    Ok(())
}

fn replace_replay_string(
    arguments: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    replacement: String,
) {
    if arguments.contains_key(key) {
        arguments.insert(key.to_owned(), serde_json::Value::String(replacement));
    }
}

struct DeterministicRunScheduler {
    driver: Arc<LoopDriver>,
    model: DeterministicReplayModelProvider,
}

#[async_trait]
impl RunScheduler for DeterministicRunScheduler {
    async fn schedule(&self, run_id: RunId) -> CoreResult<()> {
        self.model.set_current_run(run_id).await;
        self.driver.run(&run_id).await.map(|_| ())
    }
}

#[derive(Debug, Clone)]
struct AppConfig {
    workspace_name: String,
    llm_base_url_ref: CredentialReference,
    llm_api_key_ref: CredentialReference,
    llm_model: String,
    source_kind: String,
    source_id: SourceId,
    source_url_ref: Option<CredentialReference>,
    sqlite_path: PathBuf,
    metric_registry_path: PathBuf,
    dbt_manifest_path: Option<PathBuf>,
    query_policy_path: PathBuf,
    timezone: String,
    missing_config_keys: Vec<String>,
    query_budget_explicit: bool,
    query_budget: QueryBudget,
    artifact_root: PathBuf,
    export_root: PathBuf,
}

impl AppConfig {
    fn from_env() -> CoreResult<Self> {
        let source_kind = optional_env("YSDA_DATA_SOURCE_KIND", "sqlite");
        let mut required_keys = vec![
            "YSDA_LLM_BASE_URL",
            "YSDA_LLM_API_KEY",
            "YSDA_LLM_MODEL",
            "YSDA_QUERY_POLICY_PATH",
            "YSDA_TIMEZONE",
            "YSDA_QUERY_TIMEOUT_SECONDS",
            "YSDA_QUERY_MAX_ROWS",
            "YSDA_QUERY_MAX_RESULT_BYTES",
        ];
        required_keys.push(if source_kind == "postgres" {
            "YSDA_DATA_SOURCE_URL"
        } else {
            "YSDA_SQLITE_PATH"
        });
        let missing_config_keys = required_keys
            .into_iter()
            .filter(|key| nonempty_env(key).is_none())
            .map(str::to_owned)
            .collect();
        let query_budget_explicit = [
            "YSDA_QUERY_TIMEOUT_SECONDS",
            "YSDA_QUERY_MAX_ROWS",
            "YSDA_QUERY_MAX_RESULT_BYTES",
        ]
        .into_iter()
        .all(|key| nonempty_env(key).is_some());
        let mut query_budget = QueryBudget::default();
        if let Some(value) = nonempty_env("YSDA_QUERY_TIMEOUT_SECONDS") {
            query_budget.statement_timeout_ms =
                parse_nonzero(&value, "YSDA_QUERY_TIMEOUT_SECONDS")?.saturating_mul(1_000);
        }
        if let Some(value) = nonempty_env("YSDA_QUERY_MAX_ROWS") {
            query_budget.max_rows = parse_nonzero(&value, "YSDA_QUERY_MAX_ROWS")? as usize;
        }
        if let Some(value) = nonempty_env("YSDA_QUERY_MAX_RESULT_BYTES") {
            query_budget.max_result_bytes =
                parse_nonzero(&value, "YSDA_QUERY_MAX_RESULT_BYTES")? as usize;
        }
        Ok(Self {
            workspace_name: optional_env("YSDA_WORKSPACE_NAME", "local"),
            llm_base_url_ref: CredentialReference::new("env:YSDA_LLM_BASE_URL")?,
            llm_api_key_ref: CredentialReference::new("env:YSDA_LLM_API_KEY")?,
            llm_model: optional_env("YSDA_LLM_MODEL", "unconfigured"),
            source_kind,
            source_id: SourceId::new(optional_env("YSDA_DATA_SOURCE_ID", "local")),
            source_url_ref: nonempty_env("YSDA_DATA_SOURCE_URL")
                .map(|_| CredentialReference::new("env:YSDA_DATA_SOURCE_URL"))
                .transpose()?,
            sqlite_path: PathBuf::from(optional_env("YSDA_SQLITE_PATH", ".ysda/demo.db")),
            metric_registry_path: PathBuf::from(optional_env(
                "YSDA_METRIC_REGISTRY_PATH",
                ".ysda/missing-metrics.json",
            )),
            dbt_manifest_path: nonempty_env("YSDA_DBT_MANIFEST_PATH").map(PathBuf::from),
            query_policy_path: PathBuf::from(optional_env(
                "YSDA_QUERY_POLICY_PATH",
                ".ysda/missing-query-policy.json",
            )),
            timezone: optional_env("YSDA_TIMEZONE", ""),
            missing_config_keys,
            query_budget_explicit,
            query_budget,
            artifact_root: PathBuf::from(".ysda/artifacts"),
            export_root: PathBuf::from(".ysda/exports"),
        })
    }
}

fn nonempty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn optional_env(key: &str, default: &str) -> String {
    nonempty_env(key).unwrap_or_else(|| default.to_owned())
}

fn parse_nonzero(value: &str, key: &str) -> CoreResult<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            CoreError::validation(
                "invalid_query_budget",
                format!("{key} must be a positive integer"),
            )
        })
}

pub struct OwnerOnlyExportWriter {
    root: PathBuf,
}

impl OwnerOnlyExportWriter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl ExportWriter for OwnerOnlyExportWriter {
    async fn write(
        &self,
        source_artifact_id: ArtifactId,
        extension: &str,
        bytes: &[u8],
    ) -> CoreResult<WrittenExport> {
        let root = self.root.clone();
        let extension = extension.to_owned();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || {
            let hash = hex_sha256(&bytes);
            let directory = root.join(source_artifact_id.to_string());
            create_private_directory(&directory)?;
            let path = directory.join(format!("{hash}.{extension}"));
            write_private_file(&path, &bytes)?;
            Ok(WrittenExport {
                storage_uri: path.to_string_lossy().into_owned(),
                content_hash: format!("sha256:{hash}"),
                size_bytes: bytes.len() as u64,
            })
        })
        .await
        .map_err(|error| CoreError::Storage {
            message: format!("export writer task failed: {error}"),
        })?
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> CoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).map_err(storage_error("create private directory"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(storage_error("secure private directory"))
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> CoreResult<()> {
    fs::create_dir_all(path).map_err(storage_error("create private directory"))
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(storage_error("open export file"))?;
    file.write_all(bytes)
        .map_err(storage_error("write export file"))?;
    file.sync_all().map_err(storage_error("sync export file"))
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    fs::write(path, bytes).map_err(storage_error("write export file"))
}

fn resolve_workspace_id(workspace_root: &Path) -> CoreResult<WorkspaceId> {
    let path = workspace_root.join("workspace-id");
    match fs::read_to_string(&path) {
        Ok(value) => value.trim().parse::<WorkspaceId>().map_err(|error| {
            CoreError::validation(
                "workspace_id_invalid",
                format!(
                    "persisted workspace ID at {} is malformed: {error}",
                    path.display()
                ),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let workspace_id = WorkspaceId::new();
            write_private_file(&path, format!("{workspace_id}\n").as_bytes())?;
            Ok(workspace_id)
        }
        Err(error) => Err(storage_error("read workspace ID")(error)),
    }
}

fn storage_error(context: &'static str) -> impl FnOnce(std::io::Error) -> CoreError {
    move |error| CoreError::Storage {
        message: format!("{context}: {error}"),
    }
}

#[derive(Clone)]
struct RuntimeDoctorProbe {
    config: AppConfig,
    readiness: DoctorInputs,
}

#[async_trait]
impl DoctorProbe for RuntimeDoctorProbe {
    async fn inspect(&self) -> CoreResult<DoctorInputs> {
        let mut inputs = self.readiness.clone();
        inputs.query_policy_valid = fs::read(&self.config.query_policy_path)
            .ok()
            .and_then(|bytes| ResultPolicy::from_json_bytes(&bytes).ok())
            .is_some();
        inputs.metric_registry_valid = FileMetricRegistry::load(&self.config.metric_registry_path)
            .await
            .is_ok();
        inputs.dbt_manifest_valid = match &self.config.dbt_manifest_path {
            None => None,
            Some(path) => Some(DbtManifestAdapter::load(path).await.is_ok()),
        };
        inputs.artifact_directory_private_and_writable =
            owner_only_writable(&self.config.artifact_root);
        inputs.export_directory_private_and_writable =
            owner_only_writable(&self.config.export_root);
        inputs.source.reachable = if self.config.source_kind == "sqlite" {
            self.config.sqlite_path.is_file() && fs::File::open(&self.config.sqlite_path).is_ok()
        } else {
            inputs.source.reachable
        };
        inputs.source.query_capability &= inputs.source.reachable;
        inputs.source.catalog_capability &= inputs.source.reachable;
        inputs.source.freshness_capability &= inputs.source.reachable;
        Ok(inputs)
    }
}

#[cfg(unix)]
fn owner_only_writable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_dir()
            && metadata.permissions().mode() & 0o777 == 0o700
            && !metadata.permissions().readonly()
    })
}

#[cfg(not(unix))]
fn owner_only_writable(path: &Path) -> bool {
    path.is_dir() && !fs::metadata(path).is_ok_and(|metadata| metadata.permissions().readonly())
}

fn safe_readiness_inputs(config: &AppConfig) -> DoctorInputs {
    DoctorInputs {
        missing_config_keys: config.missing_config_keys.clone(),
        model: ModelReadiness {
            reachable: false,
            supports_tool_calls: false,
            supports_tool_call_ids: false,
            supports_multi_turn_tool_results: false,
            context_limit: None,
        },
        source: SourceReadiness {
            reachable: false,
            query_capability: false,
            catalog_capability: false,
            freshness_capability: false,
            database_read_only: false,
        },
        query_policy_valid: false,
        metric_registry_valid: false,
        dbt_manifest_valid: config.dbt_manifest_path.as_ref().map(|_| false),
        timezone_explicit: !config.timezone.trim().is_empty(),
        freshness_rules_explicit: true,
        query_budget_explicit: config.query_budget_explicit,
        artifact_directory_private_and_writable: false,
        export_directory_private_and_writable: false,
    }
}

#[derive(Default)]
struct EmptyMetricProvider;

#[async_trait]
impl MetricProvider for EmptyMetricProvider {
    async fn get_metric(&self, _metric_id: &str) -> CoreResult<Option<MetricDefinition>> {
        Ok(None)
    }

    async fn list_active_metrics(&self) -> CoreResult<Vec<MetricDefinition>> {
        Ok(Vec::new())
    }
}

struct BackgroundScheduler {
    driver: Arc<LoopDriver>,
    scheduled: Mutex<HashSet<RunId>>,
    publisher: Mutex<Option<ServiceEventPublisher>>,
}

impl BackgroundScheduler {
    fn new(driver: Arc<LoopDriver>) -> Self {
        Self {
            driver,
            scheduled: Mutex::new(HashSet::new()),
            publisher: Mutex::new(None),
        }
    }

    fn set_publisher(&self, publisher: ServiceEventPublisher) {
        *self.publisher.lock().expect("scheduler publisher mutex") = Some(publisher);
    }
}

#[async_trait]
impl RunScheduler for BackgroundScheduler {
    async fn schedule(&self, run_id: RunId) -> CoreResult<()> {
        if !self
            .scheduled
            .lock()
            .expect("scheduler run mutex")
            .insert(run_id)
        {
            return Ok(());
        }
        let driver = self.driver.clone();
        let publisher = self
            .publisher
            .lock()
            .expect("scheduler publisher mutex")
            .clone();
        tokio::spawn(async move {
            match driver.run(&run_id).await {
                Ok(result) => {
                    if let Some(publisher) = publisher {
                        publisher.notify(run_id, result.snapshot.version);
                    }
                }
                Err(error) => tracing::warn!(code = error.code(), "background run driver failed"),
            }
        });
        Ok(())
    }
}

async fn assemble_scheduler(
    config: &AppConfig,
    workspace_id: WorkspaceId,
    principal: ys_agent_core::Principal,
    runtime_store: Arc<dyn RuntimeStore>,
    artifact_store: Arc<dyn ArtifactStore>,
) -> CoreResult<(Arc<BackgroundScheduler>, DoctorInputs)> {
    let base_url = resolve_env_reference(&config.llm_base_url_ref)?;
    let api_key = resolve_env_reference(&config.llm_api_key_ref)?;
    let model: Arc<dyn ModelProvider> =
        Arc::new(OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: SecretString::new(api_key),
            model: config.llm_model.clone(),
            supports_tool_calls: true,
            supports_tool_call_ids: true,
            supports_multi_turn_tool_results: true,
            context_window_tokens: 32_768,
            max_tool_schema_bytes: 65_536,
            request_timeout: Duration::from_secs(30),
        })?);
    let policy_bytes = tokio::fs::read(&config.query_policy_path)
        .await
        .map_err(|error| CoreError::validation("query_policy_read_failed", error.to_string()))?;
    let result_policy = ResultPolicy::from_json_bytes(&policy_bytes)?;
    let data_scope = result_policy.allowed_scope(workspace_id, &config.source_id)?;
    let metrics: Arc<dyn MetricProvider> =
        match FileMetricRegistry::load(&config.metric_registry_path).await {
            Ok(registry) => Arc::new(registry),
            Err(_) => Arc::new(EmptyMetricProvider),
        };
    let dbt_context: Arc<dyn QueryContextProvider> = match &config.dbt_manifest_path {
        Some(path) => match DbtManifestAdapter::load(path).await {
            Ok(adapter) => Arc::new(adapter),
            Err(_) => Arc::new(InMemoryQueryContextProvider::new()),
        },
        None => Arc::new(InMemoryQueryContextProvider::new()),
    };
    let run_context: Arc<dyn QueryContextProvider> = Arc::new(InMemoryQueryContextProvider::new());
    let mut connectors = ConnectorRegistry::new();
    let (dialect, source_read_only) = match config.source_kind.as_str() {
        "sqlite" => {
            if !config.sqlite_path.is_file() || fs::File::open(&config.sqlite_path).is_err() {
                return Err(CoreError::validation(
                    "sqlite_source_unavailable",
                    "SQLite source is not readable",
                ));
            }
            let connector = Arc::new(SqliteConnector::new(
                SqliteConnectorConfig {
                    source_id: config.source_id.clone(),
                    database_path: config.sqlite_path.clone(),
                    max_concurrency: config.query_budget.max_concurrency,
                    freshness_columns: BTreeMap::new(),
                },
                SqlReadOnlyPolicy::new(SupportedDialect::SQLite, config.query_budget.max_sql_bytes),
                result_policy,
            ));
            connectors.register(
                config.source_id.clone(),
                MetricSqlDialect::Sqlite,
                connector,
            )?;
            (MetricSqlDialect::Sqlite, true)
        }
        "postgres" => {
            let source_url = config.source_url_ref.as_ref().ok_or_else(|| {
                CoreError::validation(
                    "data_source_url_missing",
                    "PostgreSQL source URL is required",
                )
            })?;
            let connector = Arc::new(
                PostgresConnector::connect(
                    PostgresConnectorConfig {
                        source_id: config.source_id.clone(),
                        max_connections: config.query_budget.max_concurrency as u32,
                        acquire_timeout: Duration::from_millis(
                            config.query_budget.acquire_timeout_ms,
                        ),
                        default_statement_timeout: Duration::from_millis(
                            config.query_budget.statement_timeout_ms,
                        ),
                        confirmation_cost_units: 1_000,
                        freshness_columns: BTreeMap::new(),
                    },
                    &resolve_env_reference(source_url)?,
                    SqlReadOnlyPolicy::new(
                        SupportedDialect::Postgres,
                        config.query_budget.max_sql_bytes,
                    ),
                    result_policy,
                )
                .await?,
            );
            connectors.register(
                config.source_id.clone(),
                MetricSqlDialect::Postgres,
                connector,
            )?;
            (MetricSqlDialect::Postgres, true)
        }
        _ => {
            return Err(CoreError::validation(
                "unsupported_data_source",
                "source kind must be sqlite or postgres",
            ));
        }
    };
    let artifact_lookup = Arc::new(RuntimeArtifactLookup::new(
        runtime_store.clone(),
        artifact_store.clone(),
    ));
    let tools: Vec<Arc<dyn ys_agent_core::Tool>> = vec![
        Arc::new(ResolveMetricTool::new(metrics.clone())),
        Arc::new(InspectSchemaTool::new(
            connectors.clone(),
            artifact_store.clone(),
            20,
            200,
            32_768,
        )),
        Arc::new(ReadFreshnessTool::new(connectors.clone(), metrics.clone())),
        Arc::new(QueryDataTool::new(
            connectors,
            metrics.clone(),
            artifact_lookup,
            artifact_store.clone(),
        )),
    ];
    let tool_policy = WorkspaceToolPolicy::default();
    let mut catalog = ToolCatalog::with_policy(tool_policy.clone());
    for tool in tools {
        catalog.register_arc(tool)?;
    }
    let telemetry = Arc::new(TelemetryDispatcher::new(Arc::new(TracingTelemetrySink)));
    let harness = Arc::new(Harness::new(
        HarnessDependencies {
            store: runtime_store,
            artifacts: artifact_store,
            model,
            catalog: Arc::new(catalog),
            tool_runtime: Arc::new(ToolRuntime::with_max_same_call_retries(1)),
            context_assembler: Arc::new(ContextAssembler::new(metrics, dbt_context, run_context)),
            telemetry,
        },
        PromptBuilder::new(config.llm_model.clone()),
        HarnessConfig {
            workspace_id,
            principal,
            query_budget: config.query_budget.clone(),
            data_scope,
            connector_tools: ConnectorToolAvailability::all_query_tools(),
            tool_policy,
            context_token_budget: 8_000,
            schema_ttl: Duration::from_secs(300),
        },
    ));
    let readiness = DoctorInputs {
        missing_config_keys: config.missing_config_keys.clone(),
        model: ModelReadiness {
            reachable: true,
            supports_tool_calls: true,
            supports_tool_call_ids: true,
            supports_multi_turn_tool_results: true,
            context_limit: Some(32_768),
        },
        source: SourceReadiness {
            reachable: true,
            query_capability: true,
            catalog_capability: true,
            freshness_capability: true,
            database_read_only: source_read_only,
        },
        query_policy_valid: true,
        metric_registry_valid: FileMetricRegistry::load(&config.metric_registry_path)
            .await
            .is_ok(),
        dbt_manifest_valid: match &config.dbt_manifest_path {
            None => None,
            Some(path) => Some(DbtManifestAdapter::load(path).await.is_ok()),
        },
        timezone_explicit: !config.timezone.trim().is_empty(),
        freshness_rules_explicit: true,
        query_budget_explicit: config.query_budget_explicit,
        artifact_directory_private_and_writable: owner_only_writable(&config.artifact_root),
        export_directory_private_and_writable: owner_only_writable(&config.export_root),
    };
    let _ = dialect;
    Ok((
        Arc::new(BackgroundScheduler::new(Arc::new(
            LoopDriver::with_defaults(harness),
        ))),
        readiness,
    ))
}

/// Assemble the same governed Query runtime used in production, with only deterministic
/// composition substitutions: local paths, a fixed SQLite source, replayed model responses, and
/// a no-op telemetry sink.
pub async fn assemble_deterministic_query_runtime(
    config: DeterministicRuntimeConfig,
) -> CoreResult<DeterministicRuntimeAssembly> {
    let _fixture_timezone = config.timezone.as_deref();
    if config.secret_canary.trim().is_empty() {
        return Err(CoreError::validation(
            "deterministic_eval_secret_missing",
            "deterministic Eval requires a non-empty secret canary",
        ));
    }
    let workspace_id = WorkspaceId::new();
    let principal = ys_agent_core::Principal::local_operator("deterministic-eval");
    let source_id = SourceId::new("sqlite_demo");
    let query_budget = QueryBudget::default();
    let runtime = Arc::new(ys_agent_store::SqliteRuntimeStore::open(&config.runtime_path).await?);
    let artifacts = Arc::new(ys_agent_store::LocalArtifactStore::new(
        &config.artifact_path,
    )?);
    let runtime_store: Arc<dyn RuntimeStore> = runtime;
    let artifact_store: Arc<dyn ArtifactStore> = artifacts;
    let policy_bytes = tokio::fs::read(&config.query_policy_path)
        .await
        .map_err(|error| CoreError::validation("query_policy_read_failed", error.to_string()))?;
    let result_policy = ResultPolicy::from_json_bytes(&policy_bytes)?;
    let data_scope = result_policy.allowed_scope(workspace_id, &source_id)?;
    let metrics: Arc<dyn MetricProvider> = Arc::new(
        FileMetricRegistry::load(&config.metric_registry_path)
            .await
            .map_err(|error| {
                CoreError::validation("metric_registry_load_failed", error.to_string())
            })?,
    );
    let dbt_context: Arc<dyn QueryContextProvider> = Arc::new(
        DbtManifestAdapter::load(&config.dbt_manifest_path)
            .await
            .map_err(|error| {
                CoreError::validation("dbt_manifest_load_failed", error.to_string())
            })?,
    );
    let run_context: Arc<dyn QueryContextProvider> = Arc::new(InMemoryQueryContextProvider::new());
    let connector = Arc::new(SqliteConnector::new(
        SqliteConnectorConfig {
            source_id: source_id.clone(),
            database_path: config.sqlite_path,
            max_concurrency: query_budget.max_concurrency,
            freshness_columns: BTreeMap::new(),
        },
        SqlReadOnlyPolicy::new(SupportedDialect::SQLite, query_budget.max_sql_bytes),
        result_policy,
    ));
    let mut connectors = ConnectorRegistry::new();
    connectors.register(source_id, MetricSqlDialect::Sqlite, connector)?;
    let artifact_lookup = Arc::new(RuntimeArtifactLookup::new(
        runtime_store.clone(),
        artifact_store.clone(),
    ));
    let tools: Vec<Arc<dyn ys_agent_core::Tool>> = vec![
        Arc::new(ResolveMetricTool::new(metrics.clone())),
        Arc::new(InspectSchemaTool::new(
            connectors.clone(),
            artifact_store.clone(),
            20,
            200,
            32_768,
        )),
        Arc::new(ReadFreshnessTool::new(connectors.clone(), metrics.clone())),
        Arc::new(QueryDataTool::new(
            connectors,
            metrics.clone(),
            artifact_lookup,
            artifact_store.clone(),
        )),
    ];
    let tool_policy = WorkspaceToolPolicy::default();
    let mut catalog = ToolCatalog::with_policy(tool_policy.clone());
    for tool in tools {
        catalog.register_arc(tool)?;
    }
    let catalog = Arc::new(catalog);
    let connector_tools = ConnectorToolAvailability::all_query_tools();
    let phase_tool_view_hashes =
        query_phase_tool_view_hashes(catalog.as_ref(), &principal, connector_tools.clone())?;
    let current_run_id = Arc::new(AsyncMutex::new(None));
    let model = DeterministicReplayModelProvider::new(
        config.replay,
        current_run_id.clone(),
        runtime_store.clone(),
    );
    let telemetry = Arc::new(TelemetryDispatcher::default());
    let harness = Arc::new(Harness::new(
        HarnessDependencies {
            store: runtime_store.clone(),
            artifacts: artifact_store.clone(),
            model: Arc::new(model.clone()),
            catalog,
            tool_runtime: Arc::new(ToolRuntime::with_max_same_call_retries(1)),
            context_assembler: Arc::new(ContextAssembler::new(metrics, dbt_context, run_context)),
            telemetry,
        },
        PromptBuilder::new("deterministic-query-eval"),
        HarnessConfig {
            workspace_id,
            principal: principal.clone(),
            query_budget,
            data_scope,
            connector_tools,
            tool_policy,
            context_token_budget: 8_000,
            schema_ttl: Duration::from_secs(300),
        },
    ));
    let scheduler: Arc<dyn RunScheduler> = Arc::new(DeterministicRunScheduler {
        driver: Arc::new(LoopDriver::with_defaults(harness)),
        model,
    });
    let service: Arc<dyn AgentServiceApi> = Arc::new(InProcessAgentService::new(
        workspace_id,
        runtime_store,
        artifact_store,
        scheduler,
    ));
    Ok(DeterministicRuntimeAssembly {
        service,
        workspace_id,
        principal,
        phase_tool_view_hashes,
    })
}

fn query_phase_tool_view_hashes(
    catalog: &ToolCatalog,
    principal: &ys_agent_core::Principal,
    connector_tools: ConnectorToolAvailability,
) -> CoreResult<BTreeMap<String, String>> {
    [
        QueryPhase::Clarify,
        QueryPhase::ClassifyIntent,
        QueryPhase::ResolveContext,
        QueryPhase::Plan,
        QueryPhase::ValidateAndPreflight,
        QueryPhase::Execute,
        QueryPhase::Verify,
        QueryPhase::Package,
        QueryPhase::ReadyToComplete,
    ]
    .into_iter()
    .map(|phase| {
        let view = ToolViewBuilder::new(catalog)
            .for_workflow(ys_agent_core::WorkflowKind::Query)
            .for_query_phase(phase)
            .for_principal(principal)
            .with_connector_tools(connector_tools.clone())
            .for_run_status(ys_agent_core::RunStatus::Running)
            .build()?;
        Ok((
            query_phase_name(phase).to_owned(),
            view.content_hash().to_owned(),
        ))
    })
    .collect()
}

fn query_phase_name(phase: QueryPhase) -> &'static str {
    match phase {
        QueryPhase::Clarify => "clarify",
        QueryPhase::ClassifyIntent => "classify_intent",
        QueryPhase::ResolveContext => "resolve_context",
        QueryPhase::Plan => "plan",
        QueryPhase::ValidateAndPreflight => "validate_and_preflight",
        QueryPhase::Execute => "execute",
        QueryPhase::Verify => "verify",
        QueryPhase::Package => "package",
        QueryPhase::ReadyToComplete => "ready_to_complete",
    }
}

fn resolve_env_reference(reference: &CredentialReference) -> CoreResult<String> {
    nonempty_env(reference.environment_variable_name()).ok_or_else(|| {
        CoreError::validation(
            "required_config_missing",
            "a required environment value is missing",
        )
    })
}

pub async fn bootstrap() -> CoreResult<AppDependencies> {
    let config = AppConfig::from_env()?;
    create_private_directory(Path::new(".ysda"))?;
    create_private_directory(&config.artifact_root)?;
    create_private_directory(&config.export_root)?;
    let workspace_id = resolve_workspace_id(Path::new(".ysda"))?;
    let principal = ys_agent_core::Principal::local_operator(
        env::var("USER").unwrap_or_else(|_| "local-operator".to_owned()),
    );
    let display = DisplayMetadata {
        workspace_name: config.workspace_name.clone(),
        model_label: format!("openai-compatible/{}", config.llm_model),
        connection_label: config.source_kind.clone(),
        permission_label: "read-only".to_owned(),
    };
    let runtime_store =
        Arc::new(ys_agent_store::SqliteRuntimeStore::open(".ysda/runtime.db").await?);
    let artifact_store = Arc::new(ys_agent_store::LocalArtifactStore::new(
        &config.artifact_root,
    )?);
    let runtime_port: Arc<dyn RuntimeStore> = runtime_store.clone();
    let artifact_port: Arc<dyn ArtifactStore> = artifact_store.clone();
    let (scheduler, readiness, background) = match assemble_scheduler(
        &config,
        workspace_id,
        principal.clone(),
        runtime_port.clone(),
        artifact_port.clone(),
    )
    .await
    {
        Ok((background, readiness)) => {
            let scheduler: Arc<dyn RunScheduler> = background.clone();
            (scheduler, readiness, Some(background))
        }
        Err(_) => {
            let scheduler: Arc<dyn RunScheduler> = Arc::new(ys_agent_runtime::NoopRunScheduler);
            (scheduler, safe_readiness_inputs(&config), None)
        }
    };
    let doctor = Arc::new(WorkspaceDoctor::new(Arc::new(RuntimeDoctorProbe {
        config: config.clone(),
        readiness,
    })));
    let exporter = Arc::new(ArtifactExporter::new(
        runtime_port.clone(),
        artifact_port.clone(),
        Arc::new(OwnerOnlyExportWriter::new(&config.export_root)),
        Arc::new(DefaultExportPolicy),
    ));
    let service = Arc::new(InProcessAgentService::with_dependencies(
        workspace_id,
        runtime_port,
        artifact_port,
        scheduler.clone(),
        doctor,
        exporter,
    ));
    if let Some(background) = background {
        background.set_publisher(service.event_publisher());
    }
    Ok(AppDependencies {
        service,
        workspace_id,
        principal,
        display,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{create_private_directory, resolve_workspace_id};
    use ys_agent_core::{CoreError, WorkspaceId};

    #[test]
    fn workspace_id_is_persisted_for_later_bootstraps() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");

        let first = resolve_workspace_id(&workspace_root).expect("resolve first workspace ID");
        let second = resolve_workspace_id(&workspace_root).expect("resolve second workspace ID");
        let persisted = fs::read_to_string(workspace_root.join("workspace-id"))
            .expect("read persisted workspace ID");

        assert_eq!(first, second);
        assert_eq!(persisted, format!("{first}\n"));
        assert_eq!(
            persisted
                .trim()
                .parse::<WorkspaceId>()
                .expect("parse workspace ID"),
            first
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(workspace_root.join("workspace-id"))
                .expect("read workspace ID metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn malformed_persisted_workspace_id_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");
        fs::write(workspace_root.join("workspace-id"), "not-a-workspace-id\n")
            .expect("write malformed workspace ID");

        let error =
            resolve_workspace_id(&workspace_root).expect_err("reject malformed workspace ID");

        assert!(matches!(
            error,
            CoreError::Validation {
                code: "workspace_id_invalid",
                ..
            }
        ));
    }
}
