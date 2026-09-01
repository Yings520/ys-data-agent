# Natural-Language Time Clarification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. The user prohibited subagents, so execute inline only.

**Goal:** Let users answer time clarifications with ordinary phrases such as “昨天” or “上周”, then automatically resume the same Query Run while keeping RFC3339 UTC as an internal-only contract.

**Architecture:** Add trusted current-time and workspace-timezone fields to the runtime-owned model state, strengthen the prompt boundary between natural user language and internal query-plan timestamps, and make the production scheduler’s Run-ID guard cover only active executions. Preserve all existing policy, evidence, and audit behavior.

**Tech Stack:** Rust, Tokio, Chrono, serde_json, Cargo tests, GitHub Actions

---

## File Map

- Modify `crates/ys-agent-runtime/src/workflow/query/prompts.rs`: user-facing clarification rules and prompt contract tests.
- Modify `crates/ys-agent-runtime/src/workflow/query/state.rs`: deterministic natural-language fallback clarification and test.
- Modify `crates/ys-agent-runtime/src/harness.rs`: trusted temporal fields in runtime-owned model state.
- Modify `crates/ys-agent-runtime/tests/support/mod.rs`: test harness workspace timezone.
- Modify `crates/ys-agent-runtime/tests/query_workflow_test.rs`: temporal-context regression assertion.
- Modify `apps/ysda/src/bootstrap.rs`: production/deterministic timezone wiring and resumable background scheduling.
- Update PR `#20`: explain both field regressions and record verification.

### Task 1: Keep internal timestamp formats out of clarification questions

**Files:**
- Modify: `crates/ys-agent-runtime/src/workflow/query/prompts.rs`
- Modify: `crates/ys-agent-runtime/src/workflow/query/state.rs`

- [ ] **Step 1: Write failing prompt-boundary tests**

Add tests which require every phase to instruct the model to use the user’s language and prohibit RFC3339, UTC conversion requests, SQL, artifact IDs, and Run IDs in clarification questions. Keep a separate assertion that the Plan phase still contains `<RFC3339 UTC>` for the internal typed action.

```rust
#[test]
fn clarification_keeps_internal_formats_away_from_users() {
    for phase in ALL_QUERY_PHASES {
        let prompt = query_system_instructions(phase);
        assert!(prompt.contains("same language as the user"));
        assert!(prompt.contains("Never ask the user for RFC3339"));
        assert!(prompt.contains("yesterday, last week, or July 2026"));
    }
    assert!(query_system_instructions(QueryPhase::Plan).contains("<RFC3339 UTC>"));
}
```

Add a state test which calls `material_ambiguity("Show GMV recently")` and asserts that the question contains natural examples and excludes `RFC3339`, `UTC`, and `timezone`.

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
rtk cargo test -p ys-agent-runtime clarification_keeps_internal_formats_away_from_users -- --nocapture
rtk cargo test -p ys-agent-runtime ambiguous_time_uses_a_natural_language_question -- --nocapture
```

Expected: both tests fail because the current prompt lacks the user-language boundary and the fallback asks for an exact range and timezone.

- [ ] **Step 3: Implement the minimal prompt and fallback wording**

Extend `BASE_INSTRUCTIONS` with an explicit user/internal boundary:

```rust
"Ask clarification in the same language as the user and use ordinary business language.\n",
"Never ask the user for RFC3339, UTC conversion, SQL, Artifact IDs, Run IDs, or other internal protocol values.\n",
"For a missing time range, ask for a natural period such as yesterday, last week, or July 2026. Convert the answer internally.\n",
```

Change the deterministic fallback question to:

```rust
"Which time period should I use—for example, yesterday, last week, or July 2026?"
```

- [ ] **Step 4: Run tests and verify GREEN**

Run the two commands from Step 2. Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/ys-agent-runtime/src/workflow/query/prompts.rs crates/ys-agent-runtime/src/workflow/query/state.rs
rtk git commit -m "fix(query): keep time clarification user friendly"
```

### Task 2: Give the model trusted temporal context

**Files:**
- Modify: `crates/ys-agent-runtime/src/harness.rs`
- Modify: `crates/ys-agent-runtime/tests/support/mod.rs`
- Modify: `crates/ys-agent-runtime/tests/query_workflow_test.rs`
- Modify: `apps/ysda/src/bootstrap.rs`

- [ ] **Step 1: Write the failing runtime-state test**

In `each_live_model_phase_receives_the_runtime_identities_it_must_reuse`, assert:

```rust
assert!(plan["current_time_utc"].as_str().is_some_and(|value| value.ends_with('Z')));
assert_eq!(plan["workspace_timezone"], "UTC");
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
rtk cargo test -p ys-agent-runtime --test query_workflow_test each_live_model_phase_receives_the_runtime_identities_it_must_reuse -- --nocapture
```

Expected: fail because `current_time_utc` and `workspace_timezone` are absent.

- [ ] **Step 3: Add workspace timezone to HarnessConfig**

