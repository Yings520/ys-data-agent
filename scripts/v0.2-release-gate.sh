#!/usr/bin/env bash
set -euo pipefail

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

rtk env \
  YSDA_TEST_POSTGRES_URL=postgres://ysda:ysda-test@127.0.0.1:55432/ysda_test \
  cargo test \
  -p ys-agent-adapters \
  --test postgres_connector_test \
  -- --ignored

rtk cargo test -p ysda --test query_eval_test
rtk cargo test -p ysda model_protocol_probe --lib
rtk cargo test \
  -p ys-agent-adapters \
  --test model_provider_test \
  returns_the_original_tool_call_id_with_a_tool_result
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
