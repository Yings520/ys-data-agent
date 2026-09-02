use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use chrono::{Datelike, TimeZone, Utc};
use serde_json::json;
use tokio::sync::Mutex;
use ys_agent_core::{
    AgentAction, ArtifactStore, CoreError, CoreResult, ModelResponse, ModelUsage,
    QueryExecutionPlan, QueryPlan, RuntimeStore, SourceId, ToolCall, ToolCallId,
};

mod provider_fixture;

#[derive(Debug, Clone)]
pub enum ScriptedAction {
    Response(ModelResponse),
    AdHocPlan { sql: String },
    QueryDataPreflight,
    QueryDataExecute,
}

impl ScriptedAction {
    fn materialize(
        self,
        state: &ys_agent_runtime::QueryWorkflowState,
    ) -> CoreResult<ModelResponse> {
        match self {
            Self::Response(response) => Ok(response),
            Self::AdHocPlan { sql } => {
                let assumption_ref = state.schema_evidence.first().ok_or_else(|| {
                    CoreError::validation(
                        "fixture_schema_evidence_missing",
                        "AdHoc test plan needs persisted schema ContextEvidence",
                    )
                })?;
                Ok(ModelResponse {
                    action: AgentAction::ProposeQueryPlan {
                        plan: serde_json::to_value(QueryPlan {
                            source_id: SourceId::new("sqlite-demo"),
                            execution: QueryExecutionPlan::AdHoc {
                                sql,
                                assumption_refs: vec![assumption_ref.id()],
                            },
                        })
                        .map_err(|error| {
                            CoreError::validation("fixture_plan_serialization", error.to_string())
                        })?,
                    },
                    raw_content: None,
                    usage: Some(ModelUsage {
                        prompt_tokens: 10,
                        completion_tokens: 10,
                        total_tokens: 20,
                    }),
                })
            }
            Self::QueryDataPreflight => {
                let plan = state.execution_plan.as_ref().ok_or_else(|| {
                    CoreError::validation(
                        "fixture_plan_missing",
                        "preflight script needs a persisted QueryPlan",
                    )
                })?;
                Ok(tool_response(
                    "query_data",
                    json!({
                        "action": "preflight",
                        "plan_artifact_id": plan.id(),
                        "plan_hash": plan.metadata.content_hash.clone(),
                    }),
                ))
            }
            Self::QueryDataExecute => {
                let plan = state.execution_plan.as_ref().ok_or_else(|| {
                    CoreError::validation(
                        "fixture_plan_missing",
                        "execute script needs a persisted QueryPlan",
                    )
                })?;
                let preflight = state.preflight.as_ref().ok_or_else(|| {
                    CoreError::validation(
                        "fixture_preflight_missing",
                        "execute script needs persisted preflight Evidence",
                    )
                })?;
                Ok(tool_response(
                    "query_data",
                    json!({
                        "action": "execute",
                        "plan_artifact_id": plan.id(),
                        "plan_hash": plan.metadata.content_hash.clone(),
                        "preflight_artifact_id": preflight.id(),
                        "preflight_hash": preflight.metadata.content_hash.clone(),
                    }),
                ))
            }
        }
    }
}

pub fn completion_response(summary: &str) -> ScriptedAction {
    ScriptedAction::Response(ModelResponse {
        action: AgentAction::ProposeCompletion {
            summary: summary.to_owned(),
            primary_artifact_hint: None,
        },
        raw_content: None,
        usage: Some(ModelUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }),
    })
}

pub fn propose_unsafe_adhoc_plan() -> ScriptedAction {
    ScriptedAction::AdHocPlan {
        sql: "DELETE FROM mart_orders".to_owned(),
    }
}

pub fn propose_safe_adhoc_plan() -> ScriptedAction {
    ScriptedAction::AdHocPlan {
        sql: "SELECT channel FROM mart_orders ORDER BY channel".to_owned(),
    }
}

fn plan_response(plan: QueryPlan) -> ScriptedAction {
    ScriptedAction::Response(ModelResponse {
        action: AgentAction::ProposeQueryPlan {
            plan: serde_json::to_value(plan).expect("serialize plan"),
        },
        raw_content: None,
        usage: Some(ModelUsage {
            prompt_tokens: 10,
            completion_tokens: 10,
            total_tokens: 20,
        }),
    })
}

pub fn call_query_data_preflight() -> ScriptedAction {
    ScriptedAction::QueryDataPreflight
}

pub fn call_query_data_execute() -> ScriptedAction {
    ScriptedAction::QueryDataExecute
}

pub fn propose_completion() -> ScriptedAction {
    completion_response("The verified result lists the observed order channels.")
}

fn tool_response(name: &str, arguments: serde_json::Value) -> ModelResponse {
    ModelResponse {
        action: AgentAction::CallTool {
            call: ToolCall {
                id: ToolCallId::new(),
                provider_call_id: None,
                name: name.to_owned(),
                arguments,
                version: "1.0.0".to_owned(),
            },
        },
        raw_content: None,
        usage: Some(ModelUsage {
            prompt_tokens: 10,
            completion_tokens: 10,
            total_tokens: 20,
        }),
    }
}

fn direct_tool_action(name: &str, arguments: serde_json::Value) -> ScriptedAction {
    ScriptedAction::Response(tool_response(name, arguments))
}

fn call_resolve_metric() -> ScriptedAction {
    direct_tool_action("resolve_metric", json!({ "metric": "commerce.gmv" }))
}

