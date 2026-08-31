# YS Data Agent

YS Data Agent is an early implementation of an Accountable-Data-Owner-governed AI data team for small and medium-sized businesses that cannot staff a complete data team.

The long-term product should absorb technical complexity for the customer. It should use mature databases, compute engines, transformation frameworks, and orchestrators instead of rebuilding them. A resident Data Engineer should not be a permanent requirement. A business owner is still required to confirm business meaning, access, and high-risk choices.

## v0.2: Trustworthy Query Pilot

v0.2 is a local technical validation Pilot for Data Engineers and technical analysts. The technical operator is a temporary entry point used to validate trustworthy query behavior. They are not the final product persona.

The Pilot accepts one natural-language question and produces one durable, verified Query Artifact or one explicit non-success state. It supports:

- GovernedMetric queries backed by an Active metric contract;
- authorized AdHocRead queries with inferred semantic status and explicit assumptions;
- Metadata questions backed by observed schema or Freshness evidence;
- clarification when business meaning, time range, timezone, metric, or retry cost is materially ambiguous;
- explicit UnsupportedCapability responses for work outside v0.2.

## Long-term Workflow outcomes

The long-term product targets five user outcomes:

1. Query — answer governed factual questions from approved data;
2. Analysis — explain changes, compare segments, and test hypotheses;
3. Build/Change — prepare reviewed changes to data products and transformations;
4. Operate — monitor and recover durable data work;
5. ML Data Prep — create governed datasets and features for machine learning.

Only Query is implemented in v0.2. The other outcomes are product direction, not hidden modes in this binary.

## Suitable Pilot Workspace

A suitable v0.2 Pilot has:

- one queryable SQLite or PostgreSQL source;
- a database-enforced least-privilege read-only identity;
- an owner for metric meaning, timezone, Freshness, and sensitivity policy;
- an OpenAI-compatible model endpoint with Tool Calls, Tool Call IDs, multi-turn Tool Result messages, and a known context limit;
- local owner-only storage for Runtime state and Artifacts.

Run `ysda doctor` before the first query. Doctor checks the configured path to a trusted answer and prints safe repair instructions. It never prints credential values.

## Task-centric architecture

```text
Session
  └── Task: durable user goal
       ├── Run: one execution attempt
       │    ├── Step
       │    ├── typed Event
       │    ├── Snapshot
       │    └── Artifact
       └── later retry Run when a terminal failure is retried

TUI / CLI
  └── AgentServiceApi
       └── Coordinator + LoopDriver + Harness + Query Workflow
            ├── ModelProvider
            ├── phase-scoped ToolView
            ├── governed Query Tools
            ├── RuntimeStore
            └── ArtifactStore
```

The TUI contains no Workflow logic. It communicates only through `AgentServiceApi`. Runtime Events and Snapshots are authoritative. Telemetry is separate and cannot roll back a committed result.

## Trust and security boundaries

The Pilot uses several independent boundaries:

- SQL AST policy accepts only the supported read shape;
- the database connection or role enforces read-only behavior;
- `AllowedDataScope` limits approved sources, relations, and fields;
- `QueryBudget` limits time, rows, bytes, concurrency, and supported estimated cost;
- `ResultPolicy` denies, redacts, or keeps sensitive results local-only;
- Tool access is narrowed by Query phase;
- Active metric contracts compile deterministically;
- dbt and other Context remain `UntrustedData`;
- `QueryVerifier` checks evidence before completion;
- export reuses persisted Artifact bytes and never reruns SQL;
- credential values never belong in Prompt, Event, Artifact, Telemetry, or Eval output.

These controls reduce risk but do not make the Pilot a complete production security platform. Test with synthetic or approved data first.

## Supported examples

GovernedMetric:

```text
GMV for the last seven complete days
```

AdHocRead:

```text
List distinct order channels
```

Metadata:

```text
What columns are in mart_orders?
```

Unsupported Analysis:

```text
Why did GMV fall?
```

The last example returns `UnsupportedCapability`. It does not create a fake Analysis Run.

## v0.2 exclusions

v0.2 does not include:

- Analysis, Build/Change, Operate, or ML Data Prep execution;
- production writes, Merge, Deploy, or approval workflows;
- Workspace Bootstrap or a non-technical onboarding wizard;
- Excel, CSV, or SaaS ingestion;
- Starter Data Stack provisioning or infrastructure management;
- a Web/API server, multi-user authentication, or managed control plane;
- Airflow or Dagster operation;
- Python workers, vector retrieval, or a complete semantic engine.

## Local setup

### 1. Build the Workspace

```bash
rtk cargo build --workspace
```

### 2. Create local configuration

```bash
rtk cp .env.example .env
```

Edit `.env` locally. Never commit it. Required keys are:

