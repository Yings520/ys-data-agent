# Task-Local Review Protocol

Review the selected task adversarially against the approved spec and actual
diff. Do not trust an implementation summary as evidence.

## Required Checks

- Run the task-relevant canonical tests and static checks; reject on failure.
- Confirm behavioral work includes relevant RED evidence.
- Compare every changed file with the task `_Boundary:_`; reject undeclared
  spillover or hidden coupling.
- Read the referenced numeric requirements and confirm observable behavior
  covers them completely.
- Read the relevant design sections and confirm contracts, file
  responsibilities, and dependency direction match.
- Reject placeholders, stubs, unrelated refactors, hardcoded secrets, swallowed
  errors, and tests that would pass without the implementation.
- Escalate instead of guessing when the spec is ambiguous or technically
  impossible.

## Verdict

Return this exact structure:

```md
## Review Verdict
- VERDICT: APPROVED | REJECTED
- TASK: <task-id>
- TESTS: PASS | FAIL (command and exit code)
- STATIC_CHECKS: PASS | FAIL | NOT_APPLICABLE
- BOUNDARY: WITHIN | VIOLATION
- RED_PHASE: VERIFIED | MISSING | NOT_APPLICABLE
- FINDINGS: <specific findings with file/spec references>
- REMEDIATION: <required when rejected>
```

Only a parseable `APPROVED` verdict permits completion verification.
