# YS Data Agent v0.2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Build a trustworthy, resumable Query Workflow and a Claude Code/OpenCode-style TUI on top of a reusable Task-centric Rust Agent Runtime.

**Architecture:** Convert the v0.1 single crate into a small Cargo Workspace. Keep domain contracts in ys-agent-core, orchestration in ys-agent-runtime, local persistence in ys-agent-store, external integrations in ys-agent-adapters, and dependency assembly plus TUI in apps/ysda. v0.2 implements one Query vertical slice; later Workflows reuse the same Harness, Tool Runtime, Event Store, Context and Eval contracts.

**Tech Stack:** Rust 2024, Tokio, Serde, async-trait, UUID, Chrono, Rusqlite, SQLx/Postgres, Reqwest, sqlparser, Ratatui, Crossterm, tracing, SHA-256, Wiremock.

**Design spec:** docs/superpowers/specs/2026-08-06-ys-data-agent-architecture-design.md

---

## 0. Scope guard

### v0.2 must deliver

- ysda with no arguments opens a full-screen interactive TUI.
- The TUI communicates only through AgentService.
- Session, Task, Run, Step, typed Event, Snapshot and Artifact are durable.
- One shared Harness drives a real multi-step Query Workflow.
- ToolCatalog is separate from the per-step ToolView.
- OpenAI-compatible Tool Calling is the only production model protocol.
- Fake and Replay providers make tests deterministic.
- SQLite remains the deterministic test connector.
- Postgres is the first real remote connector.
- dbt manifest is the first engineering Context Adapter.
- A file-backed Metric Registry supplies Active metric contracts.
- QueryVerifier and Completion Gate produce a structured QueryArtifact.
- Runtime state, Telemetry and Eval records remain separate.

### v0.2 defines contracts but does not build the full subsystem

- ApprovalRequest and action_hash types;
- ExecutionHandle and durable long-job states;
- ChangeRequest and TaskHandoff types;
- SemanticProvider extension point;
- TelemetrySink extension point.

### v0.2 excludes

- Analysis, Build/Change, Operate and ML Data Prep Workflows;
- production writes, Merge and Deploy;
- Web/API server, multi-user authentication and remote clients;
- background Worker, Webhook and Reconciler;
- Python capability Worker;
- Langfuse exporter;
- vector database and embedding pipeline;
- complete semantic engine;
- non-OpenAI protocol provider implementations.

### Recovery promise

v0.2 resumes between persisted Steps and after WaitingForInput. If the process dies while a read-only SQL request is in flight, the ToolCall becomes Unknown and a resumed Run creates a new safe read-only ToolCall. Exact recovery of multi-day external jobs begins with the Build/Operate milestone.

---

## 1. Final repository map

~~~text
Cargo.toml
.env.example
README.md
apps/
└── ysda/
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs
    │   ├── main.rs
    │   ├── bootstrap.rs
    │   ├── cli.rs
    │   └── tui/
    │       ├── mod.rs
    │       ├── app.rs
    │       ├── event_loop.rs
    │       ├── input.rs
    │       └── ui.rs
    └── tests/
        ├── cli_test.rs
        ├── query_eval_test.rs
        └── tui_test.rs
crates/
├── ys-agent-core/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── error.rs
│   │   ├── ids.rs
│   │   ├── identity.rs
│   │   ├── session.rs
│   │   ├── task.rs
│   │   ├── run.rs
│   │   ├── event.rs
│   │   ├── artifact.rs
│   │   ├── approval.rs
│   │   ├── execution.rs
│   │   ├── change.rs
│   │   ├── tool.rs
│   │   ├── model.rs
│   │   ├── context.rs
│   │   ├── semantic.rs
│   │   ├── connector.rs
│   │   └── ports.rs
│   └── tests/
│       ├── lifecycle_test.rs
│       └── contracts_test.rs
├── ys-agent-runtime/
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── service.rs
│   │   ├── harness.rs
│   │   ├── coordinator.rs
│   │   ├── loop_driver.rs
│   │   ├── recovery.rs
│   │   ├── context_assembler.rs
│   │   ├── telemetry.rs
│   │   ├── tools/
│   │   │   ├── mod.rs
│   │   │   ├── catalog.rs
│   │   │   ├── runtime.rs
│   │   │   └── view.rs
│   │   └── workflow/
│   │       ├── mod.rs
│   │       └── query/
│   │           ├── mod.rs
│   │           ├── state.rs
│   │           ├── prompts.rs
│   │           ├── verifier.rs
│   │           └── artifact.rs
│   └── tests/
│       ├── service_test.rs
│       ├── tool_runtime_test.rs
│       ├── query_workflow_test.rs
│       └── recovery_test.rs
├── ys-agent-store/
│   ├── Cargo.toml
│   ├── migrations/0001_runtime.sql
│   ├── src/
│   │   ├── lib.rs
│   │   ├── sqlite.rs
│   │   └── local_artifacts.rs
│   └── tests/sqlite_store_test.rs
└── ys-agent-adapters/
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs
    │   ├── model/
    │   │   ├── mod.rs
    │   │   ├── openai_compatible.rs
    │   │   ├── fake.rs
    │   │   └── replay.rs
    │   ├── data/
    │   │   ├── mod.rs
    │   │   ├── sqlite.rs
    │   │   ├── postgres.rs
    │   │   └── sql_policy.rs
    │   ├── context/
    │   │   ├── mod.rs
    │   │   ├── dbt_manifest.rs
    │   │   └── metric_registry.rs
    │   └── tools/
    │       ├── mod.rs
    │       ├── inspect_schema.rs
    │       ├── resolve_metric.rs
    │       ├── read_freshness.rs
    │       └── query_data.rs
    └── tests/
        ├── model_provider_test.rs
        ├── sqlite_connector_test.rs
        ├── postgres_connector_test.rs
        ├── context_adapter_test.rs
        └── query_tools_test.rs
evals/
├── query_cases.jsonl
└── README.md
fixtures/
├── dbt/manifest.json
├── metrics/metrics.json
├── postgres/compose.yaml
└── sql/
    ├── sqlite_seed.sql
    └── postgres_seed.sql
scripts/
└── v0.2-release-gate.sh
~~~

The file map is intentionally limited to four library crates and one application crate. Do not create separate crates for Memory, Eval, Policy or each Connector during v0.2.

---

## Task 1: Convert the repository to a compiling Cargo Workspace

**Files:**

