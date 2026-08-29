# TUI Terminal Mockup Design

## Purpose

Provide a standalone browser mockup that lets an operator see the proposed v0.2 `ysda` terminal UI before its Rust/Ratatui implementation exists. It is a visual aid only; it does not connect to runtime services or execute queries.

## Scope

The page depicts the default focused terminal screen described in Task 13 and supports visual-only transitions for `/doctor`, `/details`, `/new`, and `/quit`.

It does not recreate the TUI event loop, persist Sessions or Runs, call `AgentServiceApi`, expose artifacts, or change product Rust code.

## Layout and content

The mockup is a dark terminal window with an accessible high-contrast, monospace presentation:

- Header: product name plus safe Workspace, connection, permission, model, and Doctor status labels.
- Transcript: a welcome message, an example business question, and an example result area.
- Prompt: an editable command-style input with command hints.
- Status/repair panel: `/doctor` shows blocker or warning codes and curated repair guidance without secret values.
- Diagnostics panel: `/details` alone reveals illustrative internal Session, Task, Run, Step, and QueryPhase identifiers.

## Simulated interactions

| Input | Visual result |
|---|---|
| `/doctor` | Replaces the main area with safe readiness state, codes, capability list, and repairs. |
| `/details` | Toggles the diagnostics panel; normal content does not show internal identifiers. |
| `/new` | Adds a local "new session created" confirmation without implying cancellation of work. |
| `/quit` | Shows a detached-UI confirmation, not a cancelled-run confirmation. |
| Other text | Shows a non-executing example query/submission state. |

## Safety and fidelity

The mockup uses fixed, fictional labels. It includes no API keys, passwords, database URLs, or live data. Doctor failure disables the simulated query submission path. The page is explicitly labelled as a browser visualization, rather than the production Ratatui implementation.

## Verification

Open the page locally and confirm that it:

1. Resembles a full-screen terminal rather than a dashboard.
2. Keeps internal identifiers hidden until `/details`.
3. Shows clear, safe Doctor repair guidance.
4. Makes `/new` and `/quit` semantics unambiguous.