pub fn call_resolve_missing_metric() -> ScriptedAction {
    direct_tool_action("resolve_metric", json!({ "metric": "order.channels" }))
}

pub fn call_inspect_schema() -> ScriptedAction {
    direct_tool_action(
        "inspect_schema",
        json!({
            "source_id": "sqlite-demo",
            "relations": ["mart_orders"],
        }),
    )
}

fn call_read_freshness() -> ScriptedAction {
    direct_tool_action(
        "read_freshness",
        json!({
            "source_id": "sqlite-demo",
            "relation": "mart_orders",
            "time_column": "paid_at",
        }),
    )
}

fn metric_plan_response(
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> ScriptedAction {
    plan_response(QueryPlan {
        source_id: SourceId::new("sqlite-demo"),
        execution: QueryExecutionPlan::Metric {
            metric_id: "commerce.gmv".to_owned(),
            dimensions: Vec::new(),
            start,
            end,
        },
    })
}

fn metric_success_script() -> Vec<ScriptedAction> {
    vec![
        call_resolve_metric(),
        metric_plan_response(
            Utc.with_ymd_and_hms(2026, 8, 7, 0, 0, 0)
                .single()
                .expect("valid start"),
            Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0)
                .single()
                .expect("valid end"),
        ),
        call_query_data_preflight(),
        call_query_data_execute(),
        call_read_freshness(),
        completion_response("GMV is 10 for the requested complete period."),
    ]
}

fn metadata_success_script() -> Vec<ScriptedAction> {
    vec![
        call_inspect_schema(),
        completion_response("mart_orders contains the authorized observed columns."),
    ]
}

fn empty_metric_script() -> Vec<ScriptedAction> {
    vec![
        call_resolve_metric(),
        metric_plan_response(
            Utc.with_ymd_and_hms(1990, 1, 1, 0, 0, 0)
                .single()
                .expect("valid start"),
            Utc.with_ymd_and_hms(1990, 1, 2, 0, 0, 0)
                .single()
                .expect("valid end"),
        ),
        call_query_data_preflight(),
        call_query_data_execute(),
        call_read_freshness(),
        completion_response("No rows were returned for the requested period."),
    ]
}

pub struct QueryWorkflowFixture {
    _directory: tempfile::TempDir,
    runtime: Arc<ys_agent_store::SqliteRuntimeStore>,
    artifacts: Arc<ys_agent_store::LocalArtifactStore>,
    service: Arc<ys_agent_runtime::InProcessAgentService>,
    session_id: ys_agent_core::SessionId,
    driver: ys_agent_runtime::LoopDriver,
    current_run_id: Arc<Mutex<Option<ys_agent_core::RunId>>>,
    tool_counts: Arc<std::sync::Mutex<std::collections::BTreeMap<String, usize>>>,
    transport_retries: std::sync::atomic::AtomicUsize,
    cached_primary: std::sync::Mutex<Option<ys_agent_runtime::QueryArtifact>>,
    telemetry: Arc<TelemetryDispatcher>,
    model_requests: Arc<Mutex<Vec<ys_agent_core::ModelRequest>>>,
}

impl QueryWorkflowFixture {
    pub async fn with_model_actions(actions: Vec<ScriptedAction>) -> Self {
        Self::with_model_actions_and_telemetry(actions, Arc::new(TelemetryDispatcher::default()))
            .await
    }

    pub async fn with_model_actions_and_telemetry(
        actions: Vec<ScriptedAction>,
        telemetry: Arc<TelemetryDispatcher>,
    ) -> Self {
        let directory = tempfile::tempdir().expect("temporary runtime");
        let runtime = Arc::new(
            ys_agent_store::SqliteRuntimeStore::open(directory.path().join("runtime.db"))
                .await
                .expect("runtime store"),
        );
        let active_provider =
            provider_fixture::persisted_test_active_provider(runtime.as_ref()).await;
        let artifacts = Arc::new(
            ys_agent_store::LocalArtifactStore::new(directory.path()).expect("artifact store"),
        );
        let workspace_id = ys_agent_core::WorkspaceId::new();
        let principal = ys_agent_core::Principal::local_operator("tutorial");
        let service = Arc::new(
            ys_agent_runtime::InProcessAgentService::new(
                workspace_id,
                runtime.clone(),
                artifacts.clone(),
                Arc::new(ys_agent_runtime::NoopRunScheduler),
            )
            .with_run_provider_binding_source(Arc::new(
                ys_agent_runtime::StaticRunProviderBindingSource::from_active(active_provider),
            )),
        );
        let session = ys_agent_runtime::AgentServiceApi::create_session(
            service.as_ref(),
            ys_agent_core::CommandId::new(),
            principal.clone(),
        )
        .await
        .expect("create test session");
        let scripted_actions = Arc::new(Mutex::new(VecDeque::from(actions)));
        let current_run_id = Arc::new(Mutex::new(None));
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(ys_agent_adapters::model::FakeModelProvider::new({
            let scripted_actions = scripted_actions.clone();
            let current_run_id = current_run_id.clone();
            let runtime = runtime.clone();
            let model_requests = model_requests.clone();
            move |request| {
                let scripted_actions = scripted_actions.clone();
                let current_run_id = current_run_id.clone();
                let runtime = runtime.clone();
                let model_requests = model_requests.clone();
                async move {
                    model_requests.lock().await.push(request);
                    let scripted = scripted_actions.lock().await.pop_front().ok_or_else(|| {
                        ys_agent_core::CoreError::validation(
                            "model_script_exhausted",
                            "test model has no next action",
                        )
                    })?;
                    let run_id =
                        current_run_id
                            .lock()
                            .await
                            .as_ref()
                            .copied()
                            .ok_or_else(|| {
                                ys_agent_core::CoreError::validation(
                                    "fixture_run_missing",
                                    "test model was called before Run creation",
                                )
                            })?;
                    let snapshot = runtime.load_run(&run_id).await?;
                    let state = ys_agent_runtime::QueryWorkflowState::from_snapshot(
                        snapshot.workflow_state,
                    )?;
                    scripted.materialize(&state)
                }
            }
        }));

        let assembled = build_query_dependencies(
            workspace_id,
            principal,
            runtime.clone(),
            artifacts.clone(),
            model,
            telemetry.clone(),
        )
        .expect("assemble Query test dependencies");
        Self {
            _directory: directory,
            runtime,
            artifacts,
            service,
            session_id: session.id,
            driver: assembled.driver,
            current_run_id,
            tool_counts: assembled.tool_counts,
            transport_retries: std::sync::atomic::AtomicUsize::new(0),
            cached_primary: std::sync::Mutex::new(None),
            telemetry,
            model_requests,
        }
    }
}

