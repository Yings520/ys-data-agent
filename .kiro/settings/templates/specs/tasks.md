# Implementation Plan

## Task Format Template

Use whichever pattern fits the work breakdown:

### Major task only
- [ ] {{NUMBER}}. {{TASK_DESCRIPTION}}{{PARALLEL_MARK}}
  - {{DETAIL_ITEM_1}}
  - {{OBSERVABLE_COMPLETION_ITEM}} *(State what must be observably true when done.)*
  - _Requirements: {{REQUIREMENT_IDS}}_
  - _Boundary: {{FILE_OR_COMPONENT_BOUNDARIES}}_
  - _Depends: {{TASK_IDS_OR_NONE}}_

### Major + Sub-task structure
- [ ] {{MAJOR_NUMBER}}. {{MAJOR_TASK_SUMMARY}}
- [ ] {{MAJOR_NUMBER}}.{{SUB_NUMBER}} {{SUB_TASK_DESCRIPTION}}{{SUB_PARALLEL_MARK}}
  - {{DETAIL_ITEM_1}}
  - {{OBSERVABLE_COMPLETION_ITEM}} *(State what must be observably true when done.)*
  - _Requirements: {{REQUIREMENT_IDS}}_ *(IDs only; do not add descriptions or parentheses.)*
  - _Boundary: {{FILE_OR_COMPONENT_BOUNDARIES}}_
  - _Depends: {{TASK_IDS_OR_NONE}}_ *(Use `none` when the task has no prerequisites.)*

> **Ralph dispatch contract**: Every executable task must carry `_Requirements:_`, `_Boundary:_`, `_Depends:_`, and at least one observable completion bullet. These fields are machine-checked before Ralph starts. A task without them is not dispatchable.

> **Parallel marker**: Append ` (P)` only to tasks that can be executed in parallel. Omit the marker when running in `--sequential` mode.
>
> **Optional test coverage**: When a sub-task is deferrable test work tied to acceptance criteria, mark the checkbox as `- [ ]*` and explain the referenced requirements in the detail bullets. Optional tasks still enter the Ralph projection unless explicitly removed during human task approval.
