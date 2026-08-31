# Query Failure Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Execution constraint:** The user explicitly forbids subagents. Execute this plan inline with `executing-plans`.

**Goal:** Recover safely from SQL result aliases, malformed clarification actions, and invalid slash-command drafts so the reported Query TUI failures no longer terminate or disrupt user work.

**Architecture:** Keep each fix at its existing boundary. Normalize safe `ORDER BY` result aliases before the SQL authorization visitor runs, add one bounded model protocol-correction turn plus an exact clarification contract, and retain the composer draft when slash-command parsing fails.

**Tech Stack:** Rust, sqlparser 0.62 AST visitors, Tokio, Wiremock, Crossterm, Ratatui, Cargo workspace quality gates.

---

### Task 1: Authorize same-query `ORDER BY` result aliases

**Files:**
- Modify: `crates/ys-agent-adapters/src/data/sql_policy.rs`

- [ ] **Step 1: Add failing SQL policy regression and security tests**

Extend the test scope with `paid_amount`, `paid_at`, and denied `internal_note`, then add tests equivalent to:

```rust
#[test]
fn allows_ordering_by_a_declared_output_alias() {
    let policy = SqlReadOnlyPolicy::new(SupportedDialect::SQLite, 1_024);
    let decision = policy.evaluate(
        "SELECT date(paid_at) AS sale_date, SUM(paid_amount) AS daily_sales \
         FROM mart_orders WHERE paid_at >= '2026-07-01' AND paid_at < '2026-08-01' \
         GROUP BY date(paid_at) ORDER BY sale_date",
        &scope(),
    );

    assert_eq!(decision.disposition, SqlPolicyDisposition::Allowed);
    assert_eq!(decision.referenced_columns, vec!["paid_amount", "paid_at"]);
}

#[test]
fn output_alias_cannot_mask_a_denied_source_column() {
    let policy = SqlReadOnlyPolicy::new(SupportedDialect::SQLite, 1_024);
    let decision = policy.evaluate(
        "SELECT order_id AS internal_note FROM mart_orders ORDER BY internal_note",
        &scope(),
    );
    assert_eq!(decision.reasons[0].code, "column_denied");
}

#[test]
fn nested_alias_does_not_authorize_an_outer_identifier() {
    let policy = SqlReadOnlyPolicy::new(SupportedDialect::SQLite, 1_024);
    let decision = policy.evaluate(
        "SELECT secret FROM mart_orders WHERE EXISTS \
         (SELECT order_id AS secret FROM mart_orders ORDER BY secret)",
        &scope(),
    );
    assert_eq!(decision.reasons[0].code, "column_not_allowed");
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run: `rtk cargo test -p ys-agent-adapters allows_ordering_by_a_declared_output_alias`

Expected: FAIL because `sale_date` is currently collected as a physical source column.

- [ ] **Step 3: Normalize safe aliases before collecting authorization facts**

Add a small `VisitorMut` that, for each `Query`, maps unique `SelectItem::ExprWithAlias` names to their expressions and replaces a bare matching `ORDER BY` identifier with the cloned expression. Do not replace an alias whose name is configured as a source column, and process each query block independently:

```rust
struct OrderByAliasNormalizer<'a> {
    scope: &'a AllowedDataScope,
}

impl VisitorMut for OrderByAliasNormalizer<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<Self::Break> {
        let aliases = unique_projection_aliases(query);
        let Some(OrderBy { kind: OrderByKind::Expressions(items), .. }) = &mut query.order_by else {
            return ControlFlow::Continue(());
        };
        for item in items {
            let Expr::Identifier(identifier) = &item.expr else { continue };
            let name = identifier.value.to_ascii_lowercase();
            if source_column_is_configured(self.scope, &name) { continue; }
            if let Some(expression) = aliases.get(&name) {
                item.expr = expression.clone();
            }
        }
        ControlFlow::Continue(())
    }
}
```

Run this normalizer on the mutable parsed statements before the existing `AstFacts` visitor. Keep duplicate aliases unresolved and preserve every existing statement, relation, wildcard, function, and source-column check.

- [ ] **Step 4: Verify GREEN and commit**

Run:

```bash
rtk cargo test -p ys-agent-adapters data::sql_policy::tests
rtk cargo test -p ys-agent-adapters --test query_tools_test
rtk git add crates/ys-agent-adapters/src/data/sql_policy.rs
rtk git commit -m "fix(adapters): authorize safe query output aliases"
```

Expected: all focused tests pass.

### Task 2: Recover once from malformed typed model actions

**Files:**
- Modify: `crates/ys-agent-runtime/src/workflow/query/prompts.rs`
- Modify: `crates/ys-agent-adapters/src/model/openai_compatible.rs`
- Modify: `crates/ys-agent-adapters/tests/model_provider_test.rs`

- [ ] **Step 1: Add failing prompt and provider tests**

Add a prompt assertion that clarification-capable phases contain the exact `request_clarification` shape. Add a Wiremock test whose first response is:

```json
{"choices":[{"message":{"role":"assistant","content":"{\"type\":\"request_clarification\"}"}}]}
```

and whose second response is:

```json
{"choices":[{"message":{"role":"assistant","content":"{\"type\":\"request_clarification\",\"question\":\"Which time range?\"}"}}]}
```

Assert that `complete` returns `AgentAction::RequestClarification`, exactly two requests were received, the second request contains `PROTOCOL CORRECTION` and the exact action name, and it does not contain a canary placed in an ignored field of the malformed response. Extend the existing invalid-action test to assert that two malformed responses still return `invalid_model_response` after exactly two requests.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
rtk cargo test -p ys-agent-runtime clarification_action_contract
rtk cargo test -p ys-agent-adapters --test model_provider_test corrects_one_invalid_typed_action
```