use ys_agent_adapters::{
    ConnectorRegistry, InspectSchemaTool, MetricSqlDialect, QueryDataTool, ReadFreshnessTool,
    ResolveMetricTool, RuntimeArtifactLookup,
};
use ys_agent_core::{
    AllowedDataScope, CatalogReader, CellValue, ColumnPolicy, FreshnessObservation,
    FreshnessReader, MetricDefinition, MetricProvider, MetricStatus, ModelProvider, ObservedColumn,
    ObservedRelation, ObservedSchema, Principal, QueryBudget, QueryCostEstimate, QueryParameter,
    QueryPreflight, QueryPreflightDecision, QueryPreflightReader, QueryRequest, QueryResult,
    SchemaKnowledgeKind, Sensitivity, SqlQueryExecutor, Tool, ToolExecutionContext, ToolOutcome,
    WorkspaceId,
};
use ys_agent_runtime::{
    ContextAssembler, Harness, HarnessConfig, HarnessDependencies, InMemoryQueryContextProvider,
    FixedRunModelProviderResolver, LoopDriver, PromptBuilder,
    telemetry::TelemetryDispatcher,
    tools::{ConnectorToolAvailability, ToolCatalog, ToolRuntime, WorkspaceToolPolicy},
};

#[derive(Clone)]
struct FixtureMetrics {
    metric: MetricDefinition,
}

#[async_trait]
impl MetricProvider for FixtureMetrics {
    async fn get_metric(&self, query: &str) -> CoreResult<Option<MetricDefinition>> {
        let query = query.to_ascii_lowercase();
        Ok((query == self.metric.id || query.contains("gmv")).then(|| self.metric.clone()))
    }

    async fn list_active_metrics(&self) -> CoreResult<Vec<MetricDefinition>> {
        Ok(vec![self.metric.clone()])
    }
}

#[derive(Default)]
struct FixtureConnector;

#[async_trait]
impl CatalogReader for FixtureConnector {
    async fn observe_schema(&self, source_id: &SourceId) -> CoreResult<ObservedSchema> {
        Ok(ObservedSchema {
            source_id: source_id.clone(),
            kind: SchemaKnowledgeKind::Observed,
            relations: vec![ObservedRelation {
                name: "mart_orders".to_owned(),
                columns: vec![
                    ObservedColumn {
                        name: "paid_at".to_owned(),
                        data_type: "timestamp".to_owned(),
                        nullable: false,
                        primary_key_position: None,
                        sensitivity: Sensitivity::Internal,
                    },
                    ObservedColumn {
                        name: "channel".to_owned(),
                        data_type: "text".to_owned(),
                        nullable: false,
                        primary_key_position: None,
                        sensitivity: Sensitivity::Internal,
                    },
                    ObservedColumn {
                        name: "paid_amount".to_owned(),
                        data_type: "integer".to_owned(),
                        nullable: false,
                        primary_key_position: None,
                        sensitivity: Sensitivity::Internal,
                    },
                ],
            }],
            observed_at: Utc
                .with_ymd_and_hms(2026, 8, 15, 0, 0, 0)
                .single()
                .expect("valid observed time"),
        })
    }
}

#[async_trait]
impl QueryPreflightReader for FixtureConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision: QueryPreflightDecision::Allowed,
            cost: QueryCostEstimate {
                estimated_cost_units: Some(1),
                scanned_bytes: Some(128),
                estimator_version: Some("fixture-v1".to_owned()),
            },
            reason_codes: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

#[async_trait]
impl SqlQueryExecutor for FixtureConnector {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult> {
        let empty = request.parameters.iter().any(|parameter| {
            matches!(
                parameter,
                QueryParameter::Timestamp(value) if value.year() == 1990
            )
        });
        let (columns, rows) = if empty {
            (vec!["metric_value".to_owned()], Vec::new())
        } else if request.sql.to_ascii_lowercase().contains("channel") {
            (
                vec!["channel".to_owned()],
                vec![
                    vec![CellValue::Text("store".to_owned())],
                    vec![CellValue::Text("web".to_owned())],
                ],
            )
        } else {
            (
                vec!["metric_value".to_owned()],
                vec![vec![CellValue::Integer(10)]],
            )
        };
        let warning_codes = if empty {
            vec!["empty_result".to_owned()]
        } else {
            Vec::new()
        };
        let model_preview = serde_json::to_string(&json!({
            "columns": &columns,
            "rows": &rows,
        }))
        .map_err(|error| CoreError::validation("fixture_preview_failed", error.to_string()))?;
        Ok(QueryResult {
            row_count: rows.len(),
            serialized_bytes: model_preview.len(),
            columns,
            rows,
            truncated: false,
            remote_query_id: None,
            warning_codes,
            model_preview,
        })
    }
}

