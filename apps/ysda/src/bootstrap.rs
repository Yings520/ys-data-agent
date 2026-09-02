use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
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
    AgentAction, ArtifactId, ArtifactStore, AssistantToolCall, ContextManifest, CoreError,
    CoreResult, CredentialReference, MetricDefinition, MetricProvider, ModelCapabilities,
    ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelRole, QueryBudget,
    QueryContextProvider, QueryExecutionPlan, RunId, RuntimeStore, Sensitivity, SideEffect,
    SourceId, ToolRisk, ToolSpec, WorkspaceId,
};
use ys_agent_runtime::{
    AgentServiceApi, ContextAssembler, Harness, HarnessConfig, HarnessDependencies,
    InMemoryQueryContextProvider, InProcessAgentService, LoopDriver, PromptBuilder, RunScheduler,
    ServiceEventPublisher, StaticRunProviderBindingSource,
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
    artifact_retention_days: u32,
    artifact_root: PathBuf,
    export_root: PathBuf,
}

impl AppConfig {
    fn from_env() -> CoreResult<Self> {
        let source_kind = optional_env("YSDA_DATA_SOURCE_KIND", "sqlite");
        let missing_config_keys = required_config_keys(&source_kind)
            .into_iter()
            .filter(|key| nonempty_env(key).is_none())
            .map(str::to_owned)
            .collect();
        let (query_budget_explicit, query_budget) = query_budget_from_lookup(&nonempty_env)?;
        let artifact_retention_days = artifact_retention_days_from_lookup(&nonempty_env)?;
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
            artifact_retention_days,
            artifact_root: PathBuf::from(".ysda/artifacts"),
            export_root: PathBuf::from(".ysda/exports"),
        })
    }
}

fn required_config_keys(source_kind: &str) -> Vec<&'static str> {
    let mut keys = vec![
        "YSDA_LLM_BASE_URL",
        "YSDA_LLM_API_KEY",
        "YSDA_LLM_MODEL",
        "YSDA_QUERY_POLICY_PATH",
        "YSDA_TIMEZONE",
        "YSDA_QUERY_TIMEOUT_SECONDS",
        "YSDA_QUERY_MAX_ROWS",
        "YSDA_QUERY_MAX_RESULT_BYTES",
        "YSDA_ARTIFACT_RETENTION_DAYS",
    ];
    keys.push(if source_kind == "postgres" {
        "YSDA_DATA_SOURCE_URL"
    } else {
        "YSDA_SQLITE_PATH"
    });
    keys
}

fn query_budget_from_lookup(
    lookup: &impl Fn(&str) -> Option<String>,
) -> CoreResult<(bool, QueryBudget)> {
    let required_budget_keys = [
        "YSDA_QUERY_TIMEOUT_SECONDS",
        "YSDA_QUERY_MAX_ROWS",
        "YSDA_QUERY_MAX_RESULT_BYTES",
    ];
    let explicit = required_budget_keys
        .into_iter()
        .all(|key| lookup(key).is_some());
    let mut budget = QueryBudget::default();
    if let Some(value) = lookup("YSDA_QUERY_TIMEOUT_SECONDS") {
        budget.statement_timeout_ms =
            parse_nonzero(&value, "YSDA_QUERY_TIMEOUT_SECONDS")?.saturating_mul(1_000);
    }
    if let Some(value) = lookup("YSDA_QUERY_MAX_ROWS") {
        budget.max_rows = parse_nonzero(&value, "YSDA_QUERY_MAX_ROWS")? as usize;
    }
    if let Some(value) = lookup("YSDA_QUERY_MAX_RESULT_BYTES") {
        budget.max_result_bytes = parse_nonzero(&value, "YSDA_QUERY_MAX_RESULT_BYTES")? as usize;
    }
    if let Some(value) = lookup("YSDA_QUERY_MAX_ESTIMATED_COST_UNITS") {
        budget.max_estimated_cost_units = Some(parse_nonzero(
            &value,
            "YSDA_QUERY_MAX_ESTIMATED_COST_UNITS",
        )?);
    }
    Ok((explicit, budget))
}

fn artifact_retention_days_from_lookup(
    lookup: &impl Fn(&str) -> Option<String>,
) -> CoreResult<u32> {
    const DEFAULT_ARTIFACT_RETENTION_DAYS: u32 = 7;
    let Some(value) = lookup("YSDA_ARTIFACT_RETENTION_DAYS") else {
        return Ok(DEFAULT_ARTIFACT_RETENTION_DAYS);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|days| *days > 0)
        .ok_or_else(|| {
            CoreError::validation(
                "invalid_artifact_retention",
                "YSDA_ARTIFACT_RETENTION_DAYS must be a positive integer",
            )
        })
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

#[cfg(unix)]
fn write_new_private_file(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(storage_error("create workspace ID temporary file"))?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(storage_error("write workspace ID temporary file"))?;
        file.sync_all()
            .map_err(storage_error("sync workspace ID temporary file"))?;
        enforce_private_file_permissions(path)?;
        file.sync_all()
            .map_err(storage_error("sync workspace ID temporary file"))
    })();
    drop(file);
    if result.is_err() {
        let _ = remove_workspace_id_temporary_file(path);
    }
    result
}

