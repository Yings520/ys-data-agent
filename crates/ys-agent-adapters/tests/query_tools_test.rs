use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use ys_agent_adapters::{
    ArtifactLookup, ArtifactRecord, ConnectorRegistry, InspectSchemaTool, MetricSqlDialect,
    QueryDataTool, ReadFreshnessTool, ResolveMetricTool,
};
use ys_agent_core::{
    AllowedDataScope, ArtifactAccessContext, ArtifactId, ArtifactKind, ArtifactMetadata,
    ArtifactRef, ArtifactStore, CatalogReader, CellValue, ColumnPolicy, CoreError, CoreResult,
    FreshnessObservation, FreshnessReader, MetricDefinition, MetricProvider, MetricStatus,
    ObservedColumn, ObservedRelation, ObservedSchema, Principal, PutArtifact, QueryBudget,
    QueryCostEstimate, QueryExecutionPlan, QueryParameter, QueryPlan, QueryPreflight,
    QueryPreflightDecision, QueryPreflightReader, QueryRequest, QueryResult, RetentionPolicy,
    RunId, SchemaKnowledgeKind, Sensitivity, SourceId, SqlQueryExecutor, TaskId, Tool, ToolCallId,
    ToolExecutionContext, ToolOutcome, WorkspaceId,
};

#[derive(Clone)]
struct FakeMetrics {
    metric: MetricDefinition,
}

#[async_trait]
impl MetricProvider for FakeMetrics {
    async fn get_metric(&self, metric_id: &str) -> CoreResult<Option<MetricDefinition>> {
        Ok((metric_id == self.metric.id).then(|| self.metric.clone()))
    }

    async fn list_active_metrics(&self) -> CoreResult<Vec<MetricDefinition>> {
        Ok(vec![self.metric.clone()])
    }
}

#[derive(Default)]
struct FakeConnector {
    requests: Mutex<Vec<QueryRequest>>,
}

#[async_trait]
impl CatalogReader for FakeConnector {
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
                        data_type: "numeric".to_owned(),
                        nullable: false,
                        primary_key_position: None,
                        sensitivity: Sensitivity::Internal,
                    },
                ],
            }],
            observed_at: Utc.with_ymd_and_hms(2026, 8, 8, 1, 0, 0).unwrap(),
        })
    }
}