#[async_trait]
impl FreshnessReader for FixtureConnector {
    async fn read_freshness(
        &self,
        source_id: &SourceId,
        relation: &str,
        _time_column: &str,
    ) -> CoreResult<FreshnessObservation> {
        let observed_at = Utc
            .with_ymd_and_hms(2026, 8, 15, 0, 0, 0)
            .single()
            .expect("valid observed time");
        Ok(FreshnessObservation {
            source_id: source_id.clone(),
            relation: relation.to_owned(),
            observed_at,
            data_as_of: Some(observed_at - chrono::TimeDelta::minutes(5)),
            lag_seconds: Some(300),
        })
    }
}

struct CountingTool {
    inner: Arc<dyn Tool>,
    calls: Arc<StdMutex<BTreeMap<String, usize>>>,
}

#[async_trait]
impl Tool for CountingTool {
    fn spec(&self) -> ys_agent_core::ToolSpec {
        self.inner.spec()
    }

    async fn execute(
        &self,
        context: &ToolExecutionContext,
        arguments: serde_json::Value,
    ) -> CoreResult<ToolOutcome> {
        let name = self.inner.spec().name;
        *self.calls.lock().unwrap().entry(name).or_default() += 1;
        self.inner.execute(context, arguments).await
    }
}

struct AssembledQueryDependencies {
    driver: LoopDriver,
    tool_counts: Arc<StdMutex<BTreeMap<String, usize>>>,
}

