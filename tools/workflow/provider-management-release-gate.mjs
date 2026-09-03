import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// These are deliberately small, named contracts. They are the Provider-management additions to
// the broader release gate and each failure has a stable non-secret code for CI diagnostics.
const STEP_NAMES = Object.freeze([
  "evidence",
  "doctor",
  "error_metrics",
  "secret_canary",
]);

export const PROVIDER_MANAGEMENT_RELEASE_STEPS = Object.freeze([
  ["cargo", "test", "-p", "ys-agent-runtime", "--test", "provider_evidence_gate_test"],
  ["cargo", "test", "-p", "ys-agent-runtime", "provider_doctor_tests"],
  [
    "cargo",
    "test",
    "-p",
    "ys-agent-adapters",
    "provider_error_normalizer_classifies_known_liter_failures_without_echoing_canaries",
  ],
  ["node", "tools/workflow/provider-secret-canary.mjs"],
]);

export function runProviderManagementReleaseGate(
  run = (command, args) => spawnSync(command, args, { stdio: "inherit" }),
) {
  for (const [index, args] of PROVIDER_MANAGEMENT_RELEASE_STEPS.entries()) {
    const result = run("rtk", args);
    if (result?.status !== 0) {
      throw new Error(`provider.release_gate.${STEP_NAMES[index]}_failed`);
    }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    runProviderManagementReleaseGate();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
