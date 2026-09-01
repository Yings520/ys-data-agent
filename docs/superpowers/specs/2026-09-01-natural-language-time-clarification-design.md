# Natural-Language Time Clarification Design

## Problem

Ys-data currently exposes an internal query-plan requirement to end users by asking them for RFC3339 UTC timestamps. A normal user should be able to answer with phrases such as “yesterday”, “last week”, “最近 30 天”, or “2026 年 7 月”. The captured GMV Run also revealed that a Run which stops for clarification cannot be scheduled again because the production background scheduler never releases its active Run ID.

## User Experience Contract

- Users never need to provide RFC3339, UTC offsets, SQL, artifact IDs, Run IDs, or other internal protocol values.
- Clarification uses the same language as the user and asks one concise business question.
- A time-range question offers natural examples, such as “yesterday, last week, or July 2026”.
- Natural answers are preserved as clarification evidence. The model converts them to the internal half-open RFC3339 UTC range using trusted runtime temporal context.
- If a phrase is still materially ambiguous, the workflow asks one natural-language follow-up. It does not ask the user to perform the conversion.
- After the answer is accepted, the same Run resumes automatically.

## Considered Approaches

### 1. Runtime temporal context plus model normalization — selected

Provide `current_time_utc` and `workspace_timezone` in runtime-owned query state. Keep RFC3339 in the internal metric-plan schema, but explicitly prohibit it in user-facing clarification questions. This supports multilingual natural language without building a second date-language subsystem.

### 2. A deterministic phrase parser

Parse a fixed vocabulary such as “yesterday” and “last week” before the model sees it. This is predictable for a small English-only set, but it scales poorly across languages, locale conventions, fiscal calendars, and expressions such as “the last complete business week”.

### 3. Always show normalized dates for confirmation

Normalize natural language and require a second confirmation before every query. This is safe but adds friction to simple, read-only requests and contradicts the one-sentence interaction goal.

## Architecture

### Prompt boundary

The query system instructions distinguish two contracts:

- user-facing clarification: plain language, same language as the user, no internal formats;
- internal metric plan: RFC3339 UTC `start` and `end` values.

The deterministic material-ambiguity fallback uses the same user-facing wording.

### Trusted temporal context

`HarnessConfig` carries the configured workspace timezone. Each model step adds runtime-owned `current_time_utc` and `workspace_timezone` fields to `RUNTIME_QUERY_STATE_JSON`. Clarification evidence remains untrusted user content, while the temporal fields remain trusted runtime context.

The production assembly passes `YSDA_TIMEZONE`; deterministic evaluation passes its configured fixture timezone. A missing timezone remains a readiness blocker rather than being guessed.

### Resumable scheduling

`BackgroundScheduler` treats its Run-ID set as an active-execution guard, not a permanent history. It removes the Run ID after `LoopDriver::run` returns, whether the result is waiting, terminal, or failed. A duplicate schedule while execution is active is still coalesced; a later clarification resume schedules a new execution.

## Error Handling

- Missing workspace timezone continues to fail readiness checks; the agent never guesses one.
- A scheduler driver error logs the existing warning, releases the Run ID, and permits an explicit later retry.
- Ambiguous natural language triggers another plain-language clarification rather than accepting an invented range.
- Internal plan validation continues to require valid RFC3339 UTC boundaries and a non-empty half-open interval.

## Tests

- Prompt contract: every phase forbids exposing RFC3339 and internal identifiers in clarification; the Plan phase still documents RFC3339 as an internal schema.
- Fallback clarification: ambiguous time wording produces a natural example question without “RFC3339” or “UTC”.
- Runtime state: model requests contain trusted current time and configured workspace timezone.
- Scheduler lifecycle: the same Run can be scheduled again after the first driver invocation returns Waiting, while concurrent duplicate scheduling remains coalesced.
- Existing query, recovery, SQL-policy, TUI, and full-workspace gates remain green.

## Success Criteria

1. “查询 GMV” may ask which business period to use, but never asks for a timestamp format.
2. Answering “昨天” or “上周” resumes the same Run automatically.
3. The model receives enough trusted context to create the internal UTC range without asking the user to convert it.
4. Existing policy, evidence, and audit guarantees are unchanged.