fn build_query_dependencies(
    workspace_id: WorkspaceId,
    principal: Principal,
    runtime: Arc<ys_agent_store::SqliteRuntimeStore>,
    artifacts: Arc<ys_agent_store::LocalArtifactStore>,
    model: Arc<dyn ModelProvider>,
    telemetry: Arc<TelemetryDispatcher>,
) -> CoreResult<AssembledQueryDependencies> {
    let source_id = SourceId::new("sqlite-demo");
    let metric = MetricDefinition {
        id: "commerce.gmv".to_owned(),
        version: "1".to_owned(),
        status: MetricStatus::Active,
        description: "Paid order value".to_owned(),
        source_relation: "mart_orders".to_owned(),
        expression: "SUM(paid_amount)".to_owned(),
        time_column: "paid_at".to_owned(),
        allowed_dimensions: vec!["channel".to_owned()],
        owner: "data-team".to_owned(),
        freshness_sla_seconds: Some(3_600),
    };
    let metrics: Arc<dyn MetricProvider> = Arc::new(FixtureMetrics { metric });
    let connector = Arc::new(FixtureConnector);
    let mut connectors = ConnectorRegistry::new();
    connectors.register(source_id.clone(), MetricSqlDialect::Sqlite, connector)?;

    let artifact_store: Arc<dyn ArtifactStore> = artifacts.clone();
    let runtime_store: Arc<dyn RuntimeStore> = runtime.clone();
    let artifact_lookup = Arc::new(RuntimeArtifactLookup::new(
        runtime_store.clone(),
        artifact_store.clone(),
    ));
    let tools: Vec<Arc<dyn Tool>> = vec![
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
    let tool_counts = Arc::new(StdMutex::new(BTreeMap::new()));
    let mut catalog = ToolCatalog::with_policy(tool_policy.clone());
    for tool in tools {
        catalog.register_arc(Arc::new(CountingTool {
            inner: tool,
            calls: tool_counts.clone(),
        }))?;
    }
    let catalog = Arc::new(catalog);

    let dbt_context = Arc::new(InMemoryQueryContextProvider::new());
    let run_context = Arc::new(InMemoryQueryContextProvider::new());
    let context_assembler = Arc::new(ContextAssembler::new(metrics, dbt_context, run_context));

    let mut columns = BTreeMap::new();
    columns.insert("paid_at".to_owned(), ColumnPolicy::Allow);
    columns.insert("channel".to_owned(), ColumnPolicy::Allow);
    columns.insert("paid_amount".to_owned(), ColumnPolicy::Allow);
    let mut relations = BTreeMap::new();
    relations.insert("mart_orders".to_owned(), columns);

    let harness = Arc::new(Harness::new(
        HarnessDependencies {
            store: runtime_store.clone(),
            artifacts: artifact_store,
            model_resolver: Arc::new(FixedRunModelProviderResolver::new(
                Arc::new(runtime.run_binding_repository()),
                model,
            )),
            catalog,
            tool_runtime: Arc::new(ToolRuntime::with_max_same_call_retries(1)),
            context_assembler,
            telemetry,
        },
        PromptBuilder::new(),
        HarnessConfig {
            workspace_id,
            principal,
            workspace_timezone: "UTC".to_owned(),
            query_budget: QueryBudget::default(),
            data_scope: AllowedDataScope {
                workspace_id,
                source_id: source_id.as_str().to_owned(),
                relations,
            },
            connector_tools: ConnectorToolAvailability::all_query_tools(),
            tool_policy,
            context_token_budget: 8_000,
            schema_ttl: Duration::from_secs(300),
        },
    ));

    Ok(AssembledQueryDependencies {
        driver: LoopDriver::with_defaults(harness),
        tool_counts,
    })
}

impl QueryWorkflowFixture {
    pub async fn successful_metric_query() -> Self {
        Self::with_model_actions(metric_success_script()).await
    }

    pub async fn successful_metric_query_with_telemetry(
        telemetry: Arc<TelemetryDispatcher>,
    ) -> Self {
        Self::with_model_actions_and_telemetry(metric_success_script(), telemetry).await
    }

    pub async fn with_ambiguous_metrics() -> Self {
        Self::with_model_actions(Vec::new()).await
    }

    pub async fn metadata_query() -> Self {
        Self::with_model_actions(metadata_success_script()).await
    }

    pub async fn empty_metric_result() -> Self {
        Self::with_model_actions(empty_metric_script()).await
    }

    pub async fn run(&self, question: &str) -> ys_agent_core::CoreResult<TestRunResult> {
        let reply = ys_agent_runtime::AgentServiceApi::send_message(
            self.service.as_ref(),
            ys_agent_runtime::SendMessageRequest::new(
                ys_agent_core::CommandId::new(),
                self.session_id,
                question,
            ),
        )
        .await?;
        let run_id = reply.run_id().ok_or_else(|| {
            ys_agent_core::CoreError::validation(
                "fixture_query_not_scheduled",
                "test question did not create a Query Run",
            )
        })?;
        *self.current_run_id.lock().await = Some(run_id);

        let loop_result = self.driver.run(&run_id).await?;
        let result = TestRunResult::load(&self.runtime, loop_result.snapshot).await?;
        let events = self.runtime.load_events(&run_id, 0).await?;
        let proposed_calls = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.event.kind,
                    ys_agent_core::RunEventKind::ToolCallProposed { .. }
                )
            })
            .count();
        let execution_attempts = self
            .tool_counts
            .lock()
            .unwrap()
            .values()
            .copied()
            .sum::<usize>();
        self.transport_retries.store(
            execution_attempts.saturating_sub(proposed_calls),
            std::sync::atomic::Ordering::SeqCst,
        );
        if result.snapshot.primary_artifact_id.is_some() {
            let artifact = self.load_primary_query_artifact(&result).await;
            *self.cached_primary.lock().unwrap() = Some(artifact);
        }
        Ok(result)
    }

    pub async fn load_primary_query_artifact(
        &self,
        result: &TestRunResult,
    ) -> ys_agent_runtime::QueryArtifact {
        let artifact_id = result
            .snapshot
            .primary_artifact_id
            .expect("terminal result has a primary Artifact");
        let metadata = self
            .runtime
            .load_artifact(&artifact_id)
            .await
            .expect("registered primary Artifact");
        let task = self
            .runtime
            .load_task(&result.snapshot.task_id)
            .await
            .expect("load owning Task");
        let bytes = self
            .artifacts
            .get(
                &ys_agent_core::ArtifactRef::new(metadata),
                &ys_agent_core::ArtifactAccessContext {
                    workspace_id: task.workspace_id,
                    principal_id: task.created_by,
                    purpose: ys_agent_core::ArtifactAccessPurpose::RuntimeVerification,
                    max_sensitivity: ys_agent_core::Sensitivity::Restricted,
                },
            )
            .await
            .expect("read primary Artifact body");
        serde_json::from_slice(&bytes).expect("decode QueryArtifact")
    }

    pub fn primary_artifact(&self) -> ys_agent_runtime::QueryArtifact {
        self.cached_primary
            .lock()
            .unwrap()
            .clone()
            .expect("run cached its primary QueryArtifact")
    }

    pub fn tool_call_count(&self, name: &str) -> usize {
        self.tool_counts
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    pub fn transport_retry_count(&self) -> usize {
        self.transport_retries
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn telemetry_failure_count(&self) -> u64 {
        self.telemetry.failure_count()
    }

    pub async fn model_requests(&self) -> Vec<ys_agent_core::ModelRequest> {
        self.model_requests.lock().await.clone()
    }

    pub async fn has_run_completed_event(&self, run_id: &ys_agent_core::RunId) -> bool {
        self.runtime
            .load_events(run_id, 0)
            .await
            .expect("load workflow events")
            .iter()
            .any(|event| {
                matches!(
                    event.event.kind,
                    ys_agent_core::RunEventKind::RunCompleted { .. }
                )
            })
    }
}

pub struct TestRunResult {
    #[allow(dead_code)]
    pub run_id: ys_agent_core::RunId,
    pub status: ys_agent_core::RunStatus,
    snapshot: ys_agent_core::RunSnapshot,
    failure_code: Option<String>,
    pending_reason: Option<String>,
}

impl TestRunResult {
    async fn load(
        runtime: &ys_agent_store::SqliteRuntimeStore,
        snapshot: ys_agent_core::RunSnapshot,
    ) -> ys_agent_core::CoreResult<Self> {
        let events = runtime.load_events(&snapshot.run_id, 0).await?;
        let failure_code = events.iter().rev().find_map(|event| {
            if let ys_agent_core::RunEventKind::RunFailed { code, .. } = &event.event.kind {
                Some(code.clone())
            } else {
                None
            }
        });
        let pending_reason = snapshot
            .pending_wait_metadata
            .as_ref()
            .and_then(|value| value.get("reason"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            run_id: snapshot.run_id,
            status: snapshot.status,
            snapshot,
            failure_code,
            pending_reason,
        })
    }

    pub fn failure_code(&self) -> Option<&str> {
        self.failure_code.as_deref()
    }

    pub fn pending_reason(&self) -> &str {
        self.pending_reason.as_deref().unwrap_or("")
    }
}

#[allow(dead_code)]
struct RuntimeComponents {
    runtime: Arc<ys_agent_store::SqliteRuntimeStore>,
    service: Arc<ys_agent_runtime::InProcessAgentService>,
    recovery: Arc<ys_agent_runtime::RecoveryManager>,
    driver: ys_agent_runtime::LoopDriver,
    tool_counts: Arc<StdMutex<BTreeMap<String, usize>>>,
}

#[allow(dead_code)]
pub struct PersistentRuntimeFixture {
    _root: tempfile::TempDir,
    database_path: std::path::PathBuf,
    artifact_root: std::path::PathBuf,
    workspace_id: ys_agent_core::WorkspaceId,
    principal: ys_agent_core::Principal,
    actions: Arc<Mutex<VecDeque<ScriptedAction>>>,
    current_run_id: Arc<Mutex<Option<ys_agent_core::RunId>>>,
    live: Option<RuntimeComponents>,
    pub run_id: ys_agent_core::RunId,
    pub task_id: ys_agent_core::TaskId,
    pub failed_run_id: ys_agent_core::RunId,
    original_call_id: Option<ys_agent_core::ToolCallId>,
}

#[allow(dead_code)]
impl PersistentRuntimeFixture {
    fn live(&self) -> &RuntimeComponents {
        self.live.as_ref().expect("Runtime fixture is open")
    }

    pub fn runtime(&self) -> &ys_agent_store::SqliteRuntimeStore {
        self.live().runtime.as_ref()
    }

    pub fn service(&self) -> &ys_agent_runtime::InProcessAgentService {
        self.live().service.as_ref()
    }

    pub fn recovery(&self) -> &ys_agent_runtime::RecoveryManager {
        self.live().recovery.as_ref()
    }

    pub fn original_tool_call_id(&self) -> ys_agent_core::ToolCallId {
        self.original_call_id
            .expect("fixture has an interrupted Tool call")
    }

    pub fn close_runtime(&mut self) {
        let old = self.live.take().expect("Runtime fixture is open");
        drop(old);
    }

    pub async fn reopen(&self) -> ys_agent_core::CoreResult<ReopenedRuntime> {
        if self.live.is_some() {
            return Err(ys_agent_core::CoreError::validation(
                "fixture_not_closed",
                "close_runtime must drop old handles before reopen",
            ));
        }
        let components = open_runtime_components(
            &self.database_path,
            &self.artifact_root,
            self.workspace_id,
            self.principal.clone(),
            self.actions.clone(),
            self.current_run_id.clone(),
        )
        .await?;
        Ok(ReopenedRuntime {
            runtime: components.runtime,
            service: components.service,
            recovery: components.recovery,
            driver: components.driver,
            tool_counts: components.tool_counts,
            current_run_id: self.current_run_id.clone(),
            run_id: self.run_id,
            task_id: self.task_id,
        })
    }

    async fn with_actions(actions: Vec<ScriptedAction>) -> Self {
        let root = tempfile::tempdir().expect("persistent fixture root");
        let database_path = root.path().join("runtime.db");
        let artifact_root = root.path().join("artifacts");
        let workspace_id = ys_agent_core::WorkspaceId::new();
        let principal = ys_agent_core::Principal::local_operator("recovery-tutorial");
        let actions = Arc::new(Mutex::new(VecDeque::from(actions)));
        let current_run_id = Arc::new(Mutex::new(None));
        let live = open_runtime_components(
            &database_path,
            &artifact_root,
            workspace_id,
            principal.clone(),
            actions.clone(),
            current_run_id.clone(),
        )
        .await
        .expect("open persistent Runtime");
        Self {
            _root: root,
            database_path,
            artifact_root,
            workspace_id,
            principal,
            actions,
            current_run_id,
            live: Some(live),
            run_id: ys_agent_core::RunId::new(),
            task_id: ys_agent_core::TaskId::new(),
            failed_run_id: ys_agent_core::RunId::new(),
            original_call_id: None,
        }
    }

    pub async fn new() -> Self {
        Self::with_actions(metric_success_script()).await
    }

    async fn create_run(&mut self, question: &str) -> ys_agent_core::RunSnapshot {
        let session = ys_agent_runtime::AgentServiceApi::create_session(
            self.service(),
            ys_agent_core::CommandId::new(),
            self.principal.clone(),
        )
        .await
        .expect("create recovery Session");
        let reply = ys_agent_runtime::AgentServiceApi::send_message(
            self.service(),
            ys_agent_runtime::SendMessageRequest::new(
                ys_agent_core::CommandId::new(),
                session.id,
                question,
            ),
        )
        .await
        .expect("schedule recovery Run");
        let run_id = reply.run_id().expect("Query Run ID");
        let snapshot = self
            .runtime()
            .load_run(&run_id)
            .await
            .expect("load created Run");
        self.run_id = run_id;
        self.task_id = snapshot.task_id;
        *self.current_run_id.lock().await = Some(run_id);
        snapshot
    }

    pub async fn run_until_clarification(&mut self, question: &str) -> TestRunResult {
        let snapshot = self.create_run(question).await;
        let loop_result = self
            .live()
            .driver
            .run(&snapshot.run_id)
            .await
            .expect("run to clarification");
        TestRunResult::load(self.runtime(), loop_result.snapshot)
            .await
            .expect("load waiting Run")
    }

    async fn seed_started_tool(
        &mut self,
        call: ys_agent_core::ToolCall,
        phase: ys_agent_runtime::tools::QueryPhase,
        cost_class: ys_agent_core::CostClass,
    ) {
        let current = self
            .runtime()
            .load_run(&self.run_id)
            .await
            .expect("load crash Run");
        let mut state =
            ys_agent_runtime::QueryWorkflowState::from_snapshot(current.workflow_state.clone())
                .expect("decode Query state");
        state.phase = phase;
        state.intent = Some(ys_agent_core::QueryIntent::GovernedMetric);
        let next = ys_agent_core::RunSnapshot {
            version: current.version + 1,
            workflow_state: state.to_snapshot().expect("serialize crash state"),
            ..current.clone()
        };
        let events = vec![
            recovery_test_event(ys_agent_core::RunEventKind::ToolCallProposed {
                call: call.clone(),
            }),
            recovery_test_event(ys_agent_core::RunEventKind::PolicyEvaluated {
                call_id: call.id,
                decision: ys_agent_core::PolicyDecision::Allow,
            }),
            ys_agent_core::PendingRunEvent {
                actor: ys_agent_core::EventActor::Tool {
                    name: call.name.clone(),
                },
                kind: ys_agent_core::RunEventKind::ToolExecutionStarted {
                    call_id: call.id,
                    cost_class,
                },
            },
        ];
        self.runtime()
            .append(&self.run_id, current.version, vec![], events, &next)
            .await
            .expect("persist interrupted Tool boundary");
        self.original_call_id = Some(call.id);
    }

    pub async fn crash_after_low_cost_tool_started() -> Self {
        let mut fixture = Self::with_actions(metric_after_context_script()).await;
        fixture.create_run("GMV for seven complete days").await;
        let call = ys_agent_core::ToolCall {
            id: ys_agent_core::ToolCallId::new(),
            provider_call_id: None,
            name: "resolve_metric".to_owned(),
            arguments: json!({ "metric": "commerce.gmv" }),
            version: "1.0.0".to_owned(),
        };
        fixture
            .seed_started_tool(
                call,
                ys_agent_runtime::tools::QueryPhase::ResolveContext,
                ys_agent_core::CostClass::Low,
            )
            .await;
        fixture
    }

    pub async fn crash_after_high_cost_tool_started() -> Self {
        let mut fixture = Self::with_actions(Vec::new()).await;
        fixture.create_run("GMV for seven complete days").await;
        let call = ys_agent_core::ToolCall {
            id: ys_agent_core::ToolCallId::new(),
            provider_call_id: None,
            name: "query_data".to_owned(),
            arguments: json!({
                "action": "execute",
                "plan_artifact_id": ys_agent_core::ArtifactId::new(),
                "plan_hash": "sha256:interrupted-plan",
                "preflight_artifact_id": ys_agent_core::ArtifactId::new(),
                "preflight_hash": "sha256:interrupted-preflight",
            }),
            version: "1.0.0".to_owned(),
        };
        fixture
            .seed_started_tool(
                call,
                ys_agent_runtime::tools::QueryPhase::Execute,
                ys_agent_core::CostClass::High,
            )
            .await;
        fixture
    }

    pub async fn failed_run() -> Self {
        let mut fixture = Self::with_actions(Vec::new()).await;
        let current = fixture.create_run("GMV").await;
        let failed = ys_agent_core::RunSnapshot {
            status: ys_agent_core::RunStatus::Failed,
            version: current.version + 1,
            ..current.clone()
        };
        fixture
            .runtime()
            .append(
                &current.run_id,
                current.version,
                vec![],
                vec![recovery_test_event(
                    ys_agent_core::RunEventKind::RunFailed {
                        code: "fixture_failure".to_owned(),
                        message: "seeded failure".to_owned(),
                    },
                )],
                &failed,
            )
            .await
            .expect("persist failed Run");
        fixture.failed_run_id = current.run_id;
        fixture
    }

    pub async fn with_event_sequence_gap() -> Self {
        let mut fixture = Self::with_actions(Vec::new()).await;
        fixture.create_run("GMV").await;
        let connection = rusqlite::Connection::open(&fixture.database_path)
            .expect("open copied recovery database");
        let changed = connection
            .execute(
                "UPDATE run_events
                 SET sequence = sequence + 1
                 WHERE run_id = ?1
                   AND sequence = (
                       SELECT MAX(sequence) FROM run_events WHERE run_id = ?1
                   )",
                [fixture.run_id.to_string()],
            )
            .expect("create Event sequence gap");
        assert_eq!(changed, 1);
        fixture
    }
}

#[allow(dead_code)]
pub struct ReopenedRuntime {
    pub runtime: Arc<ys_agent_store::SqliteRuntimeStore>,
    pub service: Arc<ys_agent_runtime::InProcessAgentService>,
    pub recovery: Arc<ys_agent_runtime::RecoveryManager>,
    driver: ys_agent_runtime::LoopDriver,
    tool_counts: Arc<StdMutex<BTreeMap<String, usize>>>,
    current_run_id: Arc<Mutex<Option<ys_agent_core::RunId>>>,
    run_id: ys_agent_core::RunId,
    task_id: ys_agent_core::TaskId,
}

#[allow(dead_code)]
impl ReopenedRuntime {
    pub async fn run_to_terminal(&self, run_id: &ys_agent_core::RunId) -> TestRunResult {
        *self.current_run_id.lock().await = Some(*run_id);
        let result = self.driver.run(run_id).await.expect("run after reopen");
        TestRunResult::load(&self.runtime, result.snapshot)
            .await
            .expect("load terminal Run")
    }

    pub async fn resume(
        &self,
        command_id: ys_agent_core::CommandId,
    ) -> ys_agent_core::CoreResult<TestRunResult> {
        let run_id = ys_agent_runtime::AgentServiceApi::resume_task(
            self.service.as_ref(),
            command_id,
            &self.task_id,
        )
        .await?;
        *self.current_run_id.lock().await = Some(run_id);
        let snapshot = self.runtime.load_run(&run_id).await?;
        TestRunResult::load(&self.runtime, snapshot).await
    }

    pub async fn resume_to_terminal(&self, command_id: ys_agent_core::CommandId) -> TestRunResult {
        let resumed = self.resume(command_id).await.expect("resume Run");
        if resumed.status == ys_agent_core::RunStatus::Running {
            self.run_to_terminal(&resumed.run_id).await
        } else {
            resumed
        }
    }

    pub async fn has_indeterminate_event(&self, call_id: &ys_agent_core::ToolCallId) -> bool {
        self.runtime
            .load_events(&self.run_id, 0)
            .await
            .expect("load recovery Events")
            .iter()
            .any(|event| {
                matches!(
                    &event.event.kind,
                    ys_agent_core::RunEventKind::ToolExecutionIndeterminate {
                        call_id: observed,
                        ..
                    } if observed == call_id
                )
            })
    }

    pub async fn successful_tool_call_id(&self) -> ys_agent_core::ToolCallId {
        self.runtime
            .load_events(&self.run_id, 0)
            .await
            .expect("load recovery Events")
            .iter()
            .rev()
            .find_map(|event| {
                if let ys_agent_core::RunEventKind::ToolExecutionSucceeded { call_id, .. } =
                    &event.event.kind
                {
                    Some(*call_id)
                } else {
                    None
                }
            })
            .expect("a recovered Tool call succeeded")
    }

    pub fn tool_execution_count(&self) -> usize {
        self.tool_counts.lock().unwrap().values().copied().sum()
    }
}

#[allow(dead_code)]
fn recovery_test_event(kind: ys_agent_core::RunEventKind) -> ys_agent_core::PendingRunEvent {
    ys_agent_core::PendingRunEvent {
        actor: ys_agent_core::EventActor::System,
        kind,
    }
}

#[allow(dead_code)]
fn metric_after_context_script() -> Vec<ScriptedAction> {
    vec![
        metric_plan_response(
            Utc.with_ymd_and_hms(2026, 8, 7, 0, 0, 0)
                .single()
                .expect("valid start"),
            Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0)
                .single()
                .expect("valid end"),
        ),
        call_query_data_preflight(),
        call_query_data_execute(),
        call_read_freshness(),
        completion_response("GMV is 10 for the requested complete period."),
    ]
}

