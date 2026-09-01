# Fresh-Evidence Completion Gate

Verify the exact claim against the current code state. Earlier output, Agent prose, and a checked box are not evidence.

## Task Claim

Require all of the following:

- task-local tests and mechanical checks were rerun and exited successfully;
- review returned a parseable `APPROVED` verdict;
- the diff remains within the approved task boundary;
- no blocking finding or missing mandatory environment remains.

## Feature Claim

Require all of the following:

- complete canonical test suite result;
- trustworthy runtime smoke result;
- full requirements coverage;
- cross-task contract and shared-state consistency;
- end-to-end design and boundary alignment;
- no blocked executable tasks.

Return exactly one result:

```md
## Verification Result
- STATUS: VERIFIED | NOT_VERIFIED | MANUAL_VERIFY_REQUIRED
- CLAIM: <exact task or feature claim>
- EVIDENCE: <fresh commands/checks and results>
- GAPS: <missing or mismatched evidence>
```

Use `MANUAL_VERIFY_REQUIRED` when a mandatory environment or manual check is unavailable. Never widen a claim beyond its evidence.
