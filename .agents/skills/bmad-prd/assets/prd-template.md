---
title: "<project name>"
status: draft
created: YYYY-MM-DD
updated: YYYY-MM-DD
---

# PRD: <project name>

> This is the single project-wide product, architecture, and evolution design. Do not create one PRD per Feature.

## 1. Document Purpose and Decision Scope

<What project-wide facts this document governs, which prior documents it supersedes, and which details remain Feature-local in cc-sdd.>

### Change Routing

- **Small change:** <criteria for direct bounded Code Agent execution>
- **Feature:** <criteria requiring cc-sdd requirements/design/tasks and direct `$kiro-impl` execution>

## 2. Product Context

### Problem

<Who has which problem today, and why it is worth solving.>

### Desired Outcome

<What changes for the target user if this succeeds.>

## 3. Target Users and Key Journeys

- **Primary user:** <role and context>
- **Key journey:** <starting state → action → observable outcome>

## 4. Goals and Non-Goals

### Goals

- <product outcome>

### Non-Goals

- <explicitly excluded adjacent capability>

## 5. Current Release Boundary

### In Scope

- <capability>

### Out of Scope

- <deferred capability and reason>

## 6. Long-Term Product Boundary

<What the project may become, which adjacent systems it deliberately does not reimplement, and how the current release relates to that direction.>

## 7. Functional Requirements

### FR-1: <capability name>

<User-visible behavior and conditions.>

**Acceptance boundaries:**

- <observable success condition>
- <important error or edge condition>

## 8. Non-Functional Requirements

- **NFR-1 — <quality>:** <measurable product constraint>

## 9. Success Metrics

- **SM-1:** <metric, target, and measurement window>

## 10. Stable Project Architecture

### Architecture Principles

- <durable cross-Feature principle>

### System and Runtime Boundaries

<Core components, responsibility boundaries, allowed dependency direction, and authority/control boundaries.>

### Core Domain and Data Model

<Only stable concepts and relationships shared across Features.>

### Security, Governance, and Completion Invariants

1. <Invariant that every Feature must preserve.>

### Repository Structure

<Durable module ownership and dependency rules; leave Feature-local file edits to cc-sdd.>

## 11. Constraints, Risks, and Assumptions

- **Constraint:** <fixed product constraint>
- **Risk:** <risk and product-level mitigation>
- **Assumption:** <fact that must be confirmed>

## 12. Open Project Decisions

- <Only unresolved decisions that materially affect product scope, durable architecture, or evolution order.>

## 13. Evolution Plan

1. **<phase or vertical slice>:** <user value, architectural capability unlocked, dependencies, and exit evidence>

Keep the sequence independently deliverable. Do not pre-create implementation tasks here.

## 14. Approval and Change History

- **Product owner:** <name>
- **Status:** Draft | Approved
- **Approved on:** YYYY-MM-DD
- **Superseded decisions:** <section or none>
