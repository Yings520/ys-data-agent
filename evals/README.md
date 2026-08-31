# YS Data Agent Query Evals

`query_cases.jsonl` is the deterministic v0.2 Query release dataset. Each physical line is one complete JSON object and one independently executable case.

## What the gate checks

The gate checks four layers:

1. outcome: terminal or waiting status, intent, metric, relation, warnings, clarification, and failure code;
2. trajectory: phase order, allowed Tool calls, preflight before execute, verification before completion, and model-call budget;
3. security: forbidden Tools and relations, restricted result text, Context injection, and secret canaries;
4. reproducibility: case schema, fixture, Replay, Prompt, ContextManifest, and ToolView versions or hashes.

## Required rule for a production bug

Every production bug fix must add one deterministic regression case before the fix is released. The case must fail against the broken behavior and pass after the fix.

Do not delete a failing case merely to release. Fix the product or document and review an intentional contract change.

## Adding a case

1. Choose a unique stable `id`.
2. Set `schema_version`, `fixture_version`, and `replay_version` explicitly.
3. Write one user question.
4. Select or add a minimal fixture variant.
5. Add a complete fixed Replay response sequence. Never call a live model.
6. Declare one expected terminal or waiting status.
7. Add the narrowest positive expectations needed for the behavior.
8. Add forbidden Tools, relations, or answer fragments for the relevant failure hypothesis.
9. Run the single Query Eval test and the full release gate.

## Changing an expectation

Expectation changes require review approval. Explain whether the product contract, fixture, or Replay sequence changed and why the new behavior remains safe.

Never edit an expectation only because the current implementation produced a different value.

## Determinism before an LLM Judge

Prefer exact codes, typed fields, set equality, ordering, hashes, and bounded counts. A future LLM Judge may score prose quality, but it cannot replace deterministic safety and trajectory checks.

## Data rules

- Never use raw production data.
- Never store credentials, tokens, DSNs, customer names, or customer emails.
- Synthetic canaries must be obviously fake.
- Result rows used by fixtures must be synthetic and minimal.
- Eval output must not serialize restricted result rows.

## Version rules

- Increment `schema_version` when the JSONL document shape changes incompatibly.
- Increment `fixture_version` when SQLite, metric, dbt, or policy facts change.
- Increment `replay_version` when model response sequences change.
- Record Prompt, ContextManifest, ToolView, model, and Tool versions in each `EvalObservation`.

## Commands

```bash
rtk cargo test -p ysda --test query_eval_test
rtk cargo test -p ys-agent-runtime --test telemetry_test
```
