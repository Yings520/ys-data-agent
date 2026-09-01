# Requirements Document

## Introduction
{{INTRODUCTION}}

## Upstream Product Source
- **BMAD PRD**: {{BMAD_PRD_PATH}}
- **Source commit**: {{BMAD_PRD_COMMIT}}
- **Covered PRD sections**: {{BMAD_PRD_SECTIONS}}

> Product intent changes must be reconciled in the BMAD PRD before this contract is approved. Use an explicit path, commit, and section list; do not copy the whole PRD into this document.

<!-- Optional when scope could be misread or the feature touches adjacent systems/specs -->
## Boundary Context (Optional)
- **In scope**: {{IN_SCOPE_BEHAVIORS}}
- **Out of scope**: {{OUT_OF_SCOPE_BEHAVIORS}}
- **Adjacent expectations**: {{ADJACENT_SYSTEM_OR_SPEC_EXPECTATIONS}}

## Requirements

### Requirement 1: {{REQUIREMENT_AREA_1}}
<!-- Requirement headings MUST include a leading numeric ID only (for example: "Requirement 1: ...", "1. Overview", "2 Feature: ..."). Alphabetic IDs like "Requirement A" are not allowed. -->
**Objective:** As a {{ROLE}}, I want {{CAPABILITY}}, so that {{BENEFIT}}

#### Acceptance Criteria
1. When [event], the [system] shall [response/action]
2. If [trigger], then the [system] shall [response/action]
3. While [precondition], the [system] shall [response/action]
4. Where [feature is included], the [system] shall [response/action]
5. The [system] shall [response/action]

### Requirement 2: {{REQUIREMENT_AREA_2}}
**Objective:** As a {{ROLE}}, I want {{CAPABILITY}}, so that {{BENEFIT}}

#### Acceptance Criteria
1. When [event], the [system] shall [response/action]
2. When [event] and [condition], the [system] shall [response/action]

<!-- Additional requirements follow the same pattern -->