Add:

```rust
pub workspace_timezone: String,
```

Wire it at every constructor:

- production: `workspace_timezone: config.timezone.clone()`;
- deterministic eval: `workspace_timezone: config.timezone.clone().unwrap_or_else(|| "UTC".to_owned())` (the missing-timezone eval remains blocked by readiness before execution);
- runtime tests: `workspace_timezone: "UTC".to_owned()`.

- [ ] **Step 4: Add runtime-owned temporal fields**

Compute one `now = Utc::now()` per model step. Pass the same value to context assembly and `runtime_query_state_message`. Serialize:

```rust
"current_time_utc": now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
"workspace_timezone": self.config.workspace_timezone,
```

Do not place these values in untrusted clarification evidence.

- [ ] **Step 5: Run the focused test and verify GREEN**

Run the command from Step 2. Expected: pass with `UTC` and an RFC3339 `Z` value in trusted runtime state.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/ys-agent-runtime/src/harness.rs crates/ys-agent-runtime/tests/support/mod.rs crates/ys-agent-runtime/tests/query_workflow_test.rs apps/ysda/src/bootstrap.rs
rtk git commit -m "fix(query): provide trusted temporal context"
```

### Task 3: Allow a clarified Run to be scheduled again

**Files:**
- Modify: `apps/ysda/src/bootstrap.rs`

- [ ] **Step 1: Write a failing concrete-scheduler test**

Inside `bootstrap.rs` tests, implement a real `HarnessStep` which increments an `AtomicUsize` and returns `StepOutcome::Wait` with a minimal `RunSnapshot`. Construct the concrete `BackgroundScheduler` with `LoopDriver::with_defaults`, schedule the same Run ID, wait for the first call, schedule it again, and require the counter to reach two within one second.

```rust
#[tokio::test]
async fn background_scheduler_releases_a_waiting_run_for_resumption() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Arc::new(WaitingHarness { calls: calls.clone() });
    let scheduler = BackgroundScheduler::new(Arc::new(LoopDriver::with_defaults(harness)));
    let run_id = RunId::new();

    scheduler.schedule(run_id).await.expect("first schedule");
    wait_for_calls(&calls, 1).await;
    scheduler.schedule(run_id).await.expect("resume schedule");
    wait_for_calls(&calls, 2).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
rtk cargo test -p ysda background_scheduler_releases_a_waiting_run_for_resumption -- --nocapture
```

Expected: fail on the second wait because the Run ID remains permanently present in `scheduled`.

- [ ] **Step 3: Release the active Run ID when the driver returns**

Change the guard to shared ownership:

```rust
scheduled: Arc<Mutex<HashSet<RunId>>>,
```

Clone it into the spawned task and remove the Run ID immediately after `driver.run(&run_id).await` returns, before publishing or logging the result:

```rust
let result = driver.run(&run_id).await;
scheduled.lock().expect("scheduler run mutex").remove(&run_id);
match result { /* existing notification and warning behavior */ }
```

This preserves coalescing while a driver is active and permits later clarification resumption or retry.

- [ ] **Step 4: Run scheduler tests and verify GREEN**

Run the command from Step 2. Expected: pass with exactly two driver calls.

- [ ] **Step 5: Commit**

```bash
rtk git add apps/ysda/src/bootstrap.rs
rtk git commit -m "fix(runtime): reschedule clarified query runs"
```

### Task 4: Run complete verification and update the PR

**Files:**
- Verify all modified files.
- Update GitHub PR `#20` metadata.

- [ ] **Step 1: Run focused field regressions**

```bash
rtk cargo test -p ys-agent-adapters allows_the_captured_daily_sales_query -- --nocapture
rtk cargo test -p ys-agent-runtime clarification_keeps_internal_formats_away_from_users -- --nocapture
rtk cargo test -p ys-agent-runtime --test query_workflow_test each_live_model_phase_receives_the_runtime_identities_it_must_reuse -- --nocapture
rtk cargo test -p ysda background_scheduler_releases_a_waiting_run_for_resumption -- --nocapture
```

Expected: four passing regressions.

- [ ] **Step 2: Run full quality gates**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all commands exit zero with no warnings.

- [ ] **Step 3: Check cleanup and repository state**

```bash
rtk git diff --check
rtk rg -n '\[DEBUG-' . -g '!target/**' -g '!.git/**'
rtk git status --short --branch
```

Expected: no whitespace errors, no debug markers, and a clean tracked worktree after commits.

- [ ] **Step 4: Push and update PR #20**

Push `test/task-15-captured-daily-sales-replay`, retitle PR #20 to `fix(query): resume natural-language time clarifications`, and update its body with the two captured Evidence chains, focused tests, and full gates.

- [ ] **Step 5: Monitor CI to completion**

```bash
rtk gh pr checks 20 --watch --interval 10
```

Expected: Rust `build` concludes `SUCCESS`, PR is `CLEAN` and `MERGEABLE`.