#[async_trait]
impl QueryPreflightReader for FakeConnector {
    async fn preflight(&self, request: &QueryRequest) -> CoreResult<QueryPreflight> {
        Ok(QueryPreflight {
            sql: request.sql.clone(),
            decision: QueryPreflightDecision::Allowed,
            cost: QueryCostEstimate {
                estimated_cost_units: Some(1),
                scanned_bytes: Some(128),
                estimator_version: Some("fake-v1".to_owned()),
            },
            reason_codes: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

#[async_trait]
impl SqlQueryExecutor for FakeConnector {
    async fn execute_query(&self, request: QueryRequest) -> CoreResult<QueryResult> {
        self.requests.lock().await.push(request);
        Ok(QueryResult {
            columns: vec!["metric_value".to_owned()],
            rows: vec![vec![CellValue::Integer(42)]],
            truncated: false,
            remote_query_id: None,
            row_count: 1,
            serialized_bytes: 64,
            warning_codes: Vec::new(),
            model_preview: r#"{"columns":["metric_value"],"rows":[[42]]}"#.to_owned(),
        })
    }
}

#[async_trait]
impl FreshnessReader for FakeConnector {
    async fn read_freshness(
        &self,
        source_id: &SourceId,
        relation: &str,
        _time_column: &str,
    ) -> CoreResult<FreshnessObservation> {
        let observed_at = Utc.with_ymd_and_hms(2026, 8, 8, 1, 0, 0).unwrap();
        let data_as_of = Utc.with_ymd_and_hms(2026, 8, 8, 0, 55, 0).unwrap();
        Ok(FreshnessObservation {
            source_id: source_id.clone(),
            relation: relation.to_owned(),
            observed_at,
            data_as_of: Some(data_as_of),
            lag_seconds: Some(300),
        })
    }
}

#[derive(Default)]
struct MemoryArtifacts {
    records: Mutex<HashMap<ArtifactId, ArtifactRecord>>,
}

#[async_trait]
impl ArtifactStore for MemoryArtifacts {
    async fn put(&self, request: PutArtifact) -> CoreResult<ArtifactMetadata> {
        use sha2::{Digest, Sha256};

        let hash = format!(
            "sha256:{}",
            Sha256::digest(&request.bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let mut builder = ArtifactMetadata::builder(request.sensitivity)
            .workspace_id(request.workspace_id)
            .task_id(request.task_id)
            .run_id(request.run_id)
            .kind(request.kind)
            .media_type(request.media_type)
            .content_hash(hash)
            .size_bytes(request.bytes.len() as u64)
            .storage_uri("memory://query-tool-test");
        if let Some(owner) = request.owner {
            builder = builder.owner(owner);
        }
        if let Some(policy) = request.retention_policy {
            builder = builder.retention_policy(policy);
        }
        if let Some(expires_at) = request.expires_at {
            builder = builder.expires_at(expires_at);
        }
        if let Some(step_id) = request.producer_step_id {
            builder = builder.producer_step_id(step_id);
        }
        let metadata = builder.build()?;
        self.records.lock().await.insert(
            metadata.id,
            ArtifactRecord {
                metadata: metadata.clone(),
                bytes: request.bytes,
            },
        );
        Ok(metadata)
    }

    async fn get(
        &self,
        artifact: &ArtifactRef,
        _access: &ArtifactAccessContext,
    ) -> CoreResult<Vec<u8>> {
        self.records
            .lock()
            .await
            .get(&artifact.id())
            .map(|record| record.bytes.clone())
            .ok_or_else(|| CoreError::NotFound {
                entity: "artifact",
                id: artifact.id().to_string(),
            })
    }
}

#[async_trait]
impl ArtifactLookup for MemoryArtifacts {
    async fn load(
        &self,
        artifact_id: &ArtifactId,
        _access: &ArtifactAccessContext,
    ) -> CoreResult<ArtifactRecord> {
        self.records
            .lock()
            .await
            .get(artifact_id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound {
                entity: "artifact",
                id: artifact_id.to_string(),
            })
    }
}

struct QueryToolFixture {
    tools: HashMap<String, Arc<dyn Tool>>,
    calls: Mutex<HashMap<String, usize>>,
    artifacts: Arc<MemoryArtifacts>,
    context: ToolExecutionContext,
}

impl QueryToolFixture {
    async fn sqlite() -> Self {
        let workspace_id = WorkspaceId::new();
        let principal = Principal::local_operator("tutorial");
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
        let metrics: Arc<dyn MetricProvider> = Arc::new(FakeMetrics { metric });
        let connector = Arc::new(FakeConnector::default());
        let mut connectors = ConnectorRegistry::new();
        connectors
            .register(source_id.clone(), MetricSqlDialect::Sqlite, connector)
            .expect("register fake connector");

        let artifacts = Arc::new(MemoryArtifacts::default());
        let artifact_store: Arc<dyn ArtifactStore> = artifacts.clone();
        let artifact_lookup: Arc<dyn ArtifactLookup> = artifacts.clone();
        let mut tools = HashMap::<String, Arc<dyn Tool>>::new();
        let resolve = Arc::new(ResolveMetricTool::new(metrics.clone()));
        let inspect = Arc::new(InspectSchemaTool::new(
            connectors.clone(),
            artifact_store.clone(),
            20,
            200,
            8_192,
        ));
        let freshness = Arc::new(ReadFreshnessTool::new(connectors.clone(), metrics.clone()));
        let query = Arc::new(QueryDataTool::new(
            connectors,
            metrics,
            artifact_lookup,
            artifact_store,
        ));
        tools.insert("resolve_metric".to_owned(), resolve);
        tools.insert("inspect_schema".to_owned(), inspect);
        tools.insert("read_freshness".to_owned(), freshness);
        tools.insert("query_data".to_owned(), query);

        let mut columns = BTreeMap::new();
        columns.insert("paid_at".to_owned(), ColumnPolicy::Allow);
        columns.insert("channel".to_owned(), ColumnPolicy::Allow);
        columns.insert("paid_amount".to_owned(), ColumnPolicy::Allow);
        let mut relations = BTreeMap::new();
        relations.insert("mart_orders".to_owned(), columns);

        Self {
            tools,
            calls: Mutex::new(HashMap::new()),
            artifacts,
            context: ToolExecutionContext {
                call_id: ToolCallId::new(),
                workspace_id,
                task_id: TaskId::new(),
                run_id: RunId::new(),
                principal,
                query_budget: QueryBudget::default(),
                data_scope: AllowedDataScope {
                    workspace_id,
                    source_id: source_id.as_str().to_owned(),
                    relations,
                },
                confirmation_granted: false,
            },
        }
    }

    async fn call(&self, name: &str, arguments: Value) -> ToolOutcome {
        *self.calls.lock().await.entry(name.to_owned()).or_default() += 1;
        self.tools[name]
            .execute(&self.context, arguments)
            .await
            .expect("tool returns a typed outcome")
    }

    async fn call_count(&self, name: &str) -> usize {
        self.calls.lock().await.get(name).copied().unwrap_or(0)
    }

    async fn persist_plan(&self, plan: QueryPlan) -> ArtifactMetadata {
        let bytes = serde_json::to_vec(&plan).expect("serialize plan");
        self.artifacts
            .put(PutArtifact {
                workspace_id: self.context.workspace_id,
                task_id: self.context.task_id,
                run_id: self.context.run_id,
                kind: ArtifactKind::QueryPlan,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            })
            .await
            .expect("persist plan")
    }
}

#[tokio::test]
async fn resolve_metric_returns_only_the_active_contract() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture
        .call("resolve_metric", json!({ "metric": "commerce.gmv" }))
        .await;

    let output = outcome.success_json().expect("success");
    assert_eq!(output["status"], "active");
    assert_eq!(output["source_relation"], "mart_orders");
}

#[tokio::test]
async fn metric_query_is_compiled_from_the_contract_not_free_form_sql() {
    let fixture = QueryToolFixture::sqlite().await;
    let plan = fixture
        .persist_plan(QueryPlan {
            source_id: SourceId::new("sqlite-demo"),
            execution: QueryExecutionPlan::Metric {
                metric_id: "commerce.gmv".to_owned(),
                start: Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 8, 8, 0, 0, 0).unwrap(),
                dimensions: Vec::new(),
            },
        })
        .await;
    let preflight = fixture
        .call(
            "query_data",
            json!({
                "action": "preflight",
                "plan_artifact_id": plan.id,
                "plan_hash": plan.content_hash,
            }),
        )
        .await;
    let preflight_output = preflight.success_json().expect("preflight success");
    let outcome = fixture
        .call(
            "query_data",
            json!({
                "action": "execute",
                "plan_artifact_id": plan.id,
                "plan_hash": plan.content_hash,
                "preflight_artifact_id": preflight_output["artifact_id"],
                "preflight_hash": preflight_output["artifact_hash"],
            }),
        )
        .await;

    let output = outcome.success_json().expect("execute success");
    assert_eq!(output["semantic_status"], "confirmed");
    assert_eq!(output["metric_id"], "commerce.gmv");
    assert!(
        output["executed_sql"]
            .as_str()
            .unwrap()
            .contains("SUM(paid_amount)")
    );
}

#[tokio::test]
async fn an_unapproved_dimension_is_rejected_before_sql_execution() {
    let fixture = QueryToolFixture::sqlite().await;
    let plan = fixture
        .persist_plan(QueryPlan {
            source_id: SourceId::new("sqlite-demo"),
            execution: QueryExecutionPlan::Metric {
                metric_id: "commerce.gmv".to_owned(),
                start: Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 8, 8, 0, 0, 0).unwrap(),
                dimensions: vec!["card_number".to_owned()],
            },
        })
        .await;
    let outcome = fixture
        .call(
            "query_data",
            json!({
                "action": "preflight",
                "plan_artifact_id": plan.id,
                "plan_hash": plan.content_hash,
            }),
        )
        .await;

    assert!(matches!(outcome, ToolOutcome::Rejected { .. }));
}

#[tokio::test]
async fn metadata_query_completes_without_query_data() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture
        .call(
            "inspect_schema",
            json!({
                "source_id": "sqlite-demo",
                "relations": ["mart_orders"],
            }),
        )
        .await;
    let output = outcome.success_json().expect("schema success");

    assert_eq!(output["knowledge_kind"], "observed");
    assert_eq!(fixture.call_count("query_data").await, 0);
}

#[tokio::test]
async fn freshness_requires_an_approved_column_name() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture
        .call(
            "read_freshness",
            json!({
                "source_id": "sqlite-demo",
                "relation": "mart_orders",
                "time_column": "MAX(paid_at)",
            }),
        )
        .await;

    assert!(matches!(outcome, ToolOutcome::Rejected { .. }));
}

#[test]
fn query_parameter_keeps_a_timestamp_typed() {
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
    let value = serde_json::to_value(QueryParameter::Timestamp(timestamp)).unwrap();
    assert_eq!(value["type"], "timestamp");
}
