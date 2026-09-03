import assert from "node:assert/strict";
import test from "node:test";

import {
  PROVIDER_MANAGEMENT_RELEASE_STEPS,
  runProviderManagementReleaseGate,
} from "./provider-management-release-gate.mjs";

test("Provider management release gate keeps evidence, Doctor, canary, and error contracts mandatory", () => {
  const calls = [];
  runProviderManagementReleaseGate((command, args) => {
    calls.push([command, args]);
    return { status: 0 };
  });

  assert.deepEqual(
    calls,
    PROVIDER_MANAGEMENT_RELEASE_STEPS.map((args) => ["rtk", args]),
  );
});

test("Provider management release gate exposes the failed contract and exits nonzero", () => {
  assert.throws(
    () => runProviderManagementReleaseGate(() => ({ status: 1 })),
    /provider\.release_gate\.evidence_failed/,
  );
});