#[cfg(not(unix))]
fn write_new_private_file(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(storage_error("create workspace ID temporary file"))?;
    use std::io::Write;
    let result = (|| {
        file.write_all(bytes)
            .map_err(storage_error("write workspace ID temporary file"))?;
        file.sync_all()
            .map_err(storage_error("sync workspace ID temporary file"))
    })();
    drop(file);
    if result.is_err() {
        let _ = remove_workspace_id_temporary_file(path);
    }
    result
}

#[cfg(unix)]
fn enforce_private_file_permissions(path: &Path) -> CoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(storage_error("secure workspace ID file"))
}

#[cfg(not(unix))]
fn enforce_private_file_permissions(_: &Path) -> CoreResult<()> {
    Ok(())
}

fn load_workspace_id(path: &Path) -> CoreResult<Option<WorkspaceId>> {
    match fs::read_to_string(path) {
        Ok(value) => {
            let workspace_id = value.trim().parse::<WorkspaceId>().map_err(|error| {
                CoreError::validation(
                    "workspace_id_invalid",
                    format!(
                        "persisted workspace ID at {} is malformed: {error}",
                        path.display()
                    ),
                )
            })?;
            enforce_private_file_permissions(path)?;
            Ok(Some(workspace_id))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(storage_error("read workspace ID")(error)),
    }
}

fn remove_workspace_id_temporary_file(path: &Path) -> CoreResult<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage_error("remove workspace ID temporary file")(error)),
    }
    sync_workspace_id_directory(path)
}

#[cfg(unix)]
fn sync_workspace_id_directory(path: &Path) -> CoreResult<()> {
    let directory = path.parent().ok_or_else(|| CoreError::Storage {
        message: format!(
            "workspace ID temporary file has no parent: {}",
            path.display()
        ),
    })?;
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_error("sync workspace ID directory"))
}

#[cfg(not(unix))]
fn sync_workspace_id_directory(_: &Path) -> CoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn acquire_workspace_id_lock(path: &Path) -> CoreResult<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(storage_error("open workspace ID lock"))?;
    enforce_private_file_permissions(path)?;
    file.lock().map_err(storage_error("lock workspace ID"))?;
    Ok(file)
}

#[cfg(not(unix))]
fn acquire_workspace_id_lock(path: &Path) -> CoreResult<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(storage_error("open workspace ID lock"))?;
    file.lock().map_err(storage_error("lock workspace ID"))?;
    Ok(file)
}

