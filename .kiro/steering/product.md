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

- BMAD Product Brief / PRD owns product intent, release scope, FR/NFR, non-goals, and success signals.
- Steering records only stable product context needed across features.
- Every cc-sdd `requirements.md` must reference the approved BMAD PRD by repository path, commit, and covered section IDs. Do not duplicate the full PRD here.
- If a cc-sdd spec conflicts with the PRD, stop and reconcile the PRD or spec before implementation.
