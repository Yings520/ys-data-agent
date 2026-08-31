# Task 15 Runtime Readiness Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Task 15's documented cost, retention, and model-readiness controls real and release-gated.

**Architecture:** Parse limits at the app composition root, pass retention explicitly into the two runtime services that own day-retained Artifacts, and make Doctor perform a cached two-stage protocol handshake through the production `ModelProvider`. Preserve existing default constructors and deterministic automated tests.

**Tech Stack:** Rust 2024, Tokio, async traits, Wiremock/Fake model providers, SQLite/PostgreSQL fixtures, Cargo/Clippy, Bash release gate.

---

### Task 1: Wire cost and retention configuration into production

**Files:**
- Modify: `apps/ysda/src/bootstrap.rs`
- Modify: `crates/ys-agent-runtime/src/service.rs`
- Modify: `crates/ys-agent-runtime/src/export.rs`
- Test: `apps/ysda/src/bootstrap.rs`
- Test: `crates/ys-agent-runtime/src/service.rs`
- Test: `apps/ysda/tests/export_test.rs`

- [ ] **Step 1: Write failing configuration tests**

Add focused tests that construct AppConfig from isolated environment values and assert:

```rust
assert_eq!(config.query_budget.max_estimated_cost_units, Some(42));
assert_eq!(config.artifact_retention_days, 11);
```

Add invalid-value cases asserting `invalid_query_budget` for zero/non-numeric cost and `invalid_artifact_retention` for zero/non-numeric/out-of-range days.

- [ ] **Step 2: Run the configuration tests and verify RED**

Run:

```bash
rtk cargo test -p ysda bootstrap::tests --lib
```

Expected: the new tests fail because `AppConfig` has no `artifact_retention_days` and cost remains `None`.

- [ ] **Step 3: Implement strict parsing**

Add `artifact_retention_days: u32` to AppConfig. Parse optional cost into `QueryBudget.max_estimated_cost_units`; parse required retention with a dedicated typed validation code. Include `YSDA_ARTIFACT_RETENTION_DAYS` in missing-config reporting while keeping estimated cost optional.

- [ ] **Step 4: Write failing retention propagation tests**

Add a service test that constructs the service with 11 retention days, answers a restricted clarification, and asserts:

```rust
assert_eq!(metadata.retention_policy, Some(RetentionPolicy::Days { days: 11 }));
assert!(metadata.expires_at.is_some());
```

Add an export integration test that constructs the exporter with 11 retention days and asserts the same policy plus an expiry approximately eleven days after creation.

- [ ] **Step 5: Run the retention tests and verify RED**

Run:

```bash
rtk cargo test -p ys-agent-runtime service
rtk cargo test -p ysda --test export_test
```

Expected: the configured-day assertions fail because both production paths still use seven days.

- [ ] **Step 6: Implement explicit retention policy injection**

Add an `artifact_retention_days: u32` field to `InProcessAgentService` and `ArtifactExporter`. Keep existing constructors defaulting to seven days, add explicit constructors for production composition, and replace both hard-coded `RetentionPolicy::Days { days: 7 }` sites. Set export `expires_at` consistently with the configured duration.

- [ ] **Step 7: Pass production configuration and verify GREEN**

Pass `config.artifact_retention_days` from bootstrap to both services, then run:

```bash
rtk cargo fmt --all --check
rtk cargo test -p ys-agent-runtime service
rtk cargo test -p ysda bootstrap::tests --lib
rtk cargo test -p ysda --test export_test
```

Expected: all focused tests pass.

- [ ] **Step 8: Commit**

```bash
rtk git add apps/ysda/src/bootstrap.rs crates/ys-agent-runtime/src/service.rs crates/ys-agent-runtime/src/export.rs apps/ysda/tests/export_test.rs
rtk git commit -m "fix(runtime): enforce documented query limits"
```

### Task 2: Make Doctor verify the live model protocol

**Files:**
- Modify: `apps/ysda/src/bootstrap.rs`
- Test: `apps/ysda/src/bootstrap.rs`
- Verify: `crates/ys-agent-adapters/tests/model_provider_test.rs`

- [ ] **Step 1: Write failing two-stage probe tests**

Using `FakeModelProvider`, add a success test whose first response is:

```rust
AgentAction::CallTool {
    call: ToolCall {
        provider_call_id: Some("doctor-call-1".to_owned()),
        name: "ysda_doctor_probe".to_owned(),
        ..
    }
}
```

