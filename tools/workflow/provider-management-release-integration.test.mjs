import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("v0.2 main release gate runs the Provider management release gate before workspace validation", async () => {
  const script = await readFile("scripts/v0.2-release-gate.sh", "utf8");
  const providerGate = "rtk node tools/workflow/provider-management-release-gate.mjs";

  assert.ok(script.includes(providerGate));
  assert.ok(script.indexOf(providerGate) < script.indexOf("rtk cargo fmt --all --check"));
});

test("v0.2 main release gate runs the existing adapter tool-call identity contract from its library target", async () => {
  const script = await readFile("scripts/v0.2-release-gate.sh", "utf8");
  const toolCallIdentityContracts = [
    "tool_call_and_result_round_trip_preserves_the_provider_id",
    "request_and_multi_turn_tool_result_preserve_the_provider_call_id",
  ];

  for (const contract of toolCallIdentityContracts) {
    assert.ok(script.includes(contract));
  }
  assert.doesNotMatch(script, /--test model_provider_test/);
});
