use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::Mutex;
use ys_agent_core::{
    AllowedDataScope, ArtifactId, ContextManifest, CoreError, CoreResult, CostClass,
    PendingRunEvent, PolicyDecision, Principal, PrincipalId, QueryBudget, RunEventKind, RunId,
    RunStatus, Sensitivity, SideEffect, TaskId, Tool, ToolCallId, ToolExecutionContext,
    ToolFailure, ToolFailureCategory, ToolOutcome, ToolRisk, ToolSpec, WorkflowKind, WorkspaceId,
};

use ys_agent_runtime::tools::{
    ConnectorToolAvailability, GovernedToolContext, QueryPhase, ToolCatalog, ToolEventSink,
    ToolRuntime, ToolViewBuilder, WorkspaceToolPolicy,
};

#[derive(Clone)]
struct TestTool {
    spec: ToolSpec,
    outcomes: Arc<Mutex<VecDeque<ToolOutcome>>>,
    calls: Arc<AtomicUsize>,
}

impl TestTool {
    fn read_only(name: &str) -> Self {
        Self::with_outcomes(
            name,
            vec![ToolOutcome::Succeeded {
                message: "test tool succeeded".to_owned(),
                output: json!({ "ok": true }),
                artifacts: Vec::new(),
            }],
        )
    }

    fn with_outcomes(name: &str, outcomes: Vec<ToolOutcome>) -> Self {
        Self {
            spec: ToolSpec {
                name: name.to_owned(),
                description: format!("Test implementation for {name}"),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                output_schema: json!({
                    "type": "object",
                    "properties": {
                        "ok": { "type": "boolean" }
                    },
                    "required": ["ok"],
                    "additionalProperties": false
                }),
                risk: ToolRisk::Low,
                side_effect: SideEffect::None,
                idempotent: true,
                timeout_ms: 1_000,
                required_permissions: vec!["data_query".to_owned()],
                input_sensitivity: Sensitivity::Internal,
                output_sensitivity: Sensitivity::Internal,
                version: "1.0.0".to_owned(),
            },
            outcomes: Arc::new(Mutex::new(outcomes.into())),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for TestTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(
        &self,
        _context: &ToolExecutionContext,
        _arguments: Value,
    ) -> CoreResult<ToolOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| CoreError::validation("test_outcomes_exhausted", "no test outcome"))
    }
}

#[test]
fn duplicate_tool_names_are_rejected() {
    let mut catalog = ToolCatalog::new();
    catalog
        .register(TestTool::read_only("inspect_schema"))
        .expect("first registration");

    let error = catalog
        .register(TestTool::read_only("inspect_schema"))
        .expect_err("duplicate name must fail");

    assert!(matches!(error, CoreError::DuplicateTool(_)));
}

fn catalog_with_query_tools() -> ToolCatalog {
    let mut catalog = ToolCatalog::new();
    for name in [
        "resolve_metric",
        "inspect_schema",
        "query_data",
        "read_freshness",
    ] {
        catalog
            .register(TestTool::read_only(name))
            .expect("valid query tool");
    }
    catalog
}

#[test]
fn query_view_exposes_only_tools_for_the_current_phase() {
    let catalog = catalog_with_query_tools();
    let principal = Principal::local_operator("ysc");
    let view = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::ResolveContext)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("valid ToolView");

    assert!(view.contains("resolve_metric"));
    assert!(view.contains("inspect_schema"));
    assert!(!view.contains("query_data"));
    assert!(!view.contains("read_freshness"));
}

fn transient_read_failure() -> ToolOutcome {
    ToolOutcome::Failed {
        failure: ToolFailure {
            code: "connector_temporarily_unavailable".to_owned(),
            category: ToolFailureCategory::Transport,
            user_message: "connector is temporarily unavailable".to_owned(),
            retryable: true,
            parameter_revision_allowed: false,
            remote_query_id: None,
            cost_class: CostClass::Low,
        },
    }
}

fn success_outcome() -> ToolOutcome {
    ToolOutcome::Succeeded {
        message: "test tool succeeded".to_owned(),
        output: json!({ "ok": true }),
        artifacts: Vec::new(),
    }
}

fn governed_context_for(
    tool: &TestTool,
    expected_cost_class: CostClass,
    connector_cost_unknown: bool,
) -> GovernedToolContext {
    let mut catalog = ToolCatalog::new();
    catalog
        .register(TestTool::read_only("resolve_metric"))
        .expect("metric tool");
    catalog.register(tool.clone()).expect("tool under test");

    let principal = Principal::local_operator("ysc");
    let view = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::ResolveContext)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("test ToolView");
    let workspace_id = WorkspaceId::new();

    GovernedToolContext {
        execution: ToolExecutionContext {
            call_id: ToolCallId::new(),
            workspace_id,
            task_id: TaskId::new(),
            run_id: RunId::new(),
            principal,
            query_budget: QueryBudget::default(),
            data_scope: AllowedDataScope {
                workspace_id,
                source_id: "warehouse".to_owned(),
                relations: BTreeMap::new(),
            },
        },
        view,
        policy: WorkspaceToolPolicy::default(),
        run_status: RunStatus::Running,
        expected_cost_class,
        connector_cost_unknown,
    }
}