```text
YSDA_LLM_BASE_URL
YSDA_LLM_API_KEY
YSDA_LLM_MODEL
YSDA_DATA_SOURCE_KIND
YSDA_DATA_SOURCE_ID
YSDA_DATA_SOURCE_URL or YSDA_SQLITE_PATH
YSDA_METRIC_REGISTRY_PATH
YSDA_DBT_MANIFEST_PATH
YSDA_QUERY_POLICY_PATH
YSDA_TIMEZONE
YSDA_QUERY_TIMEOUT_SECONDS
YSDA_QUERY_MAX_ROWS
YSDA_QUERY_MAX_RESULT_BYTES
YSDA_QUERY_MAX_ESTIMATED_COST_UNITS (optional; Connector must support preflight cost)
YSDA_ARTIFACT_RETENTION_DAYS
```

An OpenAI-compatible provider must support Tool Calls, Tool Call IDs, multi-turn Tool Result messages, and a known context limit.

### 3. Create the SQLite demo source

`sqlite3` is required only for this demo setup:

```bash
rtk mkdir -p .ysda
rtk sqlite3 .ysda/demo.db ".read fixtures/sql/sqlite_seed.sql"
```

For the SQLite demo, the template sets `YSDA_DATA_SOURCE_ID=sqlite_demo`, which
matches the source ID authorized by `fixtures/policy/query-policy.json`. Keep
this value aligned with the selected policy source ID.

Real PostgreSQL Pilot users do not create the demo database. They configure `YSDA_DATA_SOURCE_KIND=postgres` and supply a least-privilege `CredentialReference` through `YSDA_DATA_SOURCE_URL`.

### 4. Check readiness

Load `.env` with your preferred local environment tool, then run:

```bash
rtk cargo run -p ysda -- doctor
```

Repair every blocker before submitting a query. A warning may disable only one capability, such as GovernedMetric when the Metric Registry is missing.

### 5. Open the TUI

```bash
rtk cargo run -p ysda
```

No subcommand opens the focused Query TUI. Type `/help` for supported commands. `/quit` detaches the UI and does not cancel a Task or Run.

The welcome screen is intentionally minimal. After a query, the main area shows a concise full-width answer labeled `Ys-da`; it has no persistent sidebar or recent-work panel. Secondary information is selected only when needed:

```text
/metrics
/query
/checks
/artifact [ARTIFACT_ID]
/sql
/details
```

`/artifact` without an ID shows the current primary Artifact. Supplying an ID shows that persisted Artifact. Neither form reruns SQL. Esc returns from a focused view to the answer.

Typing `/` opens a full-width command palette in place of Composer. `/theme` previews and selects `deep-navy`, `terminal`, `nord`, or `gruvbox`. Custom colors use `/theme set TOKEN COLOR`; `/theme reset` returns to `deep-navy`. Accepted colors are `#RRGGBB`, ANSI names, `ansi:N`, and `default`. Preferences persist locally in `.ysda/ui.toml`; `NO_COLOR=1` disables decorative colors.

Mouse capture is off by default so normal terminal selection keeps working. Set `YSDA_TUI_MOUSE=1` only when mouse selection inside the command palette is desired.

### 6. Run a non-interactive query

```bash
rtk cargo run -p ysda -- run "GMV for the last seven complete days"
```

Both paths use the same `AgentServiceApi`, Harness, policies, and stores.

## Local state and business data

```text
.ysda/runtime.db    Agent control state: Sessions, Tasks, Runs, Events, and Snapshots
.ysda/artifacts/    Query, verification, result, and context Artifacts
.ysda/exports/      policy-approved, content-addressed exports
.ysda/ui.toml       owner-only local theme and UI preferences
user PostgreSQL     business data queried through user-scoped credentials
```

`.ysda` is not a general log directory. Deleting it loses local Task and clarification recovery. Back it up or remove it only as an explicit Workspace reset.

Runtime, Artifact, export, and UI preference paths must be owner-only and writable. Artifacts carry sensitivity, retention, and expiry metadata. `.ysda/ui.toml` contains only validated theme preferences; it is not Runtime recovery state and never contains Prompt, SQL, rows, credentials, or Telemetry. Cleanup must be explicit and policy-aware. Secret values never belong in these paths.

## Artifact export

Inspect safe metadata or a bounded preview:

```bash
rtk cargo run -p ysda -- artifact ARTIFACT_ID
```

Request a policy-controlled export:

```bash
rtk cargo run -p ysda -- artifact ARTIFACT_ID --format json
rtk cargo run -p ysda -- artifact ARTIFACT_ID --format csv
rtk cargo run -p ysda -- artifact ARTIFACT_ID --format markdown
```

Restricted export fails closed. CSV uses the persisted result schema, Markdown uses the persisted answer and evidence fields, and JSON preserves typed fields.

## Development checks

Fast local checks:

```bash
rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
```

Complete release gate, including the fixture PostgreSQL integration:

```bash
rtk ./scripts/v0.2-release-gate.sh
```

The gate uses Fake or Replay model providers. It requires no live model request and spends no model tokens.

## Recovery promise

v0.2 resumes between durable Steps and after `WaitingForInput`. A clarification answer resumes the same Run after restart. If a process dies while SQL is in flight, the ToolCall becomes indeterminate. A low-cost read may create a new ToolCall only after explicit resume. A high-cost, cost-unknown, or remotely identifiable call waits for reconciliation, cancellation, or user confirmation.

## License

MIT
