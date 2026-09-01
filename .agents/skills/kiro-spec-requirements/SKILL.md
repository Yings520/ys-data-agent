---
name: kiro-spec-requirements
description: Refine an initialized cc-sdd feature into testable user-visible requirements while preserving the approved PRD product scope.
metadata:
  shared-rules: "ears-format.md, requirements-review-gate.md"
---

# cc-sdd Requirements

Turn product intent into the engineering workflow's authoritative `requirements.md`. Requirements define **what behavior is required**, not how it will be implemented.

## Inputs

Read completely:

- `.kiro/specs/$1/spec.json`;
- `.kiro/specs/$1/requirements.md`;
- the canonical project design `docs/PRD.md` and its relevant project sections;
- relevant `.kiro/steering/` files;
- `rules/ears-format.md` and `rules/requirements-review-gate.md`;
- `.kiro/settings/templates/specs/requirements.md`.

For brownfield work, inspect existing behavior and contracts that constrain user-visible requirements. Keep investigation in working context; persist conclusions in `requirements.md` only.

## Procedure

1. Confirm the feature is within the PRD's approved scope. If it changes product goals or MVP boundaries, stop and return the conflict to the PRD.
2. Clarify only genuine ambiguity in scope, business rules, error behavior, security expectations, performance expectations, or edge cases.
3. Draft requirements in the language set by `spec.json`:
   - use numeric requirement headings;
   - express every acceptance criterion in EARS form;
   - keep criteria observable and testable;
   - state inclusions, exclusions, and adjacent expectations where scope could be misread;
   - preserve PRD terminology and requirement traceability.
4. Run the bounded Requirements Review Gate. Repair local issues for at most two passes. If a real product ambiguity remains, ask the user instead of guessing.
5. Write `.kiro/specs/$1/requirements.md` only after the gate passes.
6. Update `spec.json`:
   - `phase: "requirements-generated"`;
   - `approvals.requirements.generated: true`;
   - refresh `updated_at`.

## Boundary

Keep technology choices, architecture, APIs, schemas, component ownership, file layout, and implementation sequencing out of requirements. Those belong in `design.md` and `tasks.md`.

## Output

Summarize the requirement groups, confirm the review gate passed, and ask the user to review the file. After approval, continue with:

```text
$kiro-spec-design $1
```

Do not approve requirements on the user's behalf unless they explicitly invoke an intentional fast-track option.
