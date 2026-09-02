import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// Every selected test injects a unique secret only at the protected input boundary and then
// asserts its absence from the named observable surface. Keep this list explicit so release
// automation cannot silently omit SQLite/WAL, telemetry, Vault/OAuth, upstream-error, or TUI
// coverage.
export const CANARY_TESTS = Object.freeze([
  ["cargo", "test", "-p", "ys-agent-runtime", "--test", "provider_secret_canary_test"],
  ["cargo", "test", "-p", "ys-agent-adapters", "--test", "credential_vault_test"],
  [
    "cargo",
    "test",
    "-p",
    "ys-agent-adapters",
    "provider_error_normalizer_classifies_known_liter_failures_without_echoing_canaries",
  ],
  [
    "cargo",
    "test",
    "-p",
    "ys-agent-adapters",
    "refresh_rotates_generation_and_invalid_refresh_fails_closed_without_echo",
  ],
  [
    "cargo",
    "test",
    "-p",
    "ysda",
    "failures_and_cancel_keep_only_non_sensitive_edit_data",
  ],
  [
    "cargo",
    "test",
    "-p",
    "ysda",
    "secret_handoff_is_move_only_and_oauth_cancel_returns_to_authentication",
  ],
]);

export function runCanaryGate(run = (command, args) =>
  spawnSync(command, args, { stdio: "inherit" })) {
  for (const args of CANARY_TESTS) {
    const result = run("rtk", args);
    if (result?.status !== 0) {
      throw new Error("provider secret canary contract failed");
    }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  runCanaryGate();
}