and whose second response is `AgentAction::ProposeCompletion`. Assert that the second request contains a Tool message with `tool_call_id == "doctor-call-1"`, and that the returned ModelReadiness has every required field true.

Add failure tests for first-turn non-tool action, missing provider call ID, second-turn tool action, and provider error. Add a counter assertion showing two Doctor inspections perform the handshake only once per process.

- [ ] **Step 2: Run probe tests and verify RED**

Run:

```bash
rtk cargo test -p ysda bootstrap::tests::model_protocol --lib
```

Expected: tests fail because no protocol probe or cache exists.

- [ ] **Step 3: Implement the isolated handshake**

Create a private `probe_model_protocol` async function using one synthetic low-risk ToolSpec and `ContextManifest::empty`. It must:

```text
request 1 -> require CallTool + matching name + non-empty provider ID
request 2 -> send Tool result with that ID -> require a non-CallTool action
```

Return all-false ModelReadiness on any error. Do not persist requests/responses or expose raw content.

- [ ] **Step 4: Cache and integrate the verdict**

Give `RuntimeDoctorProbe` the production model and a Tokio `OnceCell<ModelReadiness>`. In `inspect`, initialize the cell with the handshake and overwrite the static model readiness fields with the cached verdict. Change production assembly to return the same provider Arc to both Harness and Doctor. Safe fallback remains all false.

- [ ] **Step 5: Verify GREEN and adapter compatibility**

Run:

```bash
rtk cargo fmt --all --check
rtk cargo test -p ysda bootstrap::tests::model_protocol --lib
rtk cargo test -p ysda --test doctor_test
rtk cargo test -p ys-agent-adapters --test model_provider_test
```

Expected: all tests pass; no live network call occurs.

- [ ] **Step 6: Commit**

```bash
rtk git add apps/ysda/src/bootstrap.rs
rtk git commit -m "fix(doctor): verify model tool protocol"
```

### Task 3: Align public documentation and release verification

**Files:**
- Modify: `README.md`
- Modify: `scripts/v0.2-release-gate.sh`
- Verify: `.env.example`

- [ ] **Step 1: Update the operator contract**

Document that Doctor performs two small synthetic model calls once per process, contains no business data, and blocks submission on incompatible responses. State that cost is optional, retention is required, and both are enforced rather than descriptive.

- [ ] **Step 2: Extend the release gate**

Add the focused bootstrap model-protocol tests to the deterministic gate. Retain the statement that the gate itself performs no live model call because the tests use Fake/Wiremock providers.

- [ ] **Step 3: Verify docs and secret safety**

Run:

```bash
rtk rg -n 'protocol probe|estimated cost|retention' README.md
rtk rg -n '(sk-[A-Za-z0-9]|Bearer [A-Za-z0-9]|postgres://[^[:space:]]+:[^[:space:]]+@)' README.md .env.example scripts apps crates evals
rtk bash -n scripts/v0.2-release-gate.sh
```

Expected: documentation matches runtime behavior; only the disposable fixture URL is an expected secret-pattern match.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md scripts/v0.2-release-gate.sh
rtk git commit -m "docs: describe enforced runtime readiness"
```

### Task 4: Re-run release and live acceptance

**Files:**
- Modify: none
- Verify: `.superpowers/sdd/progress.md`

- [ ] **Step 1: Run the full automated gate**

```bash
rtk ./scripts/v0.2-release-gate.sh
```

Expected: fmt, Clippy, workspace tests, PostgreSQL, Eval, Doctor, Export, and TUI checks pass; the fixture project is removed.

- [ ] **Step 2: Run live Doctor once**

Load the ignored `.env` without printing it and run:

```bash
rtk cargo run -p ysda -- doctor
```

Expected: a compatible endpoint returns no model blocker. The currently configured endpoint may instead return `model_protocol_incompatible`; that is a truthful deployment blocker and must prevent Query submission.

- [ ] **Step 3: Re-run crash/resume when Doctor is ready**

Create an ambiguous query, detach, restart, resume the same Task, answer the clarification, and verify the same Run ID completes. If Doctor blocks the endpoint, record the external compatibility blocker without issuing additional query calls.

- [ ] **Step 4: Final review**

Generate a review package from `7a33f85` to HEAD. Require independent review with no open Critical or Important findings, then update the durable ledger with exact automated and live acceptance outcomes.