#[allow(dead_code)]
async fn open_runtime_components(
    database_path: &std::path::Path,
    artifact_root: &std::path::Path,
    workspace_id: ys_agent_core::WorkspaceId,
    principal: ys_agent_core::Principal,
    actions: Arc<Mutex<VecDeque<ScriptedAction>>>,
    current_run_id: Arc<Mutex<Option<ys_agent_core::RunId>>>,
) -> ys_agent_core::CoreResult<RuntimeComponents> {
    let runtime =
        Arc::new(ys_agent_store::SqliteRuntimeStore::open(database_path.to_path_buf()).await?);
    let active_provider = provider_fixture::persisted_test_active_provider(runtime.as_ref()).await;
    let artifacts = Arc::new(ys_agent_store::LocalArtifactStore::new(artifact_root)?);
    let model: Arc<dyn ys_agent_core::ModelProvider> =
        Arc::new(ys_agent_adapters::model::FakeModelProvider::new({
            let actions = actions.clone();
            let current_run_id = current_run_id.clone();
            let runtime = runtime.clone();
            move |_request| {
                let actions = actions.clone();
                let current_run_id = current_run_id.clone();
                let runtime = runtime.clone();
                async move {
                    let scripted = actions.lock().await.pop_front().ok_or_else(|| {
                        ys_agent_core::CoreError::validation(
                            "model_script_exhausted",
                            "recovery fixture has no next model action",
                        )
                    })?;
                    let run_id =
                        current_run_id
                            .lock()
                            .await
                            .as_ref()
                            .copied()
                            .ok_or_else(|| {
                                ys_agent_core::CoreError::validation(
                                    "fixture_run_missing",
                                    "recovery model needs a current Run ID",
                                )
                            })?;
                    let snapshot = runtime.load_run(&run_id).await?;
                    let state = ys_agent_runtime::QueryWorkflowState::from_snapshot(
                        snapshot.workflow_state,
                    )?;
                    scripted.materialize(&state)
                }
            }
        }));
    let assembled = build_query_dependencies(
        workspace_id,
        principal.clone(),
        runtime.clone(),
        artifacts.clone(),
        model,
        Arc::new(TelemetryDispatcher::default()),
    )?;
    let service = Arc::new(
        ys_agent_runtime::InProcessAgentService::new(
            workspace_id,
            runtime.clone(),
            artifacts,
            Arc::new(ys_agent_runtime::NoopRunScheduler),
        )
        .with_run_provider_binding_source(Arc::new(
            ys_agent_runtime::StaticRunProviderBindingSource::from_active(active_provider),
        )),
    );
    let recovery = Arc::new(ys_agent_runtime::RecoveryManager::new(runtime.clone()));
    Ok(RuntimeComponents {
        runtime,
        service,
        recovery,
        driver: assembled.driver,
        tool_counts: assembled.tool_counts,
    })
}
