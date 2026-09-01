---
name: kiro-spec-init
description: Initialize one cc-sdd Feature from the approved project design in docs/PRD.md. Creates only spec control metadata and the initial requirements document; small Changes must not invoke it.
---

# cc-sdd Feature Initialization

Create one engineering specification under `.kiro/specs/<feature>/`.

## Inputs

Accept a Feature description in `$ARGUMENTS`. It should reference the canonical project design `docs/PRD.md` and the project sections or evolution direction it implements.

Before writing, confirm the description contains:

- who has the problem;
- the current behavior or limitation;
- the desired user-visible change;
- the relevant `docs/PRD.md` section.

Ask only for missing information that would materially change feature scope. Do not invent product intent.

## Procedure

1. Read `docs/PRD.md` and extract only the scope relevant to this Feature.
2. Generate a stable lowercase feature slug using `[a-z0-9._-]`.
3. Check `.kiro/specs/` for a naming conflict. If the feature already exists, stop and ask whether to update it; do not create a silent duplicate.
4. Read:
   - `.kiro/settings/templates/specs/init.json`;
   - `.kiro/settings/templates/specs/requirements-init.md`.
5. Create `.kiro/specs/<feature>/spec.json` and `.kiro/specs/<feature>/requirements.md`.
6. In `requirements.md`, record the source PRD path and a concise project description. Do not generate the full requirements, design, or tasks yet.
7. Set `spec.json.language` from the requested document language and retain `spec.json` as machine-readable approval/control state.

## Output

Report the feature name, the two created paths, and the next command:

```text
$kiro-spec-requirements <feature>
```

The final human-maintained feature documents are `requirements.md`, `design.md`, and `tasks.md`; `spec.json` is control metadata, not a fourth planning document.
