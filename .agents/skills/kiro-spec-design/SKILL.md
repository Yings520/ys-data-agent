---
name: kiro-spec-design
description: Produce the authoritative technical design for one approved cc-sdd requirements document, including architecture boundaries, contracts, file ownership, and verification strategy.
metadata:
  shared-rules: "design-principles.md, design-discovery-full.md, design-discovery-light.md, design-synthesis.md, design-review-gate.md"
---

# cc-sdd Technical Design

Translate approved `requirements.md` into an implementation-ready `design.md`. Investigation is temporary working context; every decision needed downstream must be self-contained in `design.md`.

## Inputs and Approval Gate

Read completely:

- `.kiro/specs/$1/spec.json` and `requirements.md`;
- the existing `design.md`, when updating;
- relevant `.kiro/steering/` files and existing code patterns;
- `.kiro/settings/templates/specs/design.md`;
- every rule named in frontmatter.

Requirements must be human-approved in `spec.json`. With an explicit `-y`, record that intentional fast-track approval before proceeding; otherwise stop when approval is absent.

## Discovery

Choose the lightest sufficient investigation:

- **Minimal** for a local, well-understood change;
- **Light** for an extension to existing components;
- **Full** for new architecture or complex external integration.

Verify current external dependencies against primary documentation when they materially affect the design. Inspect real code for brownfield assumptions. Keep raw notes in working context and place concise evidence, alternatives, and decisions in the Supporting References and Design Decisions sections of `design.md`.

## Procedure

1. Map every numeric requirement ID to one or more components and verification points.
2. Make the boundary explicit before component detail:
   - what this feature owns;
   - what it does not own;
   - allowed dependencies and integration points;
   - which changes require revalidation.
3. Define architecture, component responsibilities, public contracts, data models, failure handling, security/performance implications, and rollout constraints.
4. Populate a concrete File Structure Plan. Each file or module must have one clear responsibility and be identified as create or modify.
5. Derive unit, integration, and end-to-end verification from the acceptance criteria. Avoid generic test placeholders.
6. Apply `rules/design-synthesis.md`, then run the bounded Design Review Gate. Repair local issues for at most two passes.
7. If review reveals missing or contradictory product behavior, stop and return to `requirements.md` instead of inventing it in the design.
8. Write `.kiro/specs/$1/design.md` only after the gate passes.
9. Update `spec.json`:
   - `phase: "design-generated"`;
   - `approvals.requirements.approved: true`;
   - `approvals.design.generated: true` and `approved: false`;
   - refresh `updated_at`.

## Output

Report the discovery depth, key decisions, requirements coverage, and review verdict. Ask the user to review `design.md`; after approval, continue with:

```text
$kiro-spec-tasks $1
```
