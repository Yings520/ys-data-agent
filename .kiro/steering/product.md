# Product Steering

## Product Direction

YS Data Agent is an Accountable-Data-Owner-governed AI data team for small and medium-sized businesses that cannot staff a complete data team. The system must absorb technical complexity while keeping business meaning, access, cost, and high-risk decisions with an accountable human owner.

## Current Release Boundary

v0.2 is a local trustworthy-query Pilot for Data Engineers and technical analysts. It accepts one natural-language question and produces one durable, verified Query Artifact or one explicit non-success state.

Supported intents:

- GovernedMetric backed by an Active metric contract;
- authorized AdHocRead with explicit assumptions;
- Metadata backed by observed evidence;
- clarification for material ambiguity;
- explicit UnsupportedCapability outside the release boundary.

## Explicit Exclusions

Do not silently expand v0.2 into Analysis, Build/Change, Operate, ML Data Prep, production writes, deployment, multi-user control plane, general ingestion, or infrastructure provisioning.

## Product Source of Truth

- `docs/PRD.md` is the project's only project-wide product, stable architecture, and evolution design. It owns product intent, release scope, project-level FR/NFR, non-goals, invariants, and evolution order.
- BMAD may create, update, or validate only that file. Do not create one PRD per Change or Feature, and do not place Feature-level requirements in it.
- Steering records only stable context needed across Changes.
- Every cc-sdd `requirements.md` must reference `docs/PRD.md` and the covered section IDs. If a spec conflicts with it, stop and reconcile before implementation.
