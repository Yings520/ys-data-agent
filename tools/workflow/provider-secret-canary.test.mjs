import assert from "node:assert/strict";
import test from "node:test";

import { CANARY_TESTS, runCanaryGate } from "./provider-secret-canary.mjs";

test("provider secret canary gate executes every required output-surface contract", () => {
  const calls = [];
  runCanaryGate((command, args) => {
    calls.push([command, args]);
    return { status: 0 };
  });

  assert.deepEqual(
    calls,
    CANARY_TESTS.map((args) => ["rtk", args]),
  );
});

test("provider secret canary gate rejects any failed surface contract", () => {
  assert.throws(
    () => runCanaryGate(() => ({ status: 1 })),
    /provider secret canary contract failed/,
  );
});