fn recover_workspace_id_temporary_files(workspace_root: &Path) -> CoreResult<()> {
    for entry in
        fs::read_dir(workspace_root).map_err(storage_error("read workspace ID directory"))?
    {
        let entry = entry.map_err(storage_error("read workspace ID directory entry"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("workspace-id.") || !name.ends_with(".tmp") {
            continue;
        }
        let temporary_path = entry.path();
        remove_workspace_id_temporary_file(&temporary_path)?;
    }
    Ok(())
}

fn resolve_workspace_id(workspace_root: &Path) -> CoreResult<WorkspaceId> {
    let lock_path = workspace_root.join("workspace-id.lock");
    let workspace_id_lock = acquire_workspace_id_lock(&lock_path)?;
    let result = (|| {
        let path = workspace_root.join("workspace-id");
        if let Some(workspace_id) = load_workspace_id(&path)? {
            recover_workspace_id_temporary_files(workspace_root)?;
            return Ok(workspace_id);
        }

        recover_workspace_id_temporary_files(workspace_root)?;
        if let Some(workspace_id) = load_workspace_id(&path)? {
            return Ok(workspace_id);
        }

        let workspace_id = WorkspaceId::new();
        let temporary_path = workspace_root.join(format!("workspace-id.{workspace_id}.tmp"));
        write_new_private_file(&temporary_path, format!("{workspace_id}\n").as_bytes())?;

        match fs::hard_link(&temporary_path, &path) {
            Ok(()) => {
                if let Err(error) = sync_workspace_id_directory(&temporary_path) {
                    let _ = remove_workspace_id_temporary_file(&temporary_path);
                    return Err(error);
                }
                remove_workspace_id_temporary_file(&temporary_path)?;
                enforce_private_file_permissions(&path)?;
                Ok(workspace_id)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                remove_workspace_id_temporary_file(&temporary_path)?;
                load_workspace_id(&path)?.ok_or_else(|| CoreError::Storage {
                    message: format!(
                        "workspace ID at {} disappeared after concurrent publication",
                        path.display()
                    ),
                })
            }
            Err(error) => {
                remove_workspace_id_temporary_file(&temporary_path)?;
                Err(storage_error("publish workspace ID")(error))
            }
        }
    })();
    drop(workspace_id_lock);
    result
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
    model_protocol: Option<ModelProtocolProbe>,
}

#[async_trait]
impl DoctorProbe for RuntimeDoctorProbe {
    async fn inspect(&self) -> CoreResult<DoctorInputs> {
        let mut inputs = self.readiness.clone();
        if let Some(model_protocol) = &self.model_protocol {
            inputs.model = model_protocol.readiness().await;
        }
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

const DOCTOR_TOOL_NAME: &str = "ysda_doctor_probe";
const DOCTOR_DECOY_TOOL_NAME: &str = "ysda_doctor_decoy";

#[derive(Clone)]
struct ModelProtocolProbe {
    model: Arc<dyn ModelProvider>,
    model_name: String,
    cached: Arc<OnceCell<ModelReadiness>>,
}

impl ModelProtocolProbe {
    fn new(model: Arc<dyn ModelProvider>, model_name: String) -> Self {
        Self {
            model,
            model_name,
            cached: Arc::new(OnceCell::new()),
        }
    }

    async fn readiness(&self) -> ModelReadiness {
        self.cached
            .get_or_init(|| async {
                match tokio::time::timeout(
                    Duration::from_secs(45),
                    probe_model_protocol(self.model.as_ref(), &self.model_name),
                )
                .await
                {
                    Ok(Ok(readiness)) => readiness,
                    Ok(Err(error)) => {
                        tracing::warn!(
                            code = error.code(),
                            "model protocol readiness probe failed"
                        );
                        incompatible_model_readiness()
                    }
                    Err(_) => {
                        tracing::warn!(
                            code = "model_protocol_probe_timeout",
                            "model protocol readiness probe timed out"
                        );
                        incompatible_model_readiness()
                    }
                }
            })
            .await
            .clone()
    }
}

async fn probe_model_protocol(
    model: &dyn ModelProvider,
    model_name: &str,
) -> CoreResult<ModelReadiness> {
    let tool = doctor_probe_tool();
    let first = model
        .complete(ModelRequest {
            model: model_name.to_owned(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::System,
                    content: "This is a YSDA protocol readiness probe. It contains no business data. Call exactly the supplied ysda_doctor_probe tool with an empty object."
                        .to_owned(),
                    tool_call_id: None,
                    name: None,
                    assistant_tool_call: None,
                },
                ModelMessage {
                    role: ModelRole::User,
                    content: "Call ysda_doctor_probe now.".to_owned(),
                    tool_call_id: None,
                    name: None,
                    assistant_tool_call: None,
                },
            ],
            tools: vec![tool.clone(), doctor_decoy_tool()],
            context_manifest: ContextManifest::empty(512),
            temperature: Some(0.0),
        })
        .await?;
    let AgentAction::CallTool { call } = first.action else {
        return Err(model_protocol_error(
            "probe response did not contain a structured Tool Call",
        ));
    };
    if call.name != DOCTOR_TOOL_NAME {
        return Err(model_protocol_error(
            "probe response called a different tool",
        ));
    }
    let provider_call_id = call
        .provider_call_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| model_protocol_error("probe Tool Call had no provider call ID"))?;

    let second = model
        .complete(ModelRequest {
            model: model_name.to_owned(),
            messages: vec![
                ModelMessage {
                    role: ModelRole::System,
                    content: r#"This is a YSDA protocol readiness probe with no business data. After the Tool result, return exactly this JSON and nothing else: {"type":"propose_completion","summary":"protocol probe complete","primary_artifact_hint":null}. Do not call another tool."#
                        .to_owned(),
                    tool_call_id: None,
                    name: None,
                    assistant_tool_call: None,
                },
                ModelMessage {
                    role: ModelRole::Assistant,
                    content: String::new(),
                    tool_call_id: None,
                    name: None,
                    assistant_tool_call: Some(AssistantToolCall {
                        provider_call_id: provider_call_id.clone(),
                        name: DOCTOR_TOOL_NAME.to_owned(),
                        arguments: call.arguments,
                    }),
                },
                ModelMessage {
                    role: ModelRole::Tool,
                    content: r#"{"ready":true}"#.to_owned(),
                    tool_call_id: Some(provider_call_id),
                    name: Some(DOCTOR_TOOL_NAME.to_owned()),
                    assistant_tool_call: None,
                },
            ],
            tools: vec![tool, doctor_decoy_tool()],
            context_manifest: ContextManifest::empty(512),
            temperature: Some(0.0),
        })
        .await?;
    match second.action {
        AgentAction::ProposeCompletion {
            summary,
            primary_artifact_hint,
        } if summary == "protocol probe complete" && primary_artifact_hint.is_none() => {}
        AgentAction::CallTool { .. } => {
            return Err(model_protocol_error(
                "probe follow-up response called another tool",
            ));
        }
        AgentAction::Respond { .. } => {
            return Err(model_protocol_error(
                "probe follow-up response used respond instead of propose_completion",
            ));
        }
        AgentAction::RequestClarification { .. } => {
            return Err(model_protocol_error(
                "probe follow-up response used request_clarification instead of propose_completion",
            ));
        }
        _ => {
            return Err(model_protocol_error(
                "probe follow-up response was not the required propose_completion action",
            ));
        }
    }

    let capabilities = model.capabilities();
    if capabilities.max_context_tokens == 0 {
        return Err(model_protocol_error(
            "model reported an unknown context limit",
        ));
    }
    Ok(ModelReadiness {
        reachable: true,
        supports_tool_calls: true,
        supports_tool_call_ids: true,
        supports_multi_turn_tool_results: true,
        context_limit: Some(u64::from(capabilities.max_context_tokens)),
    })
}

fn doctor_decoy_tool() -> ToolSpec {
    ToolSpec {
        name: DOCTOR_DECOY_TOOL_NAME.to_owned(),
        description: "A decoy protocol probe that must never be called".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "properties": { "ready": { "type": "boolean" } },
            "required": ["ready"],
            "additionalProperties": false
        }),
        risk: ToolRisk::Low,
        side_effect: SideEffect::None,
        idempotent: true,
        timeout_ms: 1_000,
        max_output_bytes: 128,
        required_permissions: Vec::new(),
        input_sensitivity: Sensitivity::Internal,
        output_sensitivity: Sensitivity::Internal,
        version: "1.0.0".to_owned(),
    }
}

