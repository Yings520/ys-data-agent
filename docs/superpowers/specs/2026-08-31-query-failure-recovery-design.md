# Query Failure Recovery Design

## Problem

Three user-visible failures were observed in the v0.2 Query TUI:

- A valid ad-hoc query using `SELECT ... AS sale_date` and `ORDER BY sale_date` was rejected as `column_not_allowed`. Replanning repeated until the Run failed with `loop_model_call_budget_exceeded`.
- A governed GMV request resolved the Active metric successfully, but a malformed clarification action without the required `question` field terminated the Run as `invalid_model_response`.
- Submitting an unknown slash-prefixed input such as `/你好` cleared the composer after showing an `unknown command` warning, making recovery needlessly destructive.

The fixes must preserve fail-closed SQL authorization, bounded model interaction, the existing command namespace, and persisted Runtime formats.

## Required Behavior

- SQL expressions may be referenced by a declared result alias in clauses supported by the SQL dialect without treating the alias as a source column.
- Every real source column remains subject to the configured `AllowedDataScope`; alias handling must not make a denied or unknown source column readable.
- Query prompts state the exact typed `request_clarification` action contract wherever clarification is permitted.
- One malformed free-form model action receives at most one protocol-correction request. A second malformed response remains a terminal failure.
- Protocol correction never echoes the malformed response, tool arguments, business data, or secrets.
- An invalid slash command shows an actionable warning and preserves the composer text for correction.
- `/你好` is not silently converted into a chat message; `/` continues to identify the command namespace.

## Considered Approaches

### Layered minimal recovery (selected)

Teach the SQL policy about declared output aliases, make the clarification JSON contract explicit in Query prompts, reuse the provider's existing single protocol-correction loop for malformed typed actions, and preserve invalid command drafts in the TUI. This addresses each observed boundary while retaining current module ownership.

### Prompt-only correction

Adding an exact clarification example to the prompt is smaller, but model compliance is probabilistic. The same malformed action could still terminate an otherwise recoverable Run.

### Full SQL name-resolution engine

Resolving every identifier by query block, relation alias, output alias, and dialect would be more complete, but it is a substantially larger parser project than the reported defect requires. It increases review and security risk for this patch.

## Design

### SQL policy

`SqlReadOnlyPolicy` recognizes aliases declared by `SelectItem::ExprWithAlias` only in alias-eligible ordering expressions of the same query block. The expression that produces the alias is still traversed, so every underlying source column is validated normally. Identifiers used in projections, filters, joins, or another query block are never exempted merely because the same name is declared as an alias elsewhere.

If an alias name also exists in the configured source scope, the source-column policy wins. This conservative collision rule prevents an alias from masking a denied or otherwise governed real column. The change does not broaden allowed relations, wildcard behavior, functions, statements, or mutation rules.

### Model protocol recovery

Query phase instructions include the exact clarification shape:

```json
{"type":"request_clarification","question":"<one concise question>"}
```

`OpenAiCompatibleProvider::complete` extends its existing bounded protocol-correction path to `invalid_model_response` errors produced after a syntactically valid provider response cannot be deserialized as an `AgentAction`. The correction message contains only the safe validation error and an instruction to return one valid typed JSON action. It does not include the rejected content.

The provider performs at most one correction across a call. If correction fails, the original fail-closed behavior remains. Transport retry limits are unchanged.

### TUI command recovery

`submit_composer` no longer calls `composer.submit()` when `parse_input` returns an error. The warning explains that slash-prefixed text is interpreted as a command and can be corrected by deleting `/` or choosing a command from `/help`. Valid commands and normal chat submissions retain current behavior.

## Data Flow

1. The TUI parses a submission. Invalid command syntax produces a warning and retains the draft.
2. A valid data request enters the existing Query workflow.
3. Exact phase instructions guide the model to a typed tool call, query plan, clarification, or completion.
4. A malformed typed action receives one bounded correction; otherwise the existing terminal failure path applies.
5. Ad-hoc SQL reaches `SqlReadOnlyPolicy`, which authorizes source expressions and recognizes declared output aliases without treating them as physical columns.
6. Existing preflight, execution, verification, Artifact, and answer packaging behavior continues unchanged.

## Compatibility and Risk

There are no public API, persistence schema, Artifact format, dependency, or configuration changes. Valid SQL previously accepted remains accepted. The only additional provider cost is one correction request after a malformed typed action; it is bounded by the existing protocol-correction counter.

The main security risk is over-broad alias exemption. It is mitigated by limiting recognition to alias-eligible ordering expressions in the same query block, continuing to validate every alias expression, and giving configured source-column policies precedence when a name collides with an alias.

## Verification

- Add an SQL policy regression test for the observed daily-sales query with `ORDER BY sale_date`; assert that its physical columns remain the only referenced governed columns.
- Add collision and nested-query tests proving that a declared alias cannot mask a denied source column or authorize the same identifier in another clause or query block.
- Add a provider integration test where the first response omits `question` and the second response returns a valid `request_clarification`; assert exactly two HTTP requests and no rejected content in the correction message.
- Retain a failure test proving that two malformed actions fail after one correction.
- Add a TUI test proving `/你好` produces an actionable warning and leaves `/你好` in the composer.
- Run focused tests first, then formatting, workspace check, workspace tests, and clippy with warnings denied.