#[tokio::test]
async fn runtime_retries_only_a_safe_transient_read() {
    let tool = TestTool::with_outcomes(
        "inspect_schema",
        vec![transient_read_failure(), success_outcome()],
    );
    let context = governed_context_for(&tool, CostClass::Low, false);
    let runtime = ToolRuntime::with_max_same_call_retries(1);

    let outcome = runtime
        .execute(Arc::new(tool.clone()), context, json!({}))
        .await;

    assert!(matches!(outcome, ToolOutcome::Succeeded { .. }));
    assert_eq!(tool.call_count(), 2);
}

#[tokio::test]
async fn runtime_never_retries_an_indeterminate_high_cost_read() {
    let tool = TestTool::with_outcomes(
        "inspect_schema",
        vec![
            ToolOutcome::Indeterminate {
                failure: ToolFailure {
                    code: "remote_status_unknown".to_owned(),
                    category: ToolFailureCategory::Transport,
                    user_message: "remote query status is unknown".to_owned(),
                    retryable: false,
                    parameter_revision_allowed: false,
                    remote_query_id: None,
                    cost_class: CostClass::High,
                },
            },
            success_outcome(),
        ],
    );
    let context = governed_context_for(&tool, CostClass::High, false);
    let runtime = ToolRuntime::with_max_same_call_retries(3);

    let outcome = runtime
        .execute(Arc::new(tool.clone()), context, json!({}))
        .await;

    assert!(matches!(outcome, ToolOutcome::Indeterminate { .. }));
    assert_eq!(tool.call_count(), 1);
}

#[test]
fn query_data_schema_is_narrowed_by_phase() {
    let catalog = catalog_with_query_tools();
    let principal = Principal::local_operator("ysc");

    let preflight = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::ValidateAndPreflight)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("preflight view");
    let execute = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::Execute)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("execute view");

    assert_eq!(
        preflight
            .spec("query_data")
            .expect("preflight spec")
            .input_schema["properties"]["action"]["const"],
        "preflight"
    );
    assert_eq!(
        execute
            .spec("query_data")
            .expect("execute spec")
            .input_schema["properties"]["action"]["const"],
        "execute"
    );
    assert_ne!(preflight.content_hash(), execute.content_hash());
}

#[test]
fn identical_tool_views_have_identical_hashes() {
    let catalog = catalog_with_query_tools();
    let principal = Principal::local_operator("ysc");

    let build = || {
        ToolViewBuilder::new(&catalog)
            .for_workflow(WorkflowKind::Query)
            .for_query_phase(QueryPhase::ResolveContext)
            .for_principal(&principal)
            .with_connector_tools(ConnectorToolAvailability::all_query_tools())
            .for_run_status(RunStatus::Running)
            .build()
            .expect("deterministic view")
    };

    assert_eq!(build().content_hash(), build().content_hash());
}

fn parameter_revision_failure() -> ToolOutcome {
    ToolOutcome::Failed {
        failure: ToolFailure {
            code: "schema_changed".to_owned(),
            category: ToolFailureCategory::SchemaChanged,
            user_message: "query parameters need a new plan".to_owned(),
            retryable: true,
            parameter_revision_allowed: true,
            remote_query_id: None,
            cost_class: CostClass::Low,
        },
    }
}

#[tokio::test]
async fn runtime_never_same_call_retries_parameter_revision() {
    let tool = TestTool::with_outcomes(
        "inspect_schema",
        vec![parameter_revision_failure(), success_outcome()],
    );
    let context = governed_context_for(&tool, CostClass::Low, false);
    let runtime = ToolRuntime::with_max_same_call_retries(3);

    let outcome = runtime
        .execute(Arc::new(tool.clone()), context, json!({}))
        .await;

    assert!(matches!(
        outcome,
        ToolOutcome::Failed {
            failure: ToolFailure {
                parameter_revision_allowed: true,
                ..
            }
        }
    ));
    assert_eq!(tool.call_count(), 1);
}

#[tokio::test]
async fn runtime_never_retries_when_connector_cost_is_unknown() {
    let tool = TestTool::with_outcomes(
        "inspect_schema",
        vec![transient_read_failure(), success_outcome()],
    );
    let context = governed_context_for(&tool, CostClass::Low, true);
    let runtime = ToolRuntime::with_max_same_call_retries(3);

    let outcome = runtime
        .execute(Arc::new(tool.clone()), context, json!({}))
        .await;

    assert!(matches!(outcome, ToolOutcome::Failed { .. }));
    assert_eq!(tool.call_count(), 1);
}

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<PendingRunEvent>>,
}