fn doctor_probe_tool() -> ToolSpec {
    ToolSpec {
        name: DOCTOR_TOOL_NAME.to_owned(),
        description: "A harmless protocol-only readiness probe that returns no business data"
            .to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        output_schema: serde_json::json!({
            "type": "object",
            "properties": { "ready": { "type": "boolean" } },
            "required": ["ready"],
            "additionalProperties": false
        }),
        risk: ToolRisk::Low,
        side_effect: SideEffect::None,
        idempotent: true,
        timeout_ms: 1_000,
        max_output_bytes: 128,
        required_permissions: Vec::new(),
        input_sensitivity: Sensitivity::Internal,
        output_sensitivity: Sensitivity::Internal,
        version: "1.0.0".to_owned(),
    }
}

fn incompatible_model_readiness() -> ModelReadiness {
    ModelReadiness {
        reachable: false,
        supports_tool_calls: false,
        supports_tool_call_ids: false,
        supports_multi_turn_tool_results: false,
        context_limit: None,
    }
}

fn model_protocol_error(message: &'static str) -> CoreError {
    CoreError::validation("model_protocol_incompatible", message)
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
    scheduled: Arc<Mutex<HashSet<RunId>>>,
    publisher: Mutex<Option<ServiceEventPublisher>>,
}

