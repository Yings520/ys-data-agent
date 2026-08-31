# Non-blocking TUI Submission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Enter acknowledge `你好` immediately while model/service work completes without blocking terminal redraw or input.

**Architecture:** Convert message and clarification submission into an owned `PendingSubmission` future that contains cloned service dependencies and request state but borrows neither `TuiApp` nor `TuiController`. Poll that future as another branch of the existing `tokio::select!`, then apply its typed completion to controller and UI state.

**Tech Stack:** Rust, Tokio, Crossterm, Ratatui, existing `AgentServiceApi` and in-process service test fixtures.

---

### Task 1: Lock down the frozen-Enter regression

**Files:**
- Modify: `apps/ysda/src/tui/event_loop.rs`

- [ ] **Step 1: Write the failing event-loop test**

Add a Tokio test in the `event_loop.rs` test module. Build an `InProcessAgentService` whose front-door `FakeModelProvider` waits on a `Notify`. Put `你好` in the composer and submit an Enter event under a 100 ms timeout:

```rust
let returned = tokio::time::timeout(
    Duration::from_millis(100),
    handle_terminal_event(&mut app, &mut controller, &mut pending, enter()),
).await;
assert!(returned.is_ok(), "Enter must not wait for the model");
assert!(matches!(app.transcript.last(), Some(TranscriptItem::UserMessage(text)) if text == "你好"));
assert_eq!(app.runtime_status.as_deref(), Some("Thinking…"));
```

Before releasing the model, send a character event and assert that the composer accepts it. Release the model, await the pending completion, apply it, and assert that the Ys-da chat answer renders.

- [ ] **Step 2: Run the test and verify RED**

Run: `cargo test -p ysda tui_submission_acknowledges_before_model_returns -- --exact`

Expected: FAIL because the current Enter handler awaits `service.send_message` and has no pending-submission seam.

- [ ] **Step 3: Commit the red regression test**

Run:

```bash
git add apps/ysda/src/tui/event_loop.rs
git commit -m "test(tui): reproduce blocked message submission"
```

### Task 2: Move submission off the terminal event handler

**Files:**
- Modify: `apps/ysda/src/tui/app.rs`
- Modify: `apps/ysda/src/tui/event_loop.rs`

- [ ] **Step 1: Add typed pending submission state**

Define an owned future and completion payload in `app.rs`:

```rust
pub struct PendingSubmission {
    future: Pin<Box<dyn Future<Output = CoreResult<SubmissionCompletion>> + Send>>,
}

pub enum SubmissionCompletion {
    Message { session_id: SessionId, reply: ServiceReply },
    Clarification,
}
```

Implement `Future` for `PendingSubmission`. Add `TuiController::start_submission`, which validates readiness, immediately records the user message and `Thinking…`/clarification progress, snapshots IDs, and returns a future built from a cloned `Arc<dyn AgentServiceApi>`. Add `complete_submission`, which updates session/focus state and maps every `ServiceReply` using the existing behavior.

- [ ] **Step 2: Poll pending work beside terminal events**

Keep `Option<PendingSubmission>` in `run_tui`. Extend `handle_terminal_event` and `submit_composer` to fill that slot without awaiting the provider. Add a guarded select branch:

```rust
completion = wait_for_submission(&mut pending_submission),
    if pending_submission.is_some() => {
        pending_submission = None;
        match completion {
            Ok(completion) => controller.complete_submission(&mut app, completion),
            Err(error) => {
                app.runtime_status = None;
                app.push_transcript(TranscriptItem::Error(user_readable_error(&error)));
            }
        }
        dirty = true;
    }
```

When a submission is already pending, keep newly composed text and show a non-destructive warning instead of sending a duplicate. Allow `/quit` and Ctrl-C to exit immediately; dropping the pending future cancels it.

- [ ] **Step 3: Run the regression test and verify GREEN**

Run: `cargo test -p ysda tui_submission_acknowledges_before_model_returns -- --exact`

Expected: PASS; Enter returns before the delayed model and the completion later renders.

- [ ] **Step 4: Add and run the failure completion case**

Use a delayed fake provider returning `CoreError::ProviderTimeout`. Assert Enter still acknowledges immediately, completion clears `runtime_status`, and the transcript contains the existing safe `provider_timeout` error.

Run: `cargo test -p ysda tui_submission_timeout_is_rendered_after_non_blocking_ack -- --exact`

Expected: PASS.

- [ ] **Step 5: Commit the implementation**

Run:

```bash
git add apps/ysda/src/tui/app.rs apps/ysda/src/tui/event_loop.rs
git commit -m "fix(tui): keep input responsive during model calls"
```

### Task 3: Release verification

**Files:**
- Modify only if a test exposes an issue in the two implementation files above.

- [ ] **Step 1: Run focused and workspace tests**

Run:

```bash
cargo test -p ysda --test tui_test
cargo test -p ysda
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all tests pass and Clippy emits no warnings.

- [ ] **Step 2: Exercise the compiled TUI in a pseudo-terminal**

Start `target/debug/ysda` through `expect`, type `你好`, and assert the captured screen reaches the submitted-message or progress state before the delayed/provider timeout deadline. Verify Ctrl-C exits while the request is in flight.

- [ ] **Step 3: Confirm cleanup and final diff**

Run:

```bash
rg '\[DEBUG-' apps/ysda/src/tui || true
git status --short
git diff HEAD~2 --check
```

Expected: no debug instrumentation, no whitespace errors, and only the planned TUI files plus design/plan commits differ from baseline.