Expected: the prompt assertion and recovery test fail because no exact contract or invalid-action correction exists.

- [ ] **Step 3: Add the exact prompt contract and bounded correction**

Add this non-negotiable instruction:

```rust
"When requesting clarification, return exactly one JSON object with this shape: ",
r#"{"type":"request_clarification","question":"<one concise question>"}"#,
". Do not wrap it in Markdown or prose.\n",
```

Extend the existing provider correction guard to accept `invalid_model_response` as well as `parallel_tool_calls_disabled`. Use a static invalid-action correction message that never embeds the rejected response or error text:

```rust
fn protocol_correction_message(error: &CoreError) -> Option<String> {
    match error.code() {
        "invalid_model_response" => Some(
            "PROTOCOL CORRECTION: Return exactly one valid typed AgentAction JSON object with all required fields, no Markdown or prose. For clarification use {\"type\":\"request_clarification\",\"question\":\"<one concise question>\"}."
                .to_owned(),
        ),
        "parallel_tool_calls_disabled" => Some(format!(
            "PROTOCOL CORRECTION: {error} Return at most one Tool Call. Do not include previous tool arguments."
        )),
        _ => None,
    }
}
```

Keep `MAX_PROTOCOL_CORRECTIONS` at one and leave transport retry limits unchanged.

- [ ] **Step 4: Verify GREEN and commit**

Run:

```bash
rtk cargo test -p ys-agent-runtime workflow::query::prompts::tests
rtk cargo test -p ys-agent-adapters --test model_provider_test
rtk git add crates/ys-agent-runtime/src/workflow/query/prompts.rs crates/ys-agent-adapters/src/model/openai_compatible.rs crates/ys-agent-adapters/tests/model_provider_test.rs
rtk git commit -m "fix(model): recover malformed clarification actions"
```

Expected: both suites pass; malformed actions receive one correction only.

### Task 3: Preserve invalid slash-command drafts

**Files:**
- Modify: `apps/ysda/src/tui/input.rs`
- Modify: `apps/ysda/src/tui/event_loop.rs`

- [ ] **Step 1: Add the failing event-loop regression test**

Add a Tokio test at the private event-loop seam. Set the composer to `/你好`, send Enter, and assert:

```rust
assert_eq!(app.composer.text(), "/你好");
assert!(matches!(
    app.transcript.last(),
    Some(TranscriptItem::Warning(text))
        if text.contains("delete the leading /") && text.contains("/help")
));
```

- [ ] **Step 2: Run the focused test and verify RED**

Run: `rtk cargo test -p ysda invalid_slash_command_keeps_draft_for_correction`

Expected: FAIL because `submit_composer` currently submits and clears the draft on parse errors.

- [ ] **Step 3: Preserve the draft and improve the warning**

Change the unknown-command error to:

```rust
format!(
    "unknown command {command}; / starts commands, delete the leading / to send chat, or type /help"
)
```

In the `Err(error)` branch of `submit_composer`, remove `app.composer.submit()` and only append the warning. Do not change valid command or chat submission behavior.

- [ ] **Step 4: Verify GREEN and commit**

Run:

```bash
rtk cargo test -p ysda invalid_slash_command_keeps_draft_for_correction
rtk cargo test -p ysda --test tui_test
rtk git add apps/ysda/src/tui/input.rs apps/ysda/src/tui/event_loop.rs
rtk git commit -m "fix(tui): preserve invalid command drafts"
```

Expected: the draft remains editable and all TUI tests pass.

### Task 4: Release validation and Pull Request

**Files:**
- Modify only if validation exposes a defect in the planned files.

- [ ] **Step 1: Run required quality gates**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every command passes without unexpected warnings.

- [ ] **Step 2: Review all changes and sensitive-data boundaries**

Run:

```bash
rtk git status --short
rtk git diff --check origin/master...HEAD
rtk git diff --stat origin/master...HEAD
rtk git log --oneline origin/master..HEAD
rtk rg '\[DEBUG-' crates apps || true
```

Confirm only the approved design, plan, tests, and implementation are present; no `.env`, Runtime database, Artifact, trace, secret, or generated file is tracked.

- [ ] **Step 3: Push and create the PR**

Run:

```bash
rtk git push -u origin fix/task-15-query-failure-recovery
rtk gh pr create --base master --head fix/task-15-query-failure-recovery --title "fix(runtime): recover query submission failures" --body "## Background

Query Runs failed on valid ORDER BY aliases and malformed clarification actions, while invalid slash commands discarded the user draft.

Task: task-15 follow-up

## Changes

- authorize safe same-query ORDER BY aliases without weakening source-column policy
- correct one malformed typed model action and document the exact clarification contract
- preserve invalid slash-command drafts with an actionable warning

## Testing

- rtk cargo fmt --all -- --check
- rtk cargo check --workspace
- rtk cargo test --workspace
- rtk cargo clippy --workspace --all-targets -- -D warnings

## Risks

- Alias authorization remains query-block scoped and collision conservative.
- Model correction is bounded to one additional request.
- Rollback by reverting the three fix commits.

## Self-Review

- [x] Scope matches Task 15 follow-up
- [x] Required format, build, tests, and lint passed
- [x] Full diff and commit history reviewed
- [x] No sensitive or unrelated files included
- [x] Compatibility and risk documented
- [x] Documentation and tests updated"
```

- [ ] **Step 4: Verify the PR state**

Run: `rtk gh pr view --json number,title,state,isDraft,baseRefName,headRefName,url,statusCheckRollup`

Expected: an open, non-draft PR from `fix/task-15-query-failure-recovery` to `master`; report any pending CI checks accurately.