#[async_trait]
impl ToolEventSink for RecordingEventSink {
    async fn emit(&self, event: PendingRunEvent) -> CoreResult<()> {
        self.events.lock().await.push(event);
        Ok(())
    }
}

#[tokio::test]
async fn runtime_emits_policy_started_and_terminal_events_in_order() {
    let tool = TestTool::read_only("inspect_schema");
    let context = governed_context_for(&tool, CostClass::Low, false);
    let sink = Arc::new(RecordingEventSink::default());
    let runtime = ToolRuntime::with_event_sink(0, sink.clone());

    let outcome = runtime.execute(Arc::new(tool), context, json!({})).await;
    assert!(matches!(outcome, ToolOutcome::Succeeded { .. }));

    let events = sink.events.lock().await;
    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0].kind,
        RunEventKind::PolicyEvaluated {
            decision: PolicyDecision::Allow,
            ..
        }
    ));
    assert!(matches!(
        &events[1].kind,
        RunEventKind::ToolExecutionStarted { .. }
    ));
    assert!(matches!(
        &events[2].kind,
        RunEventKind::ToolExecutionSucceeded { .. }
    ));
}

#[test]
fn principal_without_data_query_gets_an_empty_view() {
    let catalog = catalog_with_query_tools();
    let principal = Principal {
        id: PrincipalId::new(),
        display_name: "viewer".to_owned(),
        capabilities: BTreeSet::new(),
    };

    let view = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::ResolveContext)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("empty least-privilege view");

    assert!(view.model_tools().is_empty());
}

#[tokio::test]
async fn source_acl_rejection_happens_before_tool_execution() {
    let mut tool = TestTool::read_only("inspect_schema");
    tool.spec.input_schema = json!({
        "type": "object",
        "properties": {
            "source_id": { "type": "string" }
        },
        "required": ["source_id"],
        "additionalProperties": false
    });
    let context = governed_context_for(&tool, CostClass::Low, false);
    let runtime = ToolRuntime::with_max_same_call_retries(0);

    let outcome = runtime
        .execute(
            Arc::new(tool.clone()),
            context,
            json!({ "source_id": "not-the-allowed-warehouse" }),
        )
        .await;

    assert!(matches!(
        outcome,
        ToolOutcome::Rejected {
            failure: ToolFailure { ref code, .. }
        } if code == "source_acl_denied"
    ));
    assert_eq!(tool.call_count(), 0);
}

#[tokio::test]
async fn invalid_success_output_is_normalized_to_failure() {
    let tool = TestTool::with_outcomes(
        "inspect_schema",
        vec![ToolOutcome::Succeeded {
            message: "bad output".to_owned(),
            output: json!({ "ok": "not-a-boolean" }),
            artifacts: Vec::new(),
        }],
    );
    let context = governed_context_for(&tool, CostClass::Low, false);
    let runtime = ToolRuntime::with_max_same_call_retries(0);

    let outcome = runtime.execute(Arc::new(tool), context, json!({})).await;

    assert!(matches!(
        outcome,
        ToolOutcome::Failed {
            failure: ToolFailure { ref code, .. }
        } if code == "invalid_tool_output"
    ));
}

#[test]
fn preview_is_redacted_above_workspace_preview_sensitivity() {
    let tool = TestTool::read_only("inspect_schema");
    let policy = WorkspaceToolPolicy {
        max_preview_sensitivity: Sensitivity::Public,
        ..WorkspaceToolPolicy::default()
    };
    let runtime = ToolRuntime::with_max_same_call_retries(0);

    let preview = runtime.safe_preview(&success_outcome(), &tool.spec(), &policy);

    assert_eq!(preview["redacted"], true);
}

#[test]
fn tool_view_hash_is_shared_by_manifest_and_model_event() {
    let catalog = catalog_with_query_tools();
    let principal = Principal::local_operator("ysc");
    let view = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_query_phase(QueryPhase::ResolveContext)
        .for_principal(&principal)
        .with_connector_tools(ConnectorToolAvailability::all_query_tools())
        .for_run_status(RunStatus::Running)
        .build()
        .expect("ToolView");

    let mut manifest = ContextManifest::empty(8_000);
    view.apply_to_manifest(&mut manifest);
    let event = RunEventKind::ModelRequested {
        model_call_id: "model-call-1".to_owned(),
        context_manifest_id: ArtifactId::new(),
        tool_view_hash: view.content_hash().to_owned(),
    };

    let RunEventKind::ModelRequested { tool_view_hash, .. } = event else {
        unreachable!("constructed ModelRequested")
    };
    assert_eq!(manifest.tool_view_version, tool_view_hash);
}
