# Task 15 Runtime Readiness Fixes Design

## Context

Task 15 release verification found three gaps in the existing runtime. The public configuration names an estimated-cost limit and an Artifact retention period, but production composition does not consume them. Workspace Doctor also declares every configured OpenAI-compatible endpoint ready without exercising the protocol that Query needs. A real crash/resume smoke test therefore passed durable recovery but later reached `invalid_model_response` after a Tool result.

## Goals

- Make every documented runtime limit effective and fail closed on invalid values.
- Apply the configured retention period to the existing time-bounded Artifact paths that currently hard-code seven days.
- Make Doctor exercise the same model adapter and Tool-result message path used by Query before permitting submission.
- Keep probe content synthetic, isolated from Runtime state and business data, and bounded to two small model requests per process.
- Preserve deterministic release tests: the automated gate continues to use Fake, Replay, and Wiremock providers and never contacts a live model.

## Non-goals

- Accepting malformed model output or weakening the typed `AgentAction` contract.
- Adding provider-specific parsing for DeepSeek or any other vendor.
- Probing context-window size by consuming a large context.
- Changing Session-retained Internal Query Artifacts into day-retained Artifacts.

## Design

### Configuration and budgets

`AppConfig::from_env` will parse optional `YSDA_QUERY_MAX_ESTIMATED_COST_UNITS` into `QueryBudget.max_estimated_cost_units`. Empty means unsupported/not configured; a supplied value must be a positive integer. `YSDA_ARTIFACT_RETENTION_DAYS` remains required and must be a positive `u32`. Invalid values abort bootstrap with a typed validation error instead of silently retaining defaults.

The configured QueryBudget already flows into `HarnessConfig`; once populated, PostgreSQL preflight performs `EXPLAIN (FORMAT JSON)` and returns its existing typed cost rejection or confirmation result before execution.

### Retention

Production composition will pass the configured day count to both `InProcessAgentService` and `ArtifactExporter`. Existing constructors retain a seven-day default for tests and embedders; new explicit constructors carry production policy.

The setting applies to the two paths currently hard-coded to seven days:

- Restricted clarification evidence uses `RetentionPolicy::Days` and a matching `expires_at`.
- Export Artifacts use `RetentionPolicy::Days` and a matching `expires_at`.

Session-retained Query, Result, SQL, and verification evidence retain their existing semantics.

### Live model protocol probe

The app-level Doctor probe will hold the same `Arc<dyn ModelProvider>` used by the Harness. On its first inspection in a process, it will execute an isolated two-stage handshake:

1. Send a synthetic request containing one harmless `ysda_doctor_probe` tool and require the normalized result to be a real `AgentAction::CallTool` with a provider Tool Call ID.
2. Send a second synthetic request containing a Tool-result message with that exact ID and require a normalized non-tool action.

Because both requests use `OpenAiCompatibleProvider::complete`, they exercise HTTP reachability, authentication, response JSON shape, Tool Calls, Tool Call IDs, Tool-result serialization, and typed `AgentAction` parsing. The probe contains no database schema, query, Artifact, customer identifier, or credential value. It writes no Task, Run, Event, or Artifact.

The result is cached for the life of the process so TUI startup and later submit checks do not repeat model calls. Any transport, authentication, malformed-response, missing-ID, unexpected-action, or second-turn failure maps to model readiness false. Doctor then emits its existing `model_protocol_incompatible` blocker and query submission remains disabled. Repairing a failed endpoint requires restarting the process and rerunning Doctor.

### Error handling

- Invalid cost value: typed `invalid_query_budget` bootstrap failure.
- Invalid retention value: typed `invalid_artifact_retention` bootstrap failure.
- Probe failure: safe Doctor blocker; raw provider output and credentials are not included.
- A provider that returns plain text instead of typed action JSON remains incompatible; the adapter will not guess or coerce it.

## Verification

- RED/GREEN AppConfig tests prove both environment values reach runtime fields and invalid values fail closed.
- Runtime service/export tests prove configured days appear in policy and expiry metadata.
- Fake-model tests prove the two-stage probe preserves the Tool Call ID, caches the verdict, and fails closed.
- Wiremock adapter tests continue to prove Tool result serialization and secret-safe errors.
- Full v0.2 release gate passes and cleans the named Compose project.
- Live Doctor against the configured endpoint reports a blocker if the endpoint repeats the observed incompatible response; no Query is submitted in that state.

## Delivery criterion

The code is deliverable when all automated checks pass, independent review has no Critical or Important findings, crash/resume still preserves Task and Run identity, and Doctor truthfully blocks the currently incompatible endpoint unless it completes the two-stage probe.