impl BackgroundScheduler {
    fn new(driver: Arc<LoopDriver>) -> Self {
        Self {
            driver,
            scheduled: Arc::new(Mutex::new(HashSet::new())),
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
        let scheduled = self.scheduled.clone();
        let publisher = self
            .publisher
            .lock()
            .expect("scheduler publisher mutex")
            .clone();
        tokio::spawn(async move {
            let result = driver.run(&run_id).await;
            scheduled
                .lock()
                .expect("scheduler run mutex")
                .remove(&run_id);
            match result {
                Ok(result) => {
                    if let Some(publisher) = publisher {
                        publisher.notify(run_id, result.snapshot.version);
                    }
                }
                Err(error) => tracing::warn!(
                    code = error.code(),
                    error = %error,
                    "background run driver failed"
                ),
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
) -> CoreResult<(
    Arc<BackgroundScheduler>,
    DoctorInputs,
    Arc<dyn ModelProvider>,
)> {
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
            model: model.clone(),
            catalog: Arc::new(catalog),
            tool_runtime: Arc::new(ToolRuntime::with_max_same_call_retries(1)),
            context_assembler: Arc::new(ContextAssembler::new(metrics, dbt_context, run_context)),
            telemetry,
        },
        PromptBuilder::new(config.llm_model.clone()),
        HarnessConfig {
            workspace_id,
            principal,
            workspace_timezone: config.timezone.clone(),
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
        model,
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
    let active_provider = seed_deterministic_active_provider(runtime.as_ref()).await?;
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
            workspace_timezone: config.timezone.clone().unwrap_or_default(),
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
    let service: Arc<dyn AgentServiceApi> = Arc::new(
        InProcessAgentService::new(workspace_id, runtime_store, artifact_store, scheduler)
            .with_run_provider_binding_source(Arc::new(
                StaticRunProviderBindingSource::from_active(active_provider),
            )),
    );
    Ok(DeterministicRuntimeAssembly {
        service,
        workspace_id,
        principal,
        phase_tool_view_hashes,
    })
}

async fn seed_deterministic_active_provider(
    runtime: &ys_agent_store::SqliteRuntimeStore,
) -> CoreResult<ys_agent_core::ActiveProviderSnapshot> {
    fn fixture_error(error: ys_agent_core::ProviderManagementError) -> CoreError {
        CoreError::validation("deterministic_provider_fixture_failed", error.code())
    }

    let repository = runtime.provider_repository();
    let profile_id = ys_agent_core::ProfileId::new();
    let name = ys_agent_core::ProfileName::new("Deterministic Replay Provider")?;
    let model = ys_agent_core::ProviderModelId::new(
        ys_agent_core::ProviderId::DeepSeek,
        "deepseek/deterministic-replay",
    )?;
    repository
        .save_revision(ys_agent_core::SaveProfileRevision {
            precondition: ys_agent_core::RevisionPrecondition {
                profile_id,
                expected_current_revision: None,
            },
            name,
            revision: ys_agent_core::ProfileRevision::draft(
                profile_id,
                1,
                ys_agent_core::ProviderId::DeepSeek,
                model.clone(),
                ys_agent_core::ProviderParameters::default(),
                None,
            )?,
        })
        .await
        .map_err(fixture_error)?;

    let credential = ys_agent_core::CredentialGeneration::new(
        profile_id,
        1,
        ys_agent_core::CredentialKind::ApiKey,
    )?;
    let mutation_id = ys_agent_core::OperationId::new();
    repository
        .begin_credential_mutation(ys_agent_core::CredentialMutationIntent::create(
            mutation_id,
            profile_id,
            1,
            credential,
        )?)
        .await
        .map_err(fixture_error)?;
    repository
        .record_credential_vault_write(mutation_id)
        .await
        .map_err(fixture_error)?;
    let candidate = ys_agent_core::ProfileRevision::draft(
        profile_id,
        2,
        ys_agent_core::ProviderId::DeepSeek,
        model,
        ys_agent_core::ProviderParameters::default(),
        Some(credential),
    )?;
    repository
        .commit_credential_pointer(ys_agent_core::CredentialPointerCommit::new(
            mutation_id,
            profile_id,
            1,
            candidate.clone(),
        )?)
        .await
        .map_err(fixture_error)?;
    repository
        .complete_credential_mutation(mutation_id)
        .await
        .map_err(fixture_error)?;

    let evidence = ys_agent_core::CompatibilityEvidence::passing(candidate.validation_inputs(
        ys_agent_core::ValidationVersions::new(
            "deterministic-catalog",
            "deterministic-probe",
            "deterministic-liter",
            "deterministic-codec",
        ),
    ));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    repository
        .save_validation(ys_agent_core::ValidationCommit {
            precondition: ys_agent_core::ValidationCommitPrecondition {
                operation_id: ys_agent_core::OperationId::new(),
                profile_id,
                revision: 2,
                credential_generation: credential,
                validation_digest: validation_digest.clone(),
            },
            evidence,
        })
        .await
        .map_err(fixture_error)?;
    repository
        .activate(ys_agent_core::ActivateProfileRequest {
            operation_id: ys_agent_core::OperationId::new(),
            precondition: ys_agent_core::ActivationPrecondition {
                profile_id,
                revision: 2,
                validation_id,
                validation_digest,
                expected_activation_revision: None,
            },
        })
        .await
        .map_err(fixture_error)
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
    let (scheduler, readiness, background, model_protocol, conversation_model) =
        match assemble_scheduler(
            &config,
            workspace_id,
            principal.clone(),
            runtime_port.clone(),
            artifact_port.clone(),
        )
        .await
        {
            Ok((background, readiness, model)) => {
                let scheduler: Arc<dyn RunScheduler> = background.clone();
                (
                    scheduler,
                    readiness,
                    Some(background),
                    Some(ModelProtocolProbe::new(
                        model.clone(),
                        config.llm_model.clone(),
                    )),
                    Some(model),
                )
            }
            Err(_) => {
                let scheduler: Arc<dyn RunScheduler> = Arc::new(ys_agent_runtime::NoopRunScheduler);
                (scheduler, safe_readiness_inputs(&config), None, None, None)
            }
        };
    let doctor = Arc::new(WorkspaceDoctor::new(Arc::new(RuntimeDoctorProbe {
        config: config.clone(),
        readiness,
        model_protocol,
    })));
    let exporter = Arc::new(ArtifactExporter::with_retention_days(
        runtime_port.clone(),
        artifact_port.clone(),
        Arc::new(OwnerOnlyExportWriter::new(&config.export_root)),
        Arc::new(DefaultExportPolicy),
        config.artifact_retention_days,
    ));
    let mut service = InProcessAgentService::with_dependencies_and_retention(
        workspace_id,
        runtime_port,
        artifact_port,
        scheduler.clone(),
        doctor,
        exporter,
        config.artifact_retention_days,
    );
    if let Some(model) = conversation_model {
        service = service.with_conversation_model(model, config.llm_model.clone());
    }
    let service = Arc::new(service);
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
    use std::{
        collections::VecDeque,
        fs,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::{Mutex as AsyncMutex, Notify};

    use super::{
        BackgroundScheduler, ModelProtocolProbe, acquire_workspace_id_lock,
        artifact_retention_days_from_lookup, create_private_directory, query_budget_from_lookup,
        required_config_keys, resolve_workspace_id,
    };
    use ys_agent_core::{
        AgentAction, CoreError, CoreResult, ModelCapabilities, ModelProvider, ModelRequest,
        ModelResponse, RunId, RunSnapshot, RunStatus, TaskId, ToolCall, ToolCallId, WorkflowKind,
        WorkspaceId,
    };
    use ys_agent_runtime::{HarnessStep, LoopDriver, RunScheduler, StepAccounting, StepOutcome};

    struct WaitingHarness {
        calls: Arc<AtomicUsize>,
        called: Arc<Notify>,
    }

    #[async_trait]
    impl HarnessStep for WaitingHarness {
        async fn step(&self, run_id: &RunId) -> CoreResult<StepOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.called.notify_one();
            Ok(StepOutcome::Wait {
                snapshot: RunSnapshot {
                    run_id: *run_id,
                    task_id: TaskId::new(),
                    workflow: WorkflowKind::Query,
                    status: RunStatus::WaitingForInput,
                    attempt: 1,
                    retry_of_run_id: None,
                    version: 1,
                    workflow_state: serde_json::json!({}),
                    pending_wait_metadata: None,
                    primary_artifact_id: None,
                    last_completed_step_id: None,
                },
                accounting: StepAccounting::default(),
            })
        }

        async fn fail_terminal(
            &self,
            _run_id: &RunId,
            _code: &'static str,
            _message: String,
        ) -> CoreResult<RunSnapshot> {
            unreachable!("waiting harness never fails")
        }
    }

    async fn wait_for_calls(harness: &WaitingHarness, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while harness.calls.load(Ordering::SeqCst) < expected {
                harness.called.notified().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("scheduler did not reach {expected} calls"));
    }

    #[tokio::test]
    async fn background_scheduler_releases_a_waiting_run_for_resumption() {
        let harness = Arc::new(WaitingHarness {
            calls: Arc::new(AtomicUsize::new(0)),
            called: Arc::new(Notify::new()),
        });
        let scheduler =
            BackgroundScheduler::new(Arc::new(LoopDriver::with_defaults(harness.clone())));
        let run_id = RunId::new();

        scheduler.schedule(run_id).await.expect("first schedule");
        wait_for_calls(&harness, 1).await;
        scheduler.schedule(run_id).await.expect("resume schedule");
        wait_for_calls(&harness, 2).await;

        assert_eq!(harness.calls.load(Ordering::SeqCst), 2);
    }

    #[derive(Clone)]
    struct ScriptedDoctorModel {
        responses: Arc<AsyncMutex<VecDeque<ModelResponse>>>,
        requests: Arc<AsyncMutex<Vec<ModelRequest>>>,
    }

    impl ScriptedDoctorModel {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Arc::new(AsyncMutex::new(responses.into())),
                requests: Arc::new(AsyncMutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for ScriptedDoctorModel {
        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities {
                tool_calling: true,
                structured_outputs: true,
                max_context_tokens: 32_768,
                parallel_tool_calls: false,
                streaming: false,
            }
        }

        async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse> {
            self.requests.lock().await.push(request);
            self.responses
                .lock()
                .await
                .pop_front()
                .ok_or(CoreError::ReplayExhausted)
        }
    }

    fn doctor_tool_call(name: &str, provider_call_id: Option<&str>) -> ModelResponse {
        ModelResponse {
            action: AgentAction::CallTool {
                call: ToolCall {
                    id: ToolCallId::new(),
                    provider_call_id: provider_call_id.map(str::to_owned),
                    name: name.to_owned(),
                    arguments: json!({}),
                    version: "1.0.0".to_owned(),
                },
            },
            raw_content: None,
            usage: None,
        }
    }

    fn doctor_completion() -> ModelResponse {
        ModelResponse {
            action: AgentAction::ProposeCompletion {
                summary: "protocol probe complete".to_owned(),
                primary_artifact_hint: None,
            },
            raw_content: None,
            usage: None,
        }
    }

    #[tokio::test]
    async fn model_protocol_probe_uses_the_original_tool_call_id_and_caches_success() {
        let model = Arc::new(ScriptedDoctorModel::new(vec![
            doctor_tool_call("ysda_doctor_probe", Some("call_probe_1")),
            doctor_completion(),
        ]));
        let probe = ModelProtocolProbe::new(model.clone(), "test-model".to_owned());

        let first = probe.readiness().await;
        let second = probe.readiness().await;

        assert_eq!(first, second);
        assert!(first.reachable);
        assert!(first.supports_tool_calls);
        assert!(first.supports_tool_call_ids);
        assert!(first.supports_multi_turn_tool_results);
        assert_eq!(first.context_limit, Some(32_768));
        let requests = model.requests.lock().await;
        assert_eq!(
            requests.len(),
            2,
            "cached probe must not call the model again"
        );
        assert_eq!(requests[0].tools.len(), 2);
        assert_eq!(requests[0].tools[0].name, "ysda_doctor_probe");
        assert_eq!(requests[0].tools[1].name, "ysda_doctor_decoy");
        assert_eq!(requests[1].tools.len(), 2);
        let tool_result = requests[1]
            .messages
            .iter()
            .find(|message| message.role == ys_agent_core::ModelRole::Tool)
            .expect("second request contains a Tool result");
        assert_eq!(tool_result.tool_call_id.as_deref(), Some("call_probe_1"));
        assert_eq!(tool_result.name.as_deref(), Some("ysda_doctor_probe"));
        let assistant_call = requests[1]
            .messages
            .iter()
            .find_map(|message| message.assistant_tool_call.as_ref())
            .expect("second request replays the assistant Tool Call");
        assert_eq!(assistant_call.provider_call_id, "call_probe_1");
        assert_eq!(assistant_call.name, "ysda_doctor_probe");
    }

    #[tokio::test]
    async fn model_protocol_probe_fails_closed_when_the_second_turn_responds() {
        let model = Arc::new(ScriptedDoctorModel::new(vec![
            doctor_tool_call("ysda_doctor_probe", Some("call_probe_respond")),
            ModelResponse {
                action: AgentAction::Respond {
                    message: "ready".to_owned(),
                },
                raw_content: None,
                usage: None,
            },
        ]));
        let probe = ModelProtocolProbe::new(model, "test-model".to_owned());

        let readiness = probe.readiness().await;

        assert!(!readiness.reachable);
        assert!(!readiness.supports_tool_calls);
    }

    #[tokio::test]
    async fn model_protocol_probe_fails_closed_when_the_second_turn_is_not_exact_completion() {
        let model = Arc::new(ScriptedDoctorModel::new(vec![
            doctor_tool_call("ysda_doctor_probe", Some("call_probe_wrong")),
            ModelResponse {
                action: AgentAction::ProposeCompletion {
                    summary: "almost".to_owned(),
                    primary_artifact_hint: None,
                },
                raw_content: None,
                usage: None,
            },
        ]));
        let probe = ModelProtocolProbe::new(model, "test-model".to_owned());

        let readiness = probe.readiness().await;

        assert!(!readiness.reachable);
    }

    #[tokio::test]
    async fn model_protocol_probe_fails_closed_when_the_first_turn_calls_the_decoy() {
        let model = Arc::new(ScriptedDoctorModel::new(vec![
            doctor_tool_call("ysda_doctor_decoy", Some("call_decoy")),
            doctor_completion(),
        ]));
        let probe = ModelProtocolProbe::new(model, "test-model".to_owned());

        let readiness = probe.readiness().await;

        assert!(!readiness.reachable);
    }

    #[tokio::test]
    async fn model_protocol_probe_fails_closed_when_the_second_turn_calls_a_tool_again() {
        let model = Arc::new(ScriptedDoctorModel::new(vec![
            doctor_tool_call("ysda_doctor_probe", Some("call_probe_2")),
            doctor_tool_call("ysda_doctor_probe", Some("call_probe_3")),
        ]));
        let probe = ModelProtocolProbe::new(model, "test-model".to_owned());

        let readiness = probe.readiness().await;

        assert!(!readiness.reachable);
        assert!(!readiness.supports_tool_calls);
        assert!(!readiness.supports_tool_call_ids);
        assert!(!readiness.supports_multi_turn_tool_results);
        assert_eq!(readiness.context_limit, None);
    }

    #[test]
    fn documented_query_cost_limit_is_parsed_into_the_runtime_budget() {
        let lookup = |key: &str| match key {
            "YSDA_QUERY_TIMEOUT_SECONDS" => Some("17".to_owned()),
            "YSDA_QUERY_MAX_ROWS" => Some("23".to_owned()),
            "YSDA_QUERY_MAX_RESULT_BYTES" => Some("4096".to_owned()),
            "YSDA_QUERY_MAX_ESTIMATED_COST_UNITS" => Some("71".to_owned()),
            _ => None,
        };

        let (explicit, budget) = query_budget_from_lookup(&lookup).expect("query budget");

        assert!(explicit);
        assert_eq!(budget.statement_timeout_ms, 17_000);
        assert_eq!(budget.max_rows, 23);
        assert_eq!(budget.max_result_bytes, 4_096);
        assert_eq!(budget.max_estimated_cost_units, Some(71));
    }

    #[test]
    fn artifact_retention_is_required_and_parsed_as_positive_days() {
        assert!(required_config_keys("sqlite").contains(&"YSDA_ARTIFACT_RETENTION_DAYS"));
        assert_eq!(
            artifact_retention_days_from_lookup(&|key| {
                (key == "YSDA_ARTIFACT_RETENTION_DAYS").then(|| "19".to_owned())
            })
            .expect("artifact retention"),
            19
        );
    }

    #[test]
    fn invalid_artifact_retention_has_a_specific_error_code() {
        let error = artifact_retention_days_from_lookup(&|_| Some("0".to_owned()))
            .expect_err("zero retention must fail");

        assert_eq!(error.code(), "invalid_artifact_retention");
    }

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
    fn concurrent_first_workspace_initializers_publish_one_id_without_temporary_files() {
        for _ in 0..64 {
            let directory = tempfile::tempdir().expect("temporary directory");
            let workspace_root = directory.path().join(".ysda");
            create_private_directory(&workspace_root).expect("create workspace directory");
            let barrier = Arc::new(Barrier::new(2));

            let resolvers = (0..2)
                .map(|_| {
                    let workspace_root = workspace_root.clone();
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        resolve_workspace_id(&workspace_root)
                    })
                })
                .collect::<Vec<_>>();
            let ids = resolvers
                .into_iter()
                .map(|resolver| resolver.join().expect("resolver thread"))
                .collect::<Result<Vec<_>, _>>()
                .expect("resolve workspace IDs");

            assert_eq!(ids[0], ids[1]);
            let temporary_files = fs::read_dir(&workspace_root)
                .expect("read workspace directory")
                .map(|entry| entry.expect("workspace directory entry"))
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("workspace-id.") && name.ends_with(".tmp")
                })
                .collect::<Vec<_>>();
            assert!(
                temporary_files.is_empty(),
                "temporary identity files remain"
            );
        }
    }

    #[test]
    fn resolving_existing_workspace_id_recovers_all_temporary_files_after_locking_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");

        let workspace_id = resolve_workspace_id(&workspace_root).expect("create workspace ID");
        let persisted_path = workspace_root.join("workspace-id");
        let stale_temporary_path = workspace_root.join("workspace-id.stale.tmp");
        fs::hard_link(&persisted_path, &stale_temporary_path)
            .expect("create stale published temporary link");

        let active_temporary_path = workspace_root.join("workspace-id.active.tmp");
        fs::write(
            &active_temporary_path,
            "crashed initializer before publication\n",
        )
        .expect("create active temporary file");

        let resolved =
            resolve_workspace_id(&workspace_root).expect("resolve persisted workspace ID");

        assert_eq!(resolved, workspace_id);
        assert!(
            !stale_temporary_path.exists(),
            "published temporary hard link should be recovered"
        );
        assert!(
            !active_temporary_path.exists(),
            "a crashed pre-publication temporary file should be recovered"
        );
    }

    #[test]
    fn resolving_new_workspace_id_recovers_crashed_temporary_files_after_locking_workspace() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");

        let crashed_temporary_path = workspace_root.join("workspace-id.crashed.tmp");
        fs::write(&crashed_temporary_path, "crashed initializer\n")
            .expect("create crashed temporary file");

        let active_temporary_path = workspace_root.join("workspace-id.active.tmp");
        fs::write(&active_temporary_path, "another crashed initializer\n")
            .expect("create active temporary file");

        resolve_workspace_id(&workspace_root).expect("resolve a new workspace ID");

        assert!(
            !crashed_temporary_path.exists(),
            "a crashed initializer's temporary file should be recovered"
        );
        assert!(
            !active_temporary_path.exists(),
            "every crashed temporary file should be recovered"
        );
    }