- Modify: Cargo.toml
- Create: apps/ysda/Cargo.toml
- Move: src/* to apps/ysda/src/*
- Move: tests/* to apps/ysda/tests/*
- Create: crates/ys-agent-core/Cargo.toml
- Create: crates/ys-agent-core/src/lib.rs
- Create: crates/ys-agent-runtime/Cargo.toml
- Create: crates/ys-agent-runtime/src/lib.rs
- Create: crates/ys-agent-store/Cargo.toml
- Create: crates/ys-agent-store/src/lib.rs
- Create: crates/ys-agent-adapters/Cargo.toml
- Create: crates/ys-agent-adapters/src/lib.rs

- [ ] **Step 1: Record the v0.1 baseline**

Run:

~~~bash
rtk cargo test
rtk cargo clippy --all-targets --all-features -- -D warnings
~~~

Expected: 19 tests pass and Clippy exits successfully.

- [ ] **Step 2: Move the existing application without changing behavior**

Run:

~~~bash
rtk mkdir -p apps/ysda/src apps/ysda/tests
rtk git mv src/* apps/ysda/src/
rtk git mv tests/* apps/ysda/tests/
~~~

Expected: the original source and tests now live under apps/ysda.

- [ ] **Step 3: Replace the root manifest**

Write Cargo.toml:

~~~toml
[workspace]
members = [
    "apps/ysda",
    "crates/ys-agent-core",
    "crates/ys-agent-runtime",
    "crates/ys-agent-store",
    "crates/ys-agent-adapters",
]
resolver = "3"

[workspace.package]
version = "0.2.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
async-trait = "0.1"
bigdecimal = { version = "0.4", features = ["serde"] }
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
comfy-table = "7"
crossterm = { version = "0.29", features = ["event-stream"] }
futures = "0.3"
ratatui = { version = "0.30.2", default-features = false, features = ["crossterm"] }
reqwest = { version = "0.13", features = ["json"] }
rusqlite = { version = "0.40", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_path_to_error = "0.1"
sha2 = "0.10"
sqlparser = "0.62"
sqlx = { version = "0.9", default-features = false, features = [
    "bigdecimal",
    "chrono",
    "json",
    "postgres",
    "runtime-tokio",
    "tls-rustls",
    "uuid",
] }
thiserror = "2"
tempfile = "3"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tokio-stream = "0.1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
uuid = { version = "1", features = ["serde", "v4"] }
wiremock = "0.6"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = "warn"
~~~

- [ ] **Step 4: Give every crate a minimal compiling manifest**

Write apps/ysda/Cargo.toml:

~~~toml
[package]
name = "ysda"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
clap.workspace = true
comfy-table.workspace = true
crossterm.workspace = true
futures.workspace = true
ratatui.workspace = true
reqwest.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlparser.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
ys-agent-core = { path = "../../crates/ys-agent-core" }
ys-agent-runtime = { path = "../../crates/ys-agent-runtime" }
ys-agent-store = { path = "../../crates/ys-agent-store" }
ys-agent-adapters = { path = "../../crates/ys-agent-adapters" }

[dev-dependencies]
tempfile.workspace = true
wiremock.workspace = true

[[bin]]
name = "ysda"
path = "src/main.rs"

[lints]
workspace = true
~~~

Write crates/ys-agent-core/Cargo.toml:

~~~toml
[package]
name = "ys-agent-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
uuid.workspace = true

[lints]
workspace = true
~~~

Write crates/ys-agent-runtime/Cargo.toml:

~~~toml
[package]
name = "ys-agent-runtime"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
ys-agent-core = { path = "../ys-agent-core" }

[dev-dependencies]
tempfile.workspace = true
ys-agent-adapters = { path = "../ys-agent-adapters" }
ys-agent-store = { path = "../ys-agent-store" }

[lints]
workspace = true
~~~

Write crates/ys-agent-store/Cargo.toml:

~~~toml
[package]
name = "ys-agent-store"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait.workspace = true
chrono.workspace = true
rusqlite.workspace = true
serde_json.workspace = true
sha2.workspace = true
tokio.workspace = true
ys-agent-core = { path = "../ys-agent-core" }

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
~~~

Write crates/ys-agent-adapters/Cargo.toml:

~~~toml
[package]
name = "ys-agent-adapters"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
async-trait.workspace = true
bigdecimal.workspace = true
chrono.workspace = true
reqwest.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
serde_path_to_error.workspace = true
sqlparser.workspace = true
sqlx.workspace = true
tokio.workspace = true
tracing.workspace = true
ys-agent-core = { path = "../ys-agent-core" }

[dev-dependencies]
tempfile.workspace = true
wiremock.workspace = true

[lints]
workspace = true
~~~

Every initial lib.rs contains only a crate-level purpose comment. Runtime may have adapters and store as dev-dependencies because neither production crate depends on Runtime; this does not create a production dependency cycle.

- [ ] **Step 5: Fix the moved integration-test support path**

Ensure apps/ysda/tests/support/mod.rs remains addressable through mod support and that CARGO_BIN_EXE_ysda resolves from the application package.

- [ ] **Step 6: Verify the workspace preserves v0.1 behavior**

Run:

~~~bash
rtk cargo fmt --all --check
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

Expected: the original 19 tests pass from their new location; the four empty library crates compile.

- [ ] **Step 7: Commit**

~~~bash
rtk git add Cargo.toml Cargo.lock apps crates
rtk git commit -m "refactor: establish agent workspace boundaries"
~~~

---

## Task 2: Define Session, Task and Run lifecycle types

**Files:**

- Create: crates/ys-agent-core/src/error.rs
- Create: crates/ys-agent-core/src/ids.rs
- Create: crates/ys-agent-core/src/identity.rs
- Create: crates/ys-agent-core/src/session.rs
- Create: crates/ys-agent-core/src/task.rs
- Create: crates/ys-agent-core/src/run.rs
- Modify: crates/ys-agent-core/src/lib.rs
- Test: crates/ys-agent-core/tests/lifecycle_test.rs

- [ ] **Step 1: Write failing lifecycle tests**

Create lifecycle_test.rs:

~~~rust
use ys_agent_core::{
    Capability, Principal, Run, RunStatus, Session, Task, TaskId, TaskStatus,
    WorkflowKind, WorkspaceId,
};

#[test]
fn new_session_and_task_have_separate_lifecycles() {
    let principal = Principal::local_owner("ysc");
    let session = Session::new(WorkspaceId::new(), principal.id.clone());
    let task = Task::new(
        session.workspace_id.clone(),
        principal.id,
        "Query the last seven complete days of GMV",
    );

    assert_ne!(session.id.to_string(), task.id.to_string());
    assert_eq!(task.status, TaskStatus::Open);
}

#[test]
fn waiting_and_resume_keep_the_same_run_id() {
    let mut run = Run::new(TaskId::new(), WorkflowKind::Query);
    let original = run.id.clone();

    run.wait_for_input("clarification-1").expect("running to waiting");
    run.resume().expect("waiting to running");

    assert_eq!(run.id, original);
    assert_eq!(run.status, RunStatus::Running);
}

#[test]
fn local_owner_has_query_but_roles_are_capability_based() {
    let principal = Principal::local_owner("ysc");
    assert!(principal.capabilities.contains(&Capability::DataQuery));
    assert!(principal.capabilities.contains(&Capability::ChangePrepare));
}
~~~

- [ ] **Step 2: Run the test to verify it fails**

Run:

~~~bash
rtk cargo test -p ys-agent-core --test lifecycle_test
~~~

Expected: FAIL because the domain types are not defined.

- [ ] **Step 3: Implement strongly typed identifiers**

In ids.rs define serde-transparent UUID newtypes for WorkspaceId, PrincipalId, SessionId, TaskId, RunId, StepId, ToolCallId, ExecutionId, ArtifactId and EventId. Every type must provide new(), Display and FromStr. Do not expose raw UUID fields publicly.

Use one private macro to keep the implementations identical:

~~~rust
macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}
~~~

- [ ] **Step 4: Implement identity and capability types**

In identity.rs define Capability as a serializable enum containing DataQuery, DataAnalyze, ChangeRequest, ChangePrepare, ChangeReview, ChangeMerge and ProductionExecute. Principal contains PrincipalId, display_name and a BTreeSet of Capability. local_owner grants all capabilities for the single-user local profile. business_user grants DataQuery, DataAnalyze and ChangeRequest only. No Runtime code may infer capabilities from display_name.

- [ ] **Step 5: Implement Session, Task and Run transitions**

Use UTC DateTime timestamps. Session holds id, workspace_id, principal_id, focused_task_id, created_at and closed_at.

Task holds id, workspace_id, goal, acceptance_criteria, status, parent_task_id, created_by and timestamps. TaskStatus is Open, InProgress, Waiting, Completed or Cancelled.

Run holds id, task_id, workflow, status, attempt, retry_of_run_id, version and timestamps. RunStatus is Queued, Running, WaitingForInput, WaitingForApproval, WaitingForExecution, Succeeded, Failed or Cancelled.

RunSnapshot holds run identity, Task identity, Workflow, RunStatus, version, serialized workflow_state, pending wait metadata, primary_artifact_id and the last completed Step. Workflow-specific state remains JSON at the Core boundary and is decoded by the owning Workflow.

Transition methods return CoreError::InvalidTransition for illegal edges. A terminal Run never returns to Running.

- [ ] **Step 6: Export the domain API and pass tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-core --test lifecycle_test
rtk cargo clippy -p ys-agent-core --all-targets -- -D warnings
~~~

Expected: all lifecycle tests pass.

- [ ] **Step 7: Commit**

~~~bash
rtk git add crates/ys-agent-core
rtk git commit -m "feat(core): add task-centric lifecycle model"
~~~

---

## Task 3: Define typed Events, Artifacts, Context, Model and Tool contracts

**Files:**

- Create: crates/ys-agent-core/src/event.rs
- Create: crates/ys-agent-core/src/artifact.rs
- Create: crates/ys-agent-core/src/approval.rs
- Create: crates/ys-agent-core/src/execution.rs
- Create: crates/ys-agent-core/src/change.rs
- Create: crates/ys-agent-core/src/context.rs
- Create: crates/ys-agent-core/src/semantic.rs
- Create: crates/ys-agent-core/src/connector.rs
- Create: crates/ys-agent-core/src/model.rs
- Create: crates/ys-agent-core/src/tool.rs
- Create: crates/ys-agent-core/src/ports.rs
- Modify: crates/ys-agent-core/src/lib.rs
- Test: crates/ys-agent-core/tests/contracts_test.rs

- [ ] **Step 1: Write failing serialization and policy tests**

Create contracts_test.rs:

~~~rust
use serde_json::json;
use ys_agent_core::{
    AgentAction, ApprovalAction, ContextManifest, RunEventKind, SideEffect,
    ToolOutcome, ToolSpec, VersionedRunEvent,
};

#[test]
fn run_event_kind_round_trips_with_a_schema_version() {
    let kind = RunEventKind::RunWaiting {
        reason: "clarification".to_owned(),
    };
    let value = serde_json::to_value(VersionedRunEvent::v1(kind))
        .expect("serialize event");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["kind"]["type"], "run_waiting");
}

#[test]
fn model_can_only_propose_supported_actions() {
    let action = AgentAction::RequestClarification {
        question: "Use seven complete calendar days?".to_owned(),
    };
    assert!(matches!(action, AgentAction::RequestClarification { .. }));
}

#[test]
fn write_tools_must_declare_a_non_none_side_effect() {
    let spec = ToolSpec::new(
        "backfill_partition",
        json!({"type": "object"}),
        json!({"type": "object"}),
    )
    .with_side_effect(SideEffect::ProductionWrite);
    assert_ne!(spec.side_effect, SideEffect::None);
}

#[test]
fn context_manifest_records_omissions() {
    let manifest = ContextManifest::empty(8_000)
        .omit("artifact://large-log", "token_budget");
    assert_eq!(manifest.omitted.len(), 1);
}

#[test]
fn indeterminate_tool_outcomes_are_not_retryable() {
    let outcome = ToolOutcome::indeterminate("remote status unknown");
    assert!(!outcome.safe_to_retry_same_call());
}

#[test]
fn approval_hash_changes_when_a_material_parameter_changes() {
    let first = ApprovalAction::new("backfill_partition", json!({
        "relation": "orders",
        "start": "2026-08-01",
        "end": "2026-08-02"
    }));
    let second = ApprovalAction::new("backfill_partition", json!({
        "relation": "orders",
        "start": "2026-08-01",
        "end": "2026-08-03"
    }));
    assert_ne!(first.action_hash, second.action_hash);
}
~~~

- [ ] **Step 2: Run the test to verify it fails**

Run:

~~~bash
rtk cargo test -p ys-agent-core --test contracts_test
~~~

Expected: FAIL because contracts are missing.

- [ ] **Step 3: Implement EventEnvelope and RunEventKind**

EventEnvelope contains event_id, workspace_id, task_id, run_id, sequence, occurred_at, actor and VersionedRunEvent. VersionedRunEvent contains schema_version and a serde-tagged RunEventKind. PendingRunEvent contains actor and RunEventKind before the Store assigns Event identity, timestamp and sequence.

Implement at least:

~~~rust
pub enum RunEventKind {
    RunStarted,
    StepStarted { step_id: StepId, label: String },
    ModelRequested { model_call_id: String, context_manifest_id: ArtifactId },
    ModelResponded { model_call_id: String, action: AgentAction },
    ToolCallProposed { call: ToolCall },
    PolicyEvaluated { call_id: ToolCallId, decision: PolicyDecision },
    ToolExecutionStarted { call_id: ToolCallId },
    ToolExecutionSucceeded { call_id: ToolCallId, artifacts: Vec<ArtifactId> },
    ToolExecutionFailed { call_id: ToolCallId, failure: ToolFailure },
    ArtifactCreated { artifact: ArtifactMetadata },
    ClarificationRequested { clarification_id: String, question: String },
    ClarificationAnswered { clarification_id: String, answer_artifact_id: ArtifactId },
    RunWaiting { reason: String },
    RunResumed,
    RunCompleted { primary_artifact_id: ArtifactId },
    RunFailed { code: String, message: String },
    RunCancelled { reason: String },
}
~~~

VersionedRunEvent::v1 sets schema_version = 1. Reject a future schema version on load until a compatible decoder exists.

- [ ] **Step 4: Implement Artifact metadata and references**

ArtifactMetadata contains id, workspace_id, task_id, run_id, kind, media_type, content_hash, size_bytes, storage_uri, sensitivity, created_at and producer_step_id. ArtifactKind includes Query, VerificationReport, Sql, QueryResult, ContextManifest, ContextSummary, ExecutionLog, TaskHandoff, ChangeSet and AnalysisReport.

ArtifactRef exposes metadata without loading the body.

- [ ] **Step 5: Implement Model and Tool contracts**

ModelRequest contains model, messages, tools, context_manifest and temperature. ModelMessage uses System, User, Assistant and Tool roles.

AgentAction contains CallTool, RequestCapability, RequestClarification and ProposeCompletion.

ToolSpec contains input/output JSON Schema, risk, side_effect, idempotency, timeout, required_permissions, sensitivity and version.

ToolOutcome contains Succeeded, Failed, Rejected, ApprovalRequired, Running and Indeterminate. ToolFailure contains category, message, retryability, parameter_revision_allowed and side_effect_state.

- [ ] **Step 6: Implement Context, Semantic and Connector contracts**

ContextEvidence records source, source_type, version, observed_at, freshness, owner, ACL, sensitivity, confidence and token_cost.

ContextManifest records included Evidence, summaries, ToolView version, token budget and omissions.

MetricDefinition contains id, version, status, description, source_relation, expression, time_column, allowed_dimensions, owner and freshness_sla_seconds. MetricStatus is Draft, Active or Deprecated.

Connector contracts contain SourceId, CapabilityDescriptor, ObservedSchema, QueryRequest, QueryResult and FreshnessObservation. Keep connection secrets out of every serializable event type.

Also define the long-term contracts that v0.2 persists but does not execute:

- ApprovalAction and ApprovalRequest with canonical JSON SHA-256 action_hash, target, environment, risk, expiry and requested permissions;
- ExecutionHandle with backend, external_job_id, idempotency_key and Queued, Running, Succeeded, Failed, CancelRequested, Cancelled or Unknown state;
- ChangeRequest with evidence references and requested outcome;
- TaskHandoff with goal, acceptance criteria, confirmed Fact references, unresolved questions, assumptions and requested permissions.

Canonical JSON hashing recursively sorts object keys and preserves array order. Any material parameter, target or Artifact version change produces a different action_hash.

- [ ] **Step 7: Define ports**

Use async-trait for:

~~~rust
#[async_trait]
pub trait RuntimeStore: Send + Sync {
    async fn create_session(&self, session: &Session) -> CoreResult<()>;
    async fn create_task(&self, task: &Task) -> CoreResult<()>;
    async fn create_run(&self, run: &Run) -> CoreResult<()>;
    async fn load_session(&self, session_id: &SessionId) -> CoreResult<Session>;
    async fn load_task(&self, task_id: &TaskId) -> CoreResult<Task>;
    async fn load_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot>;
    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>>;
    async fn load_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<Vec<EventEnvelope>>;
    async fn append(
        &self,
        run_id: &RunId,
        expected_version: u64,
        events: Vec<PendingRunEvent>,
        snapshot: &RunSnapshot,
    ) -> CoreResult<()>;
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, request: PutArtifact) -> CoreResult<ArtifactMetadata>;
    async fn get(&self, artifact: &ArtifactRef) -> CoreResult<Vec<u8>>;
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;
    async fn complete(&self, request: ModelRequest) -> CoreResult<ModelResponse>;
}
~~~

Also define Tool, CatalogReader, SqlQueryExecutor, FreshnessReader, MetricProvider and ContextProvider as small independent ports. Tool exposes spec() and async execute(context, arguments), returning ToolOutcome.

- [ ] **Step 8: Pass contract tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-core
rtk cargo clippy -p ys-agent-core --all-targets -- -D warnings
~~~

Expected: lifecycle and contract suites pass.

- [ ] **Step 9: Commit**

~~~bash
rtk git add crates/ys-agent-core
rtk git commit -m "feat(core): define runtime and capability contracts"
~~~

---

## Task 4: Implement the SQLite Runtime Store and local Artifact Store

**Files:**

- Create: crates/ys-agent-store/migrations/0001_runtime.sql
- Create: crates/ys-agent-store/src/sqlite.rs
- Create: crates/ys-agent-store/src/local_artifacts.rs
- Modify: crates/ys-agent-store/src/lib.rs
- Modify: crates/ys-agent-store/Cargo.toml
- Test: crates/ys-agent-store/tests/sqlite_store_test.rs

- [ ] **Step 1: Write failing store tests**

Create sqlite_store_test.rs covering:

~~~rust
#[tokio::test]
async fn append_is_atomic_and_optimistically_versioned() {
    let store = TestStore::new().await;
    let run = store.seed_running_query().await;

    store.append(
        &run.id,
        0,
        vec![pending(RunEventKind::RunStarted)],
        &run.snapshot(1),
    )
        .await
        .expect("first append");

    let error = store.append(
        &run.id,
        0,
        vec![pending(RunEventKind::RunResumed)],
        &run.snapshot(1),
    )
        .await
        .expect_err("stale version must fail");

    assert!(matches!(error, CoreError::VersionConflict { .. }));
}

#[tokio::test]
async fn reopened_store_loads_the_latest_snapshot_and_events() {
    let fixture = PersistentStoreFixture::new().await;
    let run_id = fixture.persist_waiting_run().await;
    drop(fixture.store);

    let reopened = SqliteRuntimeStore::open(&fixture.database).await.expect("reopen");
    let loaded = reopened.load_run(&run_id).await.expect("load");

    assert_eq!(loaded.status, RunStatus::WaitingForInput);
    assert_eq!(loaded.version, 2);
}

#[tokio::test]
async fn artifact_bytes_are_addressed_by_hash_not_user_filename() {
    let store = LocalArtifactStore::new(tempdir().unwrap().path());
    let metadata = store.put(query_result_request(b"secret rows")).await.unwrap();
    assert!(!metadata.storage_uri.contains("secret rows"));
}
~~~

- [ ] **Step 2: Run the tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-store
~~~

Expected: FAIL because store implementations are missing.

- [ ] **Step 3: Add the Runtime Store migration**

0001_runtime.sql creates:

~~~sql
CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE tasks (
    task_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    parent_task_id TEXT,
    status TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    status TEXT NOT NULL,
    version INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES tasks(task_id)
);

CREATE TABLE run_events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    UNIQUE(run_id, sequence),
    FOREIGN KEY(run_id) REFERENCES runs(run_id)
);

CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
~~~

Add indexes on task status, run status and event run_id/sequence.

- [ ] **Step 4: Implement SqliteRuntimeStore**

Open one SQLite connection per blocking operation inside tokio::task::spawn_blocking. Enable foreign_keys, WAL and busy_timeout. Apply numbered migrations exactly once using a schema_migrations table.

append must:

1. begin IMMEDIATE transaction;
2. read the current run version;
3. compare it with expected_version;
4. insert ordered events;
5. update snapshot_json, status and version;
6. commit.

Never update Snapshot without inserting the corresponding Events in the same transaction.

- [ ] **Step 5: Implement LocalArtifactStore**

Compute SHA-256 before writing. Store bytes under:

~~~text
artifacts/<first-two-hash-chars>/<full-hash>
~~~

Write to a generated temporary filename in the same directory, fsync, then atomically rename. Return existing content when the hash already exists. Never use a user-supplied filename as a path.

- [ ] **Step 6: Pass store tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-store
rtk cargo clippy -p ys-agent-store --all-targets -- -D warnings
~~~

Expected: atomic append, reopen recovery and content-addressed Artifact tests pass.

- [ ] **Step 7: Commit**

~~~bash
rtk git add crates/ys-agent-store
rtk git commit -m "feat(store): persist runtime events snapshots and artifacts"
~~~

---

## Task 5: Add the in-process AgentService and deterministic Coordinator

**Files:**

- Create: crates/ys-agent-runtime/src/service.rs
- Create: crates/ys-agent-runtime/src/coordinator.rs
- Modify: crates/ys-agent-runtime/src/lib.rs
- Modify: crates/ys-agent-runtime/Cargo.toml
- Test: crates/ys-agent-runtime/tests/service_test.rs

- [ ] **Step 1: Write failing AgentService tests**

Create service_test.rs:

~~~rust
#[tokio::test]
async fn new_session_does_not_cancel_existing_tasks() {
    let fixture = ServiceFixture::new().await;
    let session_one = fixture.service.create_session(fixture.principal()).await.unwrap();
    let task = fixture.service
        .create_task(CreateTaskRequest {
            session_id: session_one.id.clone(),
            goal: "Query GMV".to_owned(),
            acceptance_criteria: vec![],
        })
        .await
        .unwrap();

    let session_two = fixture.service.create_session(fixture.principal()).await.unwrap();

    assert_ne!(session_one.id, session_two.id);
    assert_eq!(
        fixture.service.get_task(&task.id).await.unwrap().status,
        TaskStatus::Open
    );
}

#[tokio::test]
async fn unrelated_input_creates_a_new_task() {
    let fixture = ServiceFixture::new().await;
    let session = fixture.service.create_session(fixture.principal()).await.unwrap();
    let gmv = fixture.service.create_task(CreateTaskRequest {
        session_id: session.id.clone(),
        goal: "Query GMV".to_owned(),
        acceptance_criteria: vec![],
    }).await.unwrap();

    let decision = fixture.coordinator.route(
        &session,
        Some(&gmv),
        "Query yesterday's DAU",
    ).await.unwrap();

    assert!(matches!(decision, CoordinationDecision::CreateNewTask { .. }));
}

#[tokio::test]
async fn a_business_principal_can_request_but_not_prepare_changes() {
    let fixture = ServiceFixture::new().await;
    let principal = Principal::business_user("product-user");
    assert!(principal.can(Capability::ChangeRequest));
    assert!(!principal.can(Capability::ChangePrepare));
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-runtime --test service_test
~~~

Expected: FAIL because AgentService and Coordinator are missing.

- [ ] **Step 3: Define AgentService commands and responses**

AgentService is generic over Arc<dyn RuntimeStore>, Arc<dyn ArtifactStore> and a RunScheduler. It exposes:

~~~rust
pub struct CreateTaskRequest {
    pub session_id: SessionId,
    pub goal: String,
    pub acceptance_criteria: Vec<String>,
}

pub struct SendMessageRequest {
    pub session_id: SessionId,
    pub focused_task_id: Option<TaskId>,
    pub text: String,
}

#[async_trait]
pub trait AgentServiceApi: Send + Sync {
    async fn create_session(&self, principal: Principal) -> CoreResult<Session>;
    async fn create_task(&self, request: CreateTaskRequest) -> CoreResult<Task>;
    async fn send_message(&self, request: SendMessageRequest) -> CoreResult<ServiceReply>;
    async fn resume_task(&self, task_id: &TaskId) -> CoreResult<RunId>;
    async fn answer_clarification(
        &self,
        run_id: &RunId,
        answer: String,
    ) -> CoreResult<()>;
    async fn cancel_run(&self, run_id: &RunId, reason: String) -> CoreResult<()>;
    async fn list_tasks(&self, workspace_id: &WorkspaceId) -> CoreResult<Vec<Task>>;
    async fn get_task(&self, task_id: &TaskId) -> CoreResult<Task>;
    async fn get_run(&self, run_id: &RunId) -> CoreResult<RunSnapshot>;
    async fn get_artifact(&self, artifact_id: &ArtifactId) -> CoreResult<ArtifactMetadata>;
    async fn subscribe_events(
        &self,
        run_id: &RunId,
        after_sequence: u64,
    ) -> CoreResult<EventSubscription>;
}
~~~

Do not expose QueryWorkflow or concrete Store types through this API.

- [ ] **Step 4: Implement a bounded Coordinator contract**

CoordinationDecision contains ContinueCurrentTask, CreateNewTask, CreateChildTask, CreateChangeRequest and RequestClarification.

v0.2 uses deterministic rules before any model classifier:

1. no focused Task means CreateNewTask;
2. explicit /task new means CreateNewTask;
3. an active Query Task plus a short follow-up uses ContinueCurrentTask;
4. any requested mutation from a Principal without ChangePrepare becomes CreateChangeRequest;
5. unsupported Workflow types return a clear v0.2 capability message rather than pretending to execute.

Put model-assisted routing behind a CoordinatorClassifier port, but use RuleBasedCoordinatorClassifier in v0.2 tests and default local configuration.

- [ ] **Step 5: Add a live Event subscription without making it authoritative**

AgentService owns a tokio broadcast channel of ServiceEvent for live TUI updates. A subscriber first loads durable Events from RuntimeStore, then listens to the broadcast channel. Lagged broadcast receivers reload from the durable Event sequence instead of losing state.

- [ ] **Step 6: Pass service tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-runtime --test service_test
rtk cargo clippy -p ys-agent-runtime --all-targets -- -D warnings
~~~

Expected: Session and Task lifecycle tests pass without a model or data source.

- [ ] **Step 7: Commit**

~~~bash
rtk git add crates/ys-agent-runtime
rtk git commit -m "feat(runtime): add agent service and coordinator"
~~~

---

## Task 6: Implement the ModelProvider boundary and OpenAI-compatible Tool Calling

**Files:**

- Create: crates/ys-agent-adapters/src/model/mod.rs
- Create: crates/ys-agent-adapters/src/model/openai_compatible.rs
- Create: crates/ys-agent-adapters/src/model/fake.rs
- Create: crates/ys-agent-adapters/src/model/replay.rs
- Modify: crates/ys-agent-adapters/src/lib.rs
- Modify: crates/ys-agent-adapters/Cargo.toml
- Test: crates/ys-agent-adapters/tests/model_provider_test.rs

- [ ] **Step 1: Write failing Tool Calling compatibility tests**

Create model_provider_test.rs with Wiremock:

~~~rust
#[tokio::test]
async fn converts_an_openai_compatible_tool_call_to_agent_action() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "inspect_schema",
                            "arguments": "{\"source_id\":\"warehouse\"}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 12,
                "total_tokens": 112
            }
        })))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let response = provider.complete(model_request_with_schema_tool()).await.unwrap();

    assert!(matches!(
        response.action,
        AgentAction::CallTool(ref call) if call.name == "inspect_schema"
    ));
    assert_eq!(response.usage.total_tokens, Some(112));
}

#[tokio::test]
async fn rejects_a_provider_profile_without_tool_calling() {
    let error = OpenAiCompatibleProvider::new(config_without_tool_calls())
        .expect_err("tool calling is required");
    assert!(matches!(error, CoreError::UnsupportedCapability(_)));
}

#[tokio::test]
async fn replay_provider_never_uses_the_network() {
    let provider = ReplayModelProvider::from_responses(vec![
        ModelResponse::clarification("Use seven complete days?"),
    ]);
    let response = provider.complete(empty_request()).await.unwrap();
    assert!(matches!(response.action, AgentAction::RequestClarification { .. }));
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-adapters --test model_provider_test
~~~

Expected: FAIL because providers are missing.

- [ ] **Step 3: Implement ProviderConfig and capability validation**

ProviderConfig contains:

~~~rust
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub model: String,
    pub supports_tool_calls: bool,
    pub supports_tool_call_ids: bool,
    pub supports_multi_turn_tool_results: bool,
    pub request_timeout: Duration,
}
~~~

Do not implement Serialize for SecretString. Its only Debug representation is [REDACTED]. validate() rejects empty URLs, empty model names and any missing required capability. Parallel Tool Calls, token streaming and provider-specific reasoning parameters remain disabled in v0.2.

- [ ] **Step 4: Implement request conversion**

Convert ModelRequest tools into OpenAI-compatible function Tool definitions. Convert Assistant Tool Calls to AgentAction::CallTool. Convert Tool Outcome messages back using the original tool_call_id. Parse function arguments with serde_path_to_error so failures identify the invalid field.

The adapter must never parse a Tool Call out of free-form assistant prose.

- [ ] **Step 5: Normalize failures**

Map HTTP and protocol failures to:

| Failure | Retryability |
|---|---|
| 408, 429, 502, 503, 504 | retry same request with bounded backoff |
| 401, 403 | non-retryable authentication/authorization |
| invalid Tool arguments | model-revision allowed, not transport retry |
| empty choices or missing action | invalid model response |
| timeout with no response | retryable only before any Tool side effect |

Record provider name, model, latency and token usage without logging api_key or raw sensitive Context.

- [ ] **Step 6: Implement Fake and Replay providers**

FakeModelProvider accepts an async closure from ModelRequest to ModelResponse. ReplayModelProvider returns a queued sequence and fails with ReplayExhausted when no response remains. Both report the same required ModelCapabilities as the production adapter.

- [ ] **Step 7: Pass provider tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-adapters --test model_provider_test
rtk cargo clippy -p ys-agent-adapters --all-targets -- -D warnings
~~~

Expected: Tool Call conversion, capability rejection and offline replay tests pass.

- [ ] **Step 8: Commit**

~~~bash
rtk git add crates/ys-agent-adapters
rtk git commit -m "feat(adapters): add openai compatible model provider"
~~~

---

## Task 7: Implement ToolCatalog, ToolView and governed Tool Runtime

**Files:**

- Create: crates/ys-agent-runtime/src/tools/mod.rs
- Create: crates/ys-agent-runtime/src/tools/catalog.rs
- Create: crates/ys-agent-runtime/src/tools/view.rs
- Create: crates/ys-agent-runtime/src/tools/runtime.rs
- Modify: crates/ys-agent-runtime/src/lib.rs
- Test: crates/ys-agent-runtime/tests/tool_runtime_test.rs

- [ ] **Step 1: Write failing catalog, visibility and retry tests**

Create tool_runtime_test.rs:

~~~rust
#[test]
fn duplicate_tool_names_are_rejected() {
    let mut catalog = ToolCatalog::new();
    catalog.register(read_only_tool("inspect_schema")).unwrap();
    let error = catalog.register(read_only_tool("inspect_schema")).unwrap_err();
    assert!(matches!(error, CoreError::DuplicateTool(_)));
}

#[test]
fn query_view_never_exposes_change_tools_to_business_users() {
    let catalog = catalog_with_query_and_build_tools();
    let principal = Principal::business_user("product");
    let view = ToolViewBuilder::new(&catalog)
        .for_workflow(WorkflowKind::Query)
        .for_principal(&principal)
        .build()
        .unwrap();

    assert!(view.contains("query_data"));
    assert!(!view.contains("apply_patch"));
    assert!(!view.contains("backfill_partition"));
}

#[tokio::test]
async fn runtime_retries_only_a_safe_transient_read() {
    let tool = transient_once_then_success_tool();
    let runtime = ToolRuntime::with_max_same_call_retries(1);
    let outcome = runtime.execute(tool, safe_context(), json!({})).await;
    assert!(matches!(outcome, ToolOutcome::Succeeded { .. }));
}

#[tokio::test]
async fn runtime_never_retries_an_indeterminate_side_effect() {
    let tool = indeterminate_write_tool();
    let runtime = ToolRuntime::with_max_same_call_retries(3);
    let outcome = runtime.execute(tool.clone(), write_context(), json!({})).await;
    assert!(matches!(outcome, ToolOutcome::Indeterminate { .. }));
    assert_eq!(tool.call_count(), 1);
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-runtime --test tool_runtime_test
~~~

Expected: FAIL because Tool Runtime modules are missing.

- [ ] **Step 3: Implement ToolCatalog**

ToolCatalog stores Arc<dyn Tool> by stable name and immutable version. register validates:

- unique stable name;
- valid input and output JSON Schema shape;
- non-zero timeout;
- ProductionWrite or CodeWrite tools declare required permissions;
- an idempotent write declares an idempotency-key field.

The full catalog is never serialized into a ModelRequest.

- [ ] **Step 4: Implement ToolViewBuilder**

ToolViewBuilder filters on:

~~~text
Workflow allow-list
∩ Principal capabilities
∩ Connector capability availability
∩ Run state
∩ Workspace policy
~~~

The resulting ToolView has a deterministic content hash. Store that hash in ContextManifest and ModelRequested Events.

The v0.2 Query allow-list is exactly:

~~~text
resolve_metric
inspect_schema
read_freshness
query_data
~~~

- [ ] **Step 5: Implement Tool Runtime preflight**

Before calling Tool::execute:

1. validate JSON arguments;
2. confirm the Tool is in the supplied ToolView;
3. calculate effective permissions;
4. evaluate risk and side effect;
5. reject every write in v0.2;
6. emit PolicyEvaluated and ToolExecutionStarted;
7. enforce timeout;
8. normalize output to ToolOutcome;
9. validate success output Schema;
10. emit a terminal Tool event.

- [ ] **Step 6: Implement retry ownership**

Tool Runtime retries only when all conditions are true:

- SideEffect::None;
- ToolFailure.retryability is SameCall;
- identical arguments;
- no output or remote handle was observed;
- retry count is within the configured bound.

ParameterRevision returns Failed with parameter_revision_allowed = true so Agent Loop can propose a new ToolCall. Indeterminate always returns immediately.

- [ ] **Step 7: Pass Tool Runtime tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-runtime --test tool_runtime_test
rtk cargo clippy -p ys-agent-runtime --all-targets -- -D warnings
~~~

Expected: catalog uniqueness, least privilege and retry ownership tests pass.

- [ ] **Step 8: Commit**

~~~bash
rtk git add crates/ys-agent-runtime/src/tools crates/ys-agent-runtime/tests/tool_runtime_test.rs
rtk git commit -m "feat(runtime): govern tool catalog visibility and execution"
~~~

---

## Task 8: Migrate SQLite and add capability-based Postgres connectors

**Files:**

- Create: crates/ys-agent-adapters/src/data/mod.rs
- Create: crates/ys-agent-adapters/src/data/sqlite.rs
- Create: crates/ys-agent-adapters/src/data/postgres.rs
- Create: crates/ys-agent-adapters/src/data/sql_policy.rs
- Create: crates/ys-agent-adapters/tests/sqlite_connector_test.rs
- Create: crates/ys-agent-adapters/tests/postgres_connector_test.rs
- Create: fixtures/postgres/compose.yaml
- Create: fixtures/sql/sqlite_seed.sql
- Create: fixtures/sql/postgres_seed.sql
- Modify: crates/ys-agent-adapters/Cargo.toml
- Remove after migration: apps/ysda/src/schema.rs
- Remove after migration: apps/ysda/src/executor.rs
- Remove after migration: apps/ysda/src/sqlite.rs
- Remove after migration: apps/ysda/src/policy.rs

- [ ] **Step 1: Write connector contract tests against SQLite**

Create sqlite_connector_test.rs:

~~~rust
#[tokio::test]
async fn sqlite_advertises_only_implemented_capabilities() {
    let fixture = SqliteFixture::from_seed("fixtures/sql/sqlite_seed.sql").await;
    let descriptor = fixture.connector.capabilities();
    assert!(descriptor.catalog_reader);
    assert!(descriptor.sql_query_executor);
    assert!(descriptor.freshness_reader);
    assert!(!descriptor.mutation_executor);
    assert!(!descriptor.job_controller);
}

#[tokio::test]
async fn sqlite_is_physically_and_logically_read_only() {
    let fixture = SqliteFixture::from_seed("fixtures/sql/sqlite_seed.sql").await;
    let error = fixture.connector
        .query(QueryRequest::new("DELETE FROM orders"))
        .await
        .expect_err("writes must be rejected");
    assert!(matches!(error, CoreError::PolicyRejected(_)));
}

#[tokio::test]
async fn sqlite_catalog_returns_observed_not_inferred_schema() {
    let fixture = SqliteFixture::from_seed("fixtures/sql/sqlite_seed.sql").await;
    let schema = fixture.connector.inspect_catalog().await.unwrap();
    assert_eq!(schema.kind, SchemaKnowledgeKind::Observed);
}
~~~

- [ ] **Step 2: Run SQLite connector tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-adapters --test sqlite_connector_test
~~~

Expected: FAIL because the migrated connector is missing.

- [ ] **Step 3: Move v0.1 SQLite behavior behind small ports**

SqliteConnector implements CatalogReader, SqlQueryExecutor and FreshnessReader. Reuse the existing read-only OpenFlags, query_only PRAGMA, AST policy and hard row limit.

Replace SQLite-specific domain types with core ObservedSchema and QueryResult. Preserve typed CellValue and truncation. Keep the concrete file path inside adapter configuration and out of persisted Events.

- [ ] **Step 4: Generalize SQL policy by dialect**

SqlReadOnlyPolicy accepts an explicit SupportedDialect enum with SQLite and Postgres. It must:

- parse exactly one statement;
- allow only Statement::Query;
- reject dialect-specific control statements;
- enforce max SQL bytes;
- return structured PolicyDecision reasons.

The physical read-only connection is defense in depth and remains mandatory.

- [ ] **Step 5: Write the ignored Postgres integration test**

postgres_connector_test.rs uses YSDA_TEST_POSTGRES_URL and is marked ignored with the reason requires fixtures/postgres/compose.yaml. It must inspect the public.orders table, run a bounded SELECT, read freshness and prove DELETE is rejected before reaching the server.

fixtures/postgres/compose.yaml:

~~~yaml
services:
  postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_USER: ysda
      POSTGRES_PASSWORD: ysda-test
      POSTGRES_DB: ysda_test
    ports:
      - "55432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U ysda -d ysda_test"]
      interval: 2s
      timeout: 2s
      retries: 30
    volumes:
      - ../sql/postgres_seed.sql:/docker-entrypoint-initdb.d/001-seed.sql:ro
~~~

- [ ] **Step 6: Implement PostgresConnector**

Use SQLx PgPool with:

- configurable max connections;
- statement timeout;
- acquire timeout;
- application_name = ysda;
- read-only transaction for every QueryRequest;
- max row count and max serialized byte count.

CatalogReader queries pg_catalog for schemas, tables, columns, nullability and primary keys. FreshnessReader executes MAX on the Metric Contract freshness column using quoted identifiers produced by the adapter, never raw model interpolation.

Convert common Postgres values to CellValue using column Type:

~~~text
BOOL       → Boolean
INT2/4/8   → Integer
FLOAT4/8   → Real
NUMERIC    → Text preserving exact decimal representation
TEXT/VARCHAR/NAME/UUID/DATE/TIMESTAMP/TIMESTAMPTZ → Text
BYTEA      → BlobSummary
unsupported type → Text Debug representation plus a conversion warning
~~~

- [ ] **Step 7: Run connector tests**

Run deterministic SQLite tests:

~~~bash
rtk cargo test -p ys-agent-adapters --test sqlite_connector_test
~~~

Run Postgres integration:

~~~bash
rtk docker compose -f fixtures/postgres/compose.yaml up -d --wait
rtk env YSDA_TEST_POSTGRES_URL=postgres://ysda:ysda-test@127.0.0.1:55432/ysda_test cargo test -p ys-agent-adapters --test postgres_connector_test -- --ignored
rtk docker compose -f fixtures/postgres/compose.yaml down -v
~~~

Expected: both connectors expose the same small core ports and reject writes.

- [ ] **Step 8: Remove migrated v0.1 modules and pass application tests**

Update app imports to use SqliteConnector and SqlReadOnlyPolicy from ys-agent-adapters. Delete the four migrated files only after all their tests exist in the adapter crate.

Run:

~~~bash
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

- [ ] **Step 9: Commit**

~~~bash
rtk git add apps/ysda crates/ys-agent-adapters fixtures
rtk git commit -m "feat(adapters): add sqlite and postgres query capabilities"
~~~

---

## Task 9: Add the Metric Registry, dbt Context Adapter and Context Assembler

**Files:**

- Create: crates/ys-agent-adapters/src/context/mod.rs
- Create: crates/ys-agent-adapters/src/context/metric_registry.rs
- Create: crates/ys-agent-adapters/src/context/dbt_manifest.rs
- Create: crates/ys-agent-runtime/src/context_assembler.rs
- Create: crates/ys-agent-adapters/tests/context_adapter_test.rs
- Create: fixtures/metrics/metrics.json
- Create: fixtures/dbt/manifest.json
- Modify: crates/ys-agent-adapters/src/lib.rs
- Modify: crates/ys-agent-runtime/src/lib.rs

- [ ] **Step 1: Write failing governance and retrieval tests**

Create context_adapter_test.rs:

~~~rust
#[tokio::test]
async fn only_active_metrics_are_queryable_by_default() {
    let registry = FileMetricRegistry::load("fixtures/metrics/metrics.json").await.unwrap();
    assert!(registry.resolve_active("commerce.gmv").await.unwrap().is_some());
    assert!(registry.resolve_active("commerce.gmv_draft").await.unwrap().is_none());
}

#[tokio::test]
async fn dbt_manifest_evidence_keeps_provenance_and_hash() {
    let adapter = DbtManifestAdapter::load("fixtures/dbt/manifest.json").await.unwrap();
    let evidence = adapter.find_model("model.shop.mart_orders").await.unwrap();
    assert_eq!(evidence.source_type, ContextSourceType::DbtManifest);
    assert!(!evidence.version.is_empty());
    assert_eq!(evidence.knowledge_kind, SchemaKnowledgeKind::Observed);
}

#[tokio::test]
async fn context_assembler_records_omitted_large_evidence() {
    let fixture = ContextFixture::with_budget(400);
    let pack = fixture.assembler.for_query("GMV", &fixture.tool_view()).await.unwrap();
    assert!(pack.manifest.used_tokens <= 400);
    assert!(!pack.manifest.omitted.is_empty());
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-adapters --test context_adapter_test
~~~

Expected: FAIL because context adapters are missing.

- [ ] **Step 3: Create the minimal Metric Registry fixture**

fixtures/metrics/metrics.json:

~~~json
{
  "schema_version": 1,
  "metrics": [
    {
      "id": "commerce.gmv",
      "version": 1,
      "status": "active",
      "description": "Paid order amount excluding cancelled orders",
      "source_relation": "mart_orders",
      "expression": "SUM(paid_amount)",
      "time_column": "paid_at",
      "allowed_dimensions": ["country", "channel"],
      "owner": "data-team",
      "freshness_sla_seconds": 86400
    },
    {
      "id": "commerce.gmv_draft",
      "version": 1,
      "status": "draft",
      "description": "Unapproved candidate",
      "source_relation": "mart_orders",
      "expression": "SUM(gross_amount)",
      "time_column": "paid_at",
      "allowed_dimensions": [],
      "owner": "data-team",
      "freshness_sla_seconds": 86400
    }
  ]
}
~~~

- [ ] **Step 4: Implement FileMetricRegistry**

Load the entire file, reject unknown schema_version, duplicate id/version pairs, missing owners, unsafe source identifiers and empty expressions. Resolve by exact id first, then case-insensitive display alias. resolve_active never returns Draft or Deprecated.

The loader may propose no writes. Publishing Draft to Active is outside v0.2.

- [ ] **Step 5: Implement DbtManifestAdapter**

Parse only the manifest fields v0.2 needs:

- metadata.dbt_schema_version;
- metadata.generated_at;
- nodes unique_id, resource_type, database, schema, name, alias, description, columns, depends_on and checksum;
- sources with the same identity and column subset.

Store original manifest content hash as Evidence version. Do not invent descriptions or relationships.

- [ ] **Step 6: Implement deterministic Context Assembler**

ContextAssembler receives Task goal, Query Workflow state, ToolView and a token budget. It ranks:

1. exact Active Metric match;
2. dbt model referenced by that metric;
3. observed Schema for the source relation;
4. freshness observation;
5. recent Task summary only when explicitly relevant.

Use a deterministic byte-to-token estimate for v0.2 and record every omission. Do not add embeddings or a vector database.

- [ ] **Step 7: Add a ContextManifest Artifact**

Before each ModelRequest, serialize ContextManifest through ArtifactStore, then persist its ArtifactId in ModelRequested. The Prompt contains selected Evidence text; it never contains the complete manifest database or raw credentials.

- [ ] **Step 8: Pass context tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-adapters --test context_adapter_test
rtk cargo test -p ys-agent-runtime context
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

Expected: Active governance, dbt provenance and budget omission tests pass.

- [ ] **Step 9: Commit**

~~~bash
rtk git add crates/ys-agent-adapters crates/ys-agent-runtime fixtures/metrics fixtures/dbt
rtk git commit -m "feat(context): add governed metric and dbt context"
~~~

---

## Task 10: Implement the four Query tools and deterministic metric compilation

**Files:**

- Create: crates/ys-agent-adapters/src/tools/mod.rs
- Create: crates/ys-agent-adapters/src/tools/resolve_metric.rs
- Create: crates/ys-agent-adapters/src/tools/inspect_schema.rs
- Create: crates/ys-agent-adapters/src/tools/read_freshness.rs
- Create: crates/ys-agent-adapters/src/tools/query_data.rs
- Create: crates/ys-agent-adapters/tests/query_tools_test.rs
- Modify: crates/ys-agent-adapters/src/lib.rs
- Modify: crates/ys-agent-adapters/Cargo.toml

- [ ] **Step 1: Write failing end-to-end Tool tests**

Create query_tools_test.rs:

~~~rust
#[tokio::test]
async fn resolve_metric_returns_only_the_active_contract() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture.call("resolve_metric", json!({
        "metric": "commerce.gmv"
    })).await;

    let output = outcome.success_json().expect("success");
    assert_eq!(output["status"], "active");
    assert_eq!(output["source_relation"], "mart_orders");
}

#[tokio::test]
async fn metric_query_is_compiled_from_the_contract_not_free_form_sql() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture.call("query_data", json!({
        "source_id": "sqlite-demo",
        "query": {
            "kind": "metric",
            "metric_id": "commerce.gmv",
            "start": "2026-08-01T00:00:00Z",
            "end": "2026-08-08T00:00:00Z",
            "dimensions": []
        }
    })).await;

    let output = outcome.success_json().expect("success");
    assert_eq!(output["semantic_status"], "confirmed");
    assert_eq!(output["metric_id"], "commerce.gmv");
    assert!(output["executed_sql"].as_str().unwrap().contains("SUM(paid_amount)"));
}

#[tokio::test]
async fn an_unapproved_dimension_is_rejected_before_sql_execution() {
    let fixture = QueryToolFixture::sqlite().await;
    let outcome = fixture.call("query_data", json!({
        "source_id": "sqlite-demo",
        "query": {
            "kind": "metric",
            "metric_id": "commerce.gmv",
            "start": "2026-08-01T00:00:00Z",
            "end": "2026-08-08T00:00:00Z",
            "dimensions": ["card_number"]
        }
    })).await;

    assert!(matches!(outcome, ToolOutcome::Rejected { .. }));
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-adapters --test query_tools_test
~~~

Expected: FAIL because Query tools are missing.

- [ ] **Step 3: Implement ResolveMetricTool**

Input:

~~~json
{
  "metric": "commerce.gmv"
}
~~~

Output includes exact id, version, status, description, source_relation, expression, time_column, allowed_dimensions, owner and freshness SLA. A missing or non-Active metric returns a structured non-retryable ToolFailure with category metric_not_found_or_inactive.

- [ ] **Step 4: Implement InspectSchemaTool**

Input contains source_id and an optional exact relation list. The tool resolves a Connector internally and returns only ObservedSchema. It enforces a maximum relation and column count and returns Artifact references when the response exceeds the Tool output budget.

The model never receives connection URLs or credentials.

- [ ] **Step 5: Implement ReadFreshnessTool**

Input contains source_id, relation and an approved time column. The tool checks that the column came from MetricDefinition or ObservedSchema, then asks FreshnessReader for MAX(time_column). Output contains observed_at, latest_data_at, age_seconds, SLA and is_fresh.

Never let the model supply a raw SQL freshness expression.

- [ ] **Step 6: Implement QueryDataTool input as a tagged union**

~~~rust
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueryPlan {
    Metric {
        metric_id: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        dimensions: Vec<String>,
    },
    AdHoc {
        sql: String,
        assumption_refs: Vec<ArtifactId>,
    },
}
~~~

Metric queries resolve an Active MetricDefinition and compile SQL deterministically. AdHoc queries pass through SqlReadOnlyPolicy and are marked semantic_status = inferred.

- [ ] **Step 7: Implement MetricSqlCompiler for SQLite and Postgres**

Compiler checks:

- start is strictly before end;
- requested dimensions are a subset of allowed_dimensions;
- relation, time column and dimension identifiers pass adapter quoting rules;
- expression comes only from the Active MetricDefinition;
- no user or model string is interpolated as an identifier;
- time values are bound parameters.

Generated shape:

~~~sql
SELECT
    <approved dimensions>,
    <metric expression> AS metric_value
FROM <approved relation>
WHERE <approved time column> >= <bound start>
  AND <approved time column> < <bound end>
GROUP BY <approved dimensions>
ORDER BY <approved dimensions>
~~~

Use half-open time intervals. SQLite placeholders are positional question marks; Postgres placeholders are dollar-numbered. Return sanitized SQL plus typed parameters as execution evidence.

- [ ] **Step 8: Register exact ToolSpec metadata**

All four v0.2 tools declare SideEffect::None. query_data declares a longer timeout and a strict maximum output byte size. ToolSpec versions begin at 1.0.0 and required_permissions contains DataQuery.

- [ ] **Step 9: Pass Query Tool tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-adapters --test query_tools_test
rtk cargo test -p ys-agent-adapters --test sqlite_connector_test
rtk cargo clippy -p ys-agent-adapters --all-targets --all-features -- -D warnings
~~~

Expected: Active metric, deterministic compilation and dimension rejection tests pass.

- [ ] **Step 10: Commit**

~~~bash
rtk git add crates/ys-agent-adapters
rtk git commit -m "feat(tools): add governed query capabilities"
~~~

---

## Task 11: Build the Harness, Agent Loop, Query Workflow and Completion Gate

**Files:**

- Create: crates/ys-agent-runtime/src/harness.rs
- Create: crates/ys-agent-runtime/src/loop_driver.rs
- Create: crates/ys-agent-runtime/src/workflow/mod.rs
- Create: crates/ys-agent-runtime/src/workflow/query/mod.rs
- Create: crates/ys-agent-runtime/src/workflow/query/state.rs
- Create: crates/ys-agent-runtime/src/workflow/query/prompts.rs
- Create: crates/ys-agent-runtime/src/workflow/query/verifier.rs
- Create: crates/ys-agent-runtime/src/workflow/query/artifact.rs
- Modify: crates/ys-agent-runtime/src/lib.rs
- Test: crates/ys-agent-runtime/tests/query_workflow_test.rs

- [ ] **Step 1: Write failing successful-loop and repair-loop tests**

Create query_workflow_test.rs:

~~~rust
#[tokio::test]
async fn query_completion_requires_execution_verification_and_artifact() {
    let fixture = QueryWorkflowFixture::successful_metric_query().await;
    let result = fixture.run("GMV for the last seven complete days").await.unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    let artifact = fixture.load_primary_query_artifact(&result).await;
    assert_eq!(artifact.metric.id, "commerce.gmv");
    assert!(!artifact.executed_sql.is_empty());
    assert!(artifact.verification.hard_failures.is_empty());
    assert!(artifact.freshness.is_some());
}

#[tokio::test]
async fn propose_completion_before_query_execution_is_rejected() {
    let fixture = QueryWorkflowFixture::with_model_actions(vec![
        ModelResponse::propose_completion("GMV is 10"),
    ]).await;

    let result = fixture.run("GMV").await.unwrap();

    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(result.failure_code(), Some("completion_gate_failed"));
}

#[tokio::test]
async fn invalid_sql_can_be_revised_by_the_model_without_transport_retry() {
    let fixture = QueryWorkflowFixture::with_model_actions(vec![
        call_query_data_with_unsafe_sql(),
        call_query_data_with_safe_sql(),
        propose_completion(),
    ]).await;

    let result = fixture.run("List order channels").await.unwrap();

    assert_eq!(result.status, RunStatus::Succeeded);
    assert_eq!(fixture.tool_call_count("query_data"), 2);
    assert_eq!(fixture.transport_retry_count(), 0);
}

#[tokio::test]
async fn material_metric_ambiguity_waits_for_user_input() {
    let fixture = QueryWorkflowFixture::with_ambiguous_metrics().await;
    let result = fixture.run("Show GMV recently").await.unwrap();
    assert_eq!(result.status, RunStatus::WaitingForInput);
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-runtime --test query_workflow_test
~~~

Expected: FAIL because Harness and Query Workflow are missing.

- [ ] **Step 3: Implement QueryWorkflowState**

Use explicit phases:

~~~rust
pub enum QueryPhase {
    Understand,
    ResolveMetric,
    InspectContext,
    Execute,
    Verify,
    ReadyToComplete,
}

pub struct QueryWorkflowState {
    pub phase: QueryPhase,
    pub question: String,
    pub metric_evidence: Option<ArtifactRef>,
    pub schema_evidence: Vec<ArtifactRef>,
    pub freshness_evidence: Option<ArtifactRef>,
    pub query_result: Option<ArtifactRef>,
    pub verification_report: Option<ArtifactRef>,
    pub assumptions: Vec<String>,
    pub warnings: Vec<String>,
}
~~~

Each transition validates required evidence. Workflow state is part of RunSnapshot.

- [ ] **Step 4: Implement LoopDriver budgets**

LoopBudget contains max_steps, max_model_calls, max_tool_calls, max_total_tokens and deadline. Defaults for v0.2:

~~~text
max_steps: 24
max_model_calls: 12
max_tool_calls: 16
max_total_tokens: 64,000
deadline: 10 minutes
~~~

Every limit produces a typed terminal failure and persists the final Event. WaitingForInput stops the deadline clock and makes no further model calls.

- [ ] **Step 5: Implement one Harness step**

Harness::step performs exactly one state transition:

1. load RunSnapshot and version;
2. ask Workflow for permitted next behavior;
3. assemble ContextManifest and ToolView;
4. persist StepStarted and ModelRequested before external model I/O;
5. call ModelProvider;
6. persist ModelResponded;
7. validate AgentAction against Workflow phase;
8. execute at most one ToolCall;
9. persist Tool events, Artifact metadata and new Snapshot atomically;
10. return Continue, Wait or Terminal.

The outer LoopDriver repeatedly calls step. This keeps crash boundaries testable.

- [ ] **Step 6: Implement Query system instructions**

The Prompt must state:

- use tools for facts;
- prefer Active Metric contracts;
- request clarification for material ambiguity;
- never invent source names, schema, freshness or results;
- ProposeCompletion only after query result and verification evidence;
- do not expose internal chain-of-thought;
- report assumptions and warnings.

Prompt version is query-system-v1 and is included in ModelRequested metadata.

- [ ] **Step 7: Implement QueryVerifier**

QueryVerifier is deterministic and returns VerificationReport:

~~~rust
pub struct VerificationReport {
    pub checks: Vec<VerificationCheck>,
    pub hard_failures: Vec<String>,
    pub warnings: Vec<String>,
    pub evidence_refs: Vec<ArtifactRef>,
}
~~~

Hard checks:

- PolicyDecision allowed;
- executed result evidence exists;
- requested time range equals compiled range;
- Active metric id/version equals the compiler input when a metric is used;
- executed relation matches MetricDefinition;
- DataQuery permission was present;
- result columns match the declared result Schema;
- answer number references the execution Artifact.

Warnings:

- freshness SLA failed;
- AdHoc semantic status is inferred;
- result was truncated;
- result is empty or entirely null;
- unconfirmed assumptions remain.

Hard failure blocks completion. Warnings are rendered in QueryArtifact.

- [ ] **Step 8: Implement QueryArtifact**

QueryArtifact contains:

~~~rust
pub struct QueryArtifact {
    pub question: String,
    pub metric: Option<MetricReference>,
    pub semantic_status: SemanticStatus,
    pub source_id: SourceId,
    pub source_relations: Vec<String>,
    pub time_range: Option<TimeRange>,
    pub executed_sql: String,
    pub bound_parameters: Vec<RedactedParameter>,
    pub result_schema: ResultSchema,
    pub result_artifact: ArtifactRef,
    pub freshness: Option<FreshnessObservation>,
    pub verification: VerificationReport,
    pub assumptions: Vec<String>,
    pub generated_at: DateTime<Utc>,
}
~~~

Store small result bodies as JSON Artifacts. Enforce v0.2 row and byte limits; do not add Arrow or Parquet yet.

- [ ] **Step 9: Wire ProposeCompletion to the Completion Gate**

On ProposeCompletion:

1. run QueryVerifier;
2. persist VerificationReport;
3. reject and continue only when failures are model-revisable and budget remains;
4. persist QueryArtifact;
5. append ArtifactCreated and RunCompleted in one store transaction;
6. set primary_artifact_id.

The assistant's prose never becomes the only persisted result.

- [ ] **Step 10: Pass Query Workflow tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-runtime --test query_workflow_test
rtk cargo clippy -p ys-agent-runtime --all-targets --all-features -- -D warnings
~~~

Expected: complete, premature completion, SQL repair and clarification paths pass with Fake/Replay providers.

- [ ] **Step 11: Commit**

~~~bash
rtk git add crates/ys-agent-runtime
rtk git commit -m "feat(runtime): execute and verify query workflows"
~~~

---

## Task 12: Add durable recovery and clarification resume

**Files:**

- Create: crates/ys-agent-runtime/src/recovery.rs
- Modify: crates/ys-agent-runtime/src/harness.rs
- Modify: crates/ys-agent-runtime/src/service.rs
- Modify: crates/ys-agent-runtime/src/lib.rs
- Test: crates/ys-agent-runtime/tests/recovery_test.rs

- [ ] **Step 1: Write failing process-restart tests**

Create recovery_test.rs:

~~~rust
#[tokio::test]
async fn waiting_for_input_resumes_the_same_run_after_store_reopen() {
    let fixture = PersistentRuntimeFixture::new().await;
    let first = fixture.run_until_clarification("Show GMV recently").await;
    let original_run_id = first.id.clone();
    drop(fixture.runtime);

    let reopened = fixture.reopen().await;
    reopened.service
        .answer_clarification(&original_run_id, "Use seven complete days".to_owned())
        .await
        .unwrap();
    let completed = reopened.run_to_terminal(&original_run_id).await;

    assert_eq!(completed.id, original_run_id);
    assert_eq!(completed.status, RunStatus::Succeeded);
}

#[tokio::test]
async fn started_read_tool_without_terminal_event_becomes_unknown_then_new_call() {
    let fixture = PersistentRuntimeFixture::crash_after_tool_started().await;
    let original_call = fixture.original_tool_call_id();
    let reopened = fixture.reopen().await;

    let completed = reopened.resume_to_terminal().await;

    assert_eq!(completed.status, RunStatus::Succeeded);
    assert!(reopened.has_indeterminate_event(&original_call).await);
    assert_ne!(reopened.successful_tool_call_id(), original_call);
}

#[tokio::test]
async fn a_terminal_failed_run_is_never_resumed_in_place() {
    let fixture = PersistentRuntimeFixture::failed_run().await;
    let new_run = fixture.service.resume_task(&fixture.task_id).await.unwrap();
    assert_ne!(new_run, fixture.failed_run_id);
}
~~~

- [ ] **Step 2: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ys-agent-runtime --test recovery_test
~~~

Expected: FAIL because recovery rules are missing.

- [ ] **Step 3: Implement the recovery decision table**

~~~text
Last durable state                         Recovery action
Queued                                    start same Run
Running, no external operation pending    continue same Run
WaitingForInput                           wait; resume same Run after answer
WaitingForApproval                        wait; v0.2 cannot approve write action
WaitingForExecution                       preserve handle; v0.2 reports unsupported backend
ModelRequested without ModelResponded     issue a new model_call_id in same Run
ToolExecutionStarted without terminal     append Indeterminate
Indeterminate read-only Tool              create new ToolCall after explicit resume
Succeeded/Cancelled                       return terminal state
Failed                                    create a new Run with retry_of_run_id
~~~

- [ ] **Step 4: Implement event projection**

Recovery reconstructs RunSnapshot by applying Events in sequence when Snapshot is missing or its version does not equal the last Event sequence. Reject gaps, duplicate sequence numbers and unknown future schema versions with a CorruptRunHistory error.

- [ ] **Step 5: Implement clarification answers as Events**

answer_clarification:

1. loads a WaitingForInput Run;
2. validates the pending clarification id;
3. writes the answer as a sensitive Artifact when needed;
4. appends ClarificationAnswered and RunResumed;
5. updates the same Run Snapshot;
6. schedules LoopDriver.

Do not append the entire Session transcript to the model Context.

- [ ] **Step 6: Pass recovery tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-runtime --test recovery_test
rtk cargo test -p ys-agent-store
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

Expected: same-Run clarification resume, indeterminate read recovery and new-Run retry tests pass.

- [ ] **Step 7: Commit**

~~~bash
rtk git add crates/ys-agent-runtime
rtk git commit -m "feat(runtime): resume durable query runs"
~~~

---

## Task 13: Replace the v0.1 command shell with the interactive TUI

**Files:**

- Create: apps/ysda/src/bootstrap.rs
- Replace: apps/ysda/src/cli.rs
- Replace: apps/ysda/src/lib.rs
- Replace: apps/ysda/src/main.rs
- Create: apps/ysda/src/tui/mod.rs
- Create: apps/ysda/src/tui/app.rs
- Create: apps/ysda/src/tui/event_loop.rs
- Create: apps/ysda/src/tui/input.rs
- Create: apps/ysda/src/tui/ui.rs
- Create: apps/ysda/tests/tui_test.rs
- Modify: apps/ysda/tests/cli_test.rs
- Modify: apps/ysda/Cargo.toml
- Remove after replacement: apps/ysda/src/agent.rs
- Remove after replacement: apps/ysda/src/domain.rs
- Remove after replacement: apps/ysda/src/error.rs
- Remove after replacement: apps/ysda/src/llm.rs
- Remove after replacement: apps/ysda/src/trace.rs
- Remove after replacement: apps/ysda/src/output.rs
- Remove after migration: apps/ysda/tests/agent_test.rs
- Remove after migration: apps/ysda/tests/domain_test.rs
- Remove after migration: apps/ysda/tests/executor_test.rs
- Remove after migration: apps/ysda/tests/llm_test.rs
- Remove after migration: apps/ysda/tests/output_test.rs
- Remove after migration: apps/ysda/tests/policy_test.rs
- Remove after migration: apps/ysda/tests/schema_test.rs
- Remove after migration: apps/ysda/tests/trace_test.rs
- Remove after migration: apps/ysda/tests/support/mod.rs

- [ ] **Step 1: Write failing CLI parsing tests**

Replace cli_test.rs with:

~~~rust
use clap::Parser;
use ysda::cli::{Cli, Command, TaskCommand};

#[test]
fn no_subcommand_selects_interactive_tui() {
    let cli = Cli::try_parse_from(["ysda"]).unwrap();
    assert!(cli.command.is_none());
}

#[test]
fn parses_non_interactive_run() {
    let cli = Cli::try_parse_from(["ysda", "run", "last seven days GMV"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Run { question }) if question == "last seven days GMV"
    ));
}

#[test]
fn parses_task_resume() {
    let cli = Cli::try_parse_from([
        "ysda",
        "task",
        "resume",
        "3d315500-ec47-4ce3-83ee-4284ec34cdbc",
    ]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::Task {
            command: TaskCommand::Resume { .. }
        })
    ));
}
~~~

- [ ] **Step 2: Write failing TUI rendering and command tests**

Create tui_test.rs:

~~~rust
#[test]
fn welcome_screen_shows_workspace_connection_and_permission() {
    let app = TuiApp::test_home(
        "ecommerce",
        "postgres-prod",
        "read-only",
        "openai-compatible/test-model",
    );
    let rendered = render_to_string(&app, 100, 28);

    assert!(rendered.contains("YS DATA AGENT"));
    assert!(rendered.contains("ecommerce"));
    assert!(rendered.contains("postgres-prod"));
    assert!(rendered.contains("read-only"));
}

#[test]
fn slash_new_creates_a_session_command_not_a_cancel_command() {
    let action = parse_input("/new").unwrap();
    assert_eq!(action, InputAction::NewSession);
    assert!(!matches!(action, InputAction::CancelRun { .. }));
}

#[test]
fn business_user_does_not_see_build_mode() {
    let app = TuiApp::for_principal(Principal::business_user("product"));
    let rendered = render_to_string(&app, 100, 28);
    assert!(!rendered.contains("Build mode"));
}
~~~

- [ ] **Step 3: Run tests to verify they fail**

Run:

~~~bash
rtk cargo test -p ysda --test cli_test
rtk cargo test -p ysda --test tui_test
~~~

Expected: FAIL because the new CLI and TUI are missing.

- [ ] **Step 4: Implement optional Clap subcommands**

~~~rust
#[derive(Debug, Parser)]
#[command(name = "ysda", version, about = "YS Data Agent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Run { question: String },
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    Artifact { artifact_id: ArtifactId },
    Schema { source_id: String },
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    List,
    Resume { task_id: TaskId },
    Cancel { run_id: RunId },
}
~~~

No subcommand launches TUI. Non-interactive commands use the same AgentService instance as TUI.

- [ ] **Step 5: Implement bootstrap without leaking concrete dependencies into UI**

bootstrap.rs loads local configuration and constructs:

~~~text
SqliteRuntimeStore at .ysda/runtime.db
LocalArtifactStore at .ysda/artifacts
FileMetricRegistry
optional DbtManifestAdapter
configured SQLite or Postgres Connector
OpenAiCompatibleProvider
ToolCatalog with four Query tools
Harness and LoopDriver
InProcessAgentService
~~~

Return AppDependencies containing Arc<dyn AgentServiceApi> and read-only display metadata. Never return raw database passwords or Model api_key to TUI state.

- [ ] **Step 6: Implement TuiApp as a pure view model**

~~~rust
pub struct TuiApp {
    pub workspace_name: String,
    pub principal_name: String,
    pub model_label: String,
    pub connection_label: String,
    pub permission_label: String,
    pub session_id: SessionId,
    pub focused_task: Option<TaskSummary>,
    pub transcript: Vec<TranscriptItem>,
    pub input: String,
    pub cursor: usize,
    pub scroll: u16,
    pub mode: UiMode,
    pub should_quit: bool,
}
~~~

TranscriptItem is a typed view model: UserMessage, AssistantMessage, StepStatus, ToolCall, Warning, Error and ArtifactLink. TUI must not render arbitrary terminal control sequences from model or Tool output.

- [ ] **Step 7: Implement the Ratatui layout**

Use three vertical regions:

~~~text
Header: logo or title, Workspace, model, connection, permission
Body: recent Tasks on empty Session; transcript and Run events when active
Footer: bordered input editor plus Task/Workflow/Run status line
~~~

Use Ratatui TestBackend in tests. Tool Calls default to one-line collapsed summaries. The TUI displays Workflow after routing; it never requires normal users to choose an Agent.

- [ ] **Step 8: Implement input and Slash Commands**

Supported v0.2 commands:

| Input | Action |
|---|---|
| /new | create a new Session |
| /tasks | list Workspace Tasks |
| /task new TEXT | create a Task |
| /resume TASK_ID | focus and resume a Task |
| /cancel RUN_ID | request explicit cancellation |
| /artifact ARTIFACT_ID | show Artifact metadata or safe body |
| /connections | show configured source capabilities |
| /model | show current provider and model |
| /help | show commands |
| /quit | exit TUI without cancelling Tasks |

All other non-empty input becomes AgentService::send_message.

- [ ] **Step 9: Implement the terminal Event Loop and restoration guard**

Use Crossterm event-stream with tokio::select over:

- keyboard events;
- AgentService event subscription;
- periodic redraw tick;
- Ctrl-C shutdown signal.

Enter alternate screen and raw mode through a TerminalGuard whose Drop implementation always restores cursor, raw mode and screen, including panic paths. /quit and Ctrl-C detach the UI; they do not cancel Task or Run.

- [ ] **Step 10: Render structured clarification**

When ServiceEvent::ClarificationRequested arrives:

- display the exact question;
- show recommended default and interpretations when supplied;
- set UiMode::Clarification;
- send the next answer through answer_clarification;
- never create a new Task for that answer.

Approval UI is represented by a disabled explanatory panel in v0.2 because no write Tool is registered. Do not implement fake approvals.

- [ ] **Step 11: Remove the replaced v0.1 orchestration**

After the new TUI and non-interactive Run path use AgentService:

- delete QueryAgent;
- delete AgentRun and string RunEvent;
- delete the v0.1 AppError wrapper after callers use typed Core errors;
- delete LlmClient;
- delete TraceRecorder;
- delete render_run.
- delete the migrated v0.1 integration tests listed in this Task.
- remove direct comfy-table, reqwest, rusqlite and sqlparser dependencies from apps/ysda; those dependencies belong to Adapter crates.

Keep no compatibility wrapper for v0.1 Trace JSON. Retain the v0.1 Git history and documents.

- [ ] **Step 12: Pass CLI and TUI tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ysda --test cli_test
rtk cargo test -p ysda --test tui_test
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

Expected: no-argument TUI, Slash Command semantics and full workspace tests pass.

- [ ] **Step 13: Manually smoke-test the terminal**

Run:

~~~bash
rtk cargo run -p ysda
~~~

Verify:

1. welcome view renders without wrapping at 100x28;
2. resizing does not panic;
3. /new changes Session ID;
4. /quit restores the terminal;
5. reopening lists the previous Task;
6. a Query displays Tool progress and QueryArtifact links.

- [ ] **Step 14: Commit**

~~~bash
rtk git add apps/ysda
rtk git commit -m "feat(tui): add interactive data agent interface"
~~~

---

## Task 14: Separate Telemetry and add the Query Eval release gate

**Files:**

- Create: crates/ys-agent-runtime/src/telemetry.rs
- Create: evals/query_cases.jsonl
- Create: evals/README.md
- Create: apps/ysda/tests/query_eval_test.rs
- Modify: crates/ys-agent-runtime/src/harness.rs
- Modify: crates/ys-agent-runtime/src/lib.rs
- Modify: apps/ysda/Cargo.toml

- [ ] **Step 1: Write a failing Telemetry isolation test**

Add to a new runtime telemetry test module:

~~~rust
#[tokio::test]
async fn telemetry_failure_never_rolls_back_a_persisted_event() {
    let fixture = RuntimeFixture::with_telemetry_sink(AlwaysFailTelemetrySink);
    let result = fixture.run_simple_query().await;

    assert_eq!(result.status, RunStatus::Succeeded);
    assert!(fixture.store.contains_event("run_completed").await);
    assert!(fixture.telemetry_failures() > 0);
}

#[tokio::test]
async fn telemetry_does_not_receive_query_result_rows() {
    let sink = RecordingTelemetrySink::new();
    let fixture = RuntimeFixture::with_telemetry_sink(sink.clone());
    fixture.run_simple_query().await;

    assert!(!sink.serialized_events().contains("secret_customer_name"));
}
~~~

- [ ] **Step 2: Implement TelemetrySink**

~~~rust
#[async_trait]
pub trait TelemetrySink: Send + Sync {
    async fn emit(&self, event: TelemetryEvent) -> Result<(), TelemetryError>;
}

pub enum TelemetryEvent {
    RunLatency { run_id: RunId, milliseconds: u64 },
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
~~~

Default TracingTelemetrySink writes structured tracing fields. Emit only after Runtime Store commits. Swallow and count sink failures. Do not include Prompt bodies, credentials or QueryResult rows.

- [ ] **Step 3: Create the deterministic Eval dataset**

evals/query_cases.jsonl contains at least these complete cases:

~~~jsonl
{"id":"metric_gmv_7d","question":"GMV for the last seven complete days","expected_metric":"commerce.gmv","expected_relation":"mart_orders","expected_status":"succeeded","expected_warning_codes":[]}
{"id":"metric_gmv_ambiguous_recent","question":"Show GMV recently","expected_status":"waiting_for_input","expected_clarification_contains":"time range"}
{"id":"unsafe_delete","question":"Delete all old orders","expected_status":"failed","expected_failure_code":"policy_rejected"}
{"id":"draft_metric","question":"Show commerce.gmv_draft","expected_status":"waiting_for_input","expected_clarification_contains":"active metric"}
{"id":"stale_metric_source","question":"Show GMV for the last seven complete days","fixture_variant":"stale","expected_status":"succeeded","expected_warning_codes":["freshness_sla_failed"]}
~~~

Every case declares an expected terminal or waiting state. No case relies on a live model.

- [ ] **Step 4: Implement the Eval harness**

apps/ysda/tests/query_eval_test.rs:

1. reads every JSONL line;
2. creates an isolated Runtime Store and SQLite fixture;
3. loads fixed Metric and dbt fixtures;
4. selects a ReplayModelProvider response sequence by case id;
5. runs the same AgentService and Harness used by the application;
6. asserts status, metric, relation, failure and warning expectations;
7. records Model, Prompt, Tool, Context, Workflow and Policy versions in EvalResult.

The test must fail when any dataset line is unparsed or unexecuted.

- [ ] **Step 5: Add trajectory assertions**

For successful Query cases assert:

- resolve_metric occurs before query_data;
- no Tool outside the Query ToolView is called;
- query_data success exists before ProposeCompletion;
- VerificationReport exists before RunCompleted;
- model calls do not exceed the case budget;
- exact ContextManifest and ToolView hashes are present.

- [ ] **Step 6: Pass Telemetry and Eval tests**

Run:

~~~bash
rtk cargo fmt --all
rtk cargo test -p ys-agent-runtime telemetry
rtk cargo test -p ysda --test query_eval_test
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

Expected: Telemetry failure isolation and every deterministic Query case pass.

- [ ] **Step 7: Document Eval extension rules**

evals/README.md must require:

- one regression case for every production bug;
- deterministic checks before LLM Judge;
- explicit version fields;
- no raw production data or credentials;
- review approval for expectation changes;
- no deleting a failing case merely to release.

- [ ] **Step 8: Commit**

~~~bash
rtk git add crates/ys-agent-runtime evals apps/ysda/tests/query_eval_test.rs apps/ysda/Cargo.toml
rtk git commit -m "test(eval): gate query runtime releases"
~~~

---

## Task 15: Complete end-to-end verification and v0.2 release documentation

**Files:**

- Create: scripts/v0.2-release-gate.sh
- Create: .env.example
- Modify: README.md
- Modify: apps/ysda/Cargo.toml
- Modify: Cargo.lock
- Verify removal: apps/ysda/src/agent.rs
- Verify removal: apps/ysda/src/domain.rs
- Verify removal: apps/ysda/src/llm.rs
- Verify removal: apps/ysda/src/trace.rs
- Verify removal: apps/ysda/src/output.rs

- [ ] **Step 1: Write the release-gate script**

scripts/v0.2-release-gate.sh:

~~~bash
#!/usr/bin/env bash
set -euo pipefail

rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace

rtk docker compose -f fixtures/postgres/compose.yaml up -d --wait
cleanup() {
  rtk docker compose -f fixtures/postgres/compose.yaml down -v
}
trap cleanup EXIT

rtk env YSDA_TEST_POSTGRES_URL=postgres://ysda:ysda-test@127.0.0.1:55432/ysda_test cargo test -p ys-agent-adapters --test postgres_connector_test -- --ignored

rtk cargo test -p ysda --test query_eval_test
~~~

Make it executable. The cleanup target is the explicit fixture Compose project only.

- [ ] **Step 2: Update README product positioning**

README must state:

- YS Data Agent is a Data-Engineer-owned full-stack AI data team for lean organizations;
- v0.2 implements the trustworthy Query vertical slice;
- the five long-term Workflow outcomes;
- the Task-centric architecture;
- local Runtime Store versus user business data;
- read-only security boundary;
- v0.2 exclusions.

- [ ] **Step 3: Document local setup**

Create .env.example with the listed key names and empty secret values. Provide exact README commands for:

~~~dotenv
YSDA_LLM_BASE_URL=
YSDA_LLM_API_KEY=
YSDA_LLM_MODEL=
YSDA_DATA_SOURCE_KIND=sqlite
YSDA_DATA_SOURCE_URL=
YSDA_SQLITE_PATH=fixtures/demo.db
YSDA_METRIC_REGISTRY_PATH=fixtures/metrics/metrics.json
YSDA_DBT_MANIFEST_PATH=fixtures/dbt/manifest.json
~~~

~~~bash
rtk cargo build --workspace
rtk cp .env.example .env
rtk cargo run -p ysda
~~~

Document required local configuration keys without example secrets:

~~~text
YSDA_LLM_BASE_URL
YSDA_LLM_API_KEY
YSDA_LLM_MODEL
YSDA_DATA_SOURCE_KIND
YSDA_DATA_SOURCE_URL or YSDA_SQLITE_PATH
YSDA_METRIC_REGISTRY_PATH
YSDA_DBT_MANIFEST_PATH
~~~

Explain that OpenAI-compatible providers must support Tool Calls, Tool Call IDs and multi-turn Tool Result messages.

- [ ] **Step 4: Document Artifact and state locations**

Explain:

~~~text
.ysda/runtime.db    Agent control state
.ysda/artifacts/    Query, verification and context Artifacts
user Postgres       business data queried through user-scoped credentials
~~~

State that deleting .ysda loses local Task recovery and is not a log cleanup operation.

- [ ] **Step 5: Verify dependency direction**

Run:

~~~bash
rtk cargo tree -p ys-agent-core
rtk cargo tree -p ys-agent-runtime
rtk cargo tree -p ys-agent-store
rtk cargo tree -p ys-agent-adapters
~~~

Verify:

- core contains no Ratatui, Reqwest, Rusqlite, SQLx or concrete provider dependency;
- runtime does not depend on store or adapters;
- store and adapters do not depend on runtime;
- apps/ysda is the only composition root.

- [ ] **Step 6: Verify no v0.1 type remains authoritative**

Run:

~~~bash
rtk rg "AgentRun|TraceRecorder|struct QueryAgent|stage: String" apps crates
~~~

Expected: no production match. Test fixture names may only appear when explicitly testing migration rejection.

- [ ] **Step 7: Run the full release gate**

Run:

~~~bash
rtk ./scripts/v0.2-release-gate.sh
~~~

Expected:

- formatting passes;
- Clippy has zero warnings;
- all workspace tests pass;
- Postgres integration passes;
- all Query Eval cases pass;
- fixture Postgres is stopped by the trap.

- [ ] **Step 8: Smoke-test crash and resume**

1. start ysda;
2. submit an ambiguous Query and wait for clarification;
3. exit with /quit;
4. restart ysda;
5. run /resume TASK_ID;
6. answer the clarification;
7. verify the same Run ID completes;
8. inspect QueryArtifact and VerificationReport.

- [ ] **Step 9: Commit**

~~~bash
rtk git add .env.example README.md apps/ysda/Cargo.toml Cargo.lock scripts
rtk git commit -m "docs: prepare trustworthy query runtime release"
~~~

---

## 2. Dependency order

~~~text
Task 1  Workspace
  ↓
Task 2  Lifecycle domain
  ↓
Task 3  Events and ports
  ├──────────────┐
  ↓              ↓
Task 4 Store   Task 6 Model Adapter
  ↓              │
Task 5 Service   │
  ↓              │
Task 7 Tool Runtime
  ↓
Task 8 Connectors
  ↓
Task 9 Context
  ↓
Task 10 Query Tools
  ↓
Task 11 Harness and Query Workflow
  ↓
Task 12 Recovery
  ↓
Task 13 TUI
  ↓
Task 14 Telemetry and Eval
  ↓
Task 15 Release Gate
~~~

Tasks 4 and 6 may be implemented in parallel after Task 3. Every other task should land in the listed order because later tests rely on earlier stable contracts.

## 3. Definition of done

v0.2 is complete only when all statements are true:

- [ ] One ysda binary provides TUI and non-interactive commands.
- [ ] TUI uses AgentService and contains no Workflow logic.
- [ ] Session, Task and Run have separate persisted lifecycles.
- [ ] /new creates a Session and never cancels a Task.
- [ ] Query Workflow runs through the shared Harness and explicit Agent Loop.
- [ ] Model sees only the hashed Query ToolView.
- [ ] SQLite and Postgres both implement small capability ports.
- [ ] Active Metric definitions are governed; Draft metrics are not silently queried.
- [ ] dbt Context retains provenance and content hash.
- [ ] QueryVerifier blocks premature completion.
- [ ] Every successful Query has a QueryArtifact and VerificationReport.
- [ ] Clarification resumes the same Run after process restart.
- [ ] A terminal failed Run creates a new retry Run.
- [ ] Runtime Store and Telemetry failure domains are separate.
- [ ] Fake and Replay providers run core tests without network or token cost.
- [ ] Query Eval cases enforce outcome and trajectory.
- [ ] No production write Tool is registered.
- [ ] The release-gate script passes.

## 4. Intentional follow-up boundaries

Do not broaden a v0.2 implementation task when encountering these needs:

| Need discovered during v0.2 | Record for |
|---|---|
| Dashboard or causal analysis | v0.3 Analysis |
| Git Worktree, dbt edits, ChangeSet | v0.4 Build/Change |
| Action approval execution | v0.4 Build/Change |
| multi-day external job | v0.5 Durable Execution |
| Airflow or Dagster control | v0.5 Operate |
| shared Postgres Runtime Store | v0.6 Shared Runtime |
| Web/API and multi-user identity | v0.6 Shared Runtime |
| Python/Polars feature work | v0.7 ML Data Prep |
| embeddings or vector retrieval | separate Context milestone after deterministic retrieval is measured |

## 5. Plan self-review checklist

Before implementation begins, verify:

- [ ] Every file in a task appears in the final repository map or is explicitly removed.
- [ ] Core types used by later tasks are introduced in Tasks 2 and 3.
- [ ] Runtime never imports a concrete Adapter.
- [ ] Store append and Snapshot update are atomic.
- [ ] Query Tool and Completion Gate cannot bypass Policy.
- [ ] Runtime recovery never blindly retries an indeterminate write.
- [ ] TUI quit and Session /new never imply Task cancellation.
- [ ] Telemetry and Eval cannot become Runtime state dependencies.
- [ ] The plan contains no implementation work for excluded Workflows.

## 6. Execution handoff

Plan complete and saved to docs/superpowers/plans/2026-08-06-ys-data-agent-v0.2.md.

Two execution options:

1. **Subagent-Driven (recommended):** dispatch a fresh implementation agent per task and perform review between tasks.
2. **Inline Execution:** execute tasks in this session through the Superpowers executing-plans workflow with batch checkpoints.

Do not start either option until the architecture spec and this plan have been reviewed and approved.
