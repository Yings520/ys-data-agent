#!/usr/bin/env bash
set -euo pipefail

rtk node --test tools/workflow/*.test.mjs
rtk node tools/workflow/provider-management-release-gate.mjs

compose_file="fixtures/postgres/compose.yaml"
compose_project="ysda-v02-release-gate"

cleanup() {
  rtk docker compose \
    --project-name "${compose_project}" \
    --file "${compose_file}" \
    down --volumes --remove-orphans || true
}

trap cleanup EXIT INT TERM

rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace

rtk docker compose \
  --project-name "${compose_project}" \
  --file "${compose_file}" \
  config --quiet

rtk docker compose \
  --project-name "${compose_project}" \
  --file "${compose_file}" \
  up --detach --wait

rtk cargo test \
  -p ys-agent-adapters \
  --test datasource_release_evidence_test \
  -- --nocapture

rtk docker compose \
  --project-name "${compose_project}" \
  --file "${compose_file}" \
  exec --no-TTY postgres \
  psql --username ysda --dbname ysda_test --tuples-only --no-align \
  --command 'SHOW server_version'

rtk cargo test \
  -p ys-agent-adapters \
  --test managed_sqlite_test \
  -- --nocapture

rtk cargo test \
  -p ys-agent-adapters \
  --test managed_duckdb_test \
  -- --nocapture

rtk cargo test \
  -p ys-agent-adapters \
  --test managed_postgres_test \
  -- --ignored --nocapture

rtk env \
  YSDA_TEST_POSTGRES_URL=postgres://ysda:ysda-test@127.0.0.1:55432/ysda_test \
  cargo test \
  -p ys-agent-adapters \
  --test postgres_connector_test \
  -- --ignored

rtk cargo test \
  -p ys-agent-runtime \
  --test datasource_runtime_test \
  service_runs_real_postgres_save_validate_and_select \
  -- --ignored --nocapture

rtk env \
  YSDA_TEST_POSTGRES_PASSWORD=ysda-reader-test \
  cargo test \
  -p ysda \
  --test datasource_tui_test \
  real_three_driver_forms_save_validate_select_and_set_default \
  -- --ignored --nocapture

rtk cargo test -p ysda --test query_eval_test
rtk cargo test -p ysda model_protocol_probe --lib
rtk cargo test \
  -p ys-agent-adapters \
  tool_call_and_result_round_trip_preserves_the_provider_id
rtk cargo test \
  -p ys-agent-adapters \
  request_and_multi_turn_tool_result_preserve_the_provider_call_id
rtk cargo test -p ysda --test doctor_test
rtk cargo test -p ysda --test export_test
rtk cargo test -p ysda --test tui_test
rtk cargo test -p ysda tui::composer
rtk cargo test -p ysda tui::palette
rtk cargo test -p ysda tui::theme

if rtk cargo tree -p ysda | rtk rg -q 'vtcode-ui'; then
  rtk echo 'unexpected vtcode-ui dependency' >&2
  exit 1
fi
if rtk rg -n 'Color::|Rgb\(|Indexed\(' apps/ysda/src/tui/ui.rs; then
  rtk echo 'renderer must use YsdaTheme semantic tokens only' >&2
  exit 1
fi