    #[test]
    fn workspace_identity_lock_serializes_concurrent_resolvers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");
        let lock = acquire_workspace_id_lock(&workspace_root.join("workspace-id.lock"))
            .expect("acquire workspace identity lock");
        let (sender, receiver) = mpsc::channel();
        let resolver_workspace_root = workspace_root.clone();
        let resolver =
            thread::spawn(move || sender.send(resolve_workspace_id(&resolver_workspace_root)));

        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "a resolver must wait until the active resolver releases the workspace identity lock"
        );

        drop(lock);
        let workspace_id = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("resolver completes after lock release")
            .expect("resolve workspace ID");
        resolver
            .join()
            .expect("resolver thread")
            .expect("send resolver result");
        assert_eq!(
            fs::read_to_string(workspace_root.join("workspace-id"))
                .expect("read workspace ID")
                .trim()
                .parse::<WorkspaceId>()
                .expect("parse workspace ID"),
            workspace_id
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolving_existing_workspace_id_enforces_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace_root = directory.path().join(".ysda");
        create_private_directory(&workspace_root).expect("create workspace directory");
        let workspace_id = WorkspaceId::new();
        let path = workspace_root.join("workspace-id");
        fs::write(&path, format!("{workspace_id}\n")).expect("write workspace ID");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("make workspace ID permissive");

        let resolved = resolve_workspace_id(&workspace_root).expect("resolve workspace ID");
        let mode = fs::metadata(&path)
            .expect("read workspace ID metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(resolved, workspace_id);
        assert_eq!(mode, 0o600);
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
