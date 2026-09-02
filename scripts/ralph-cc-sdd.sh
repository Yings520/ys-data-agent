#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  rtk echo "usage: scripts/ralph-cc-sdd.sh <feature>" >&2
  exit 2
fi

feature="$1"
if [[ ! "${feature}" =~ ^[a-z0-9][a-z0-9._-]*$ ]]; then
  rtk echo "invalid feature name: ${feature}" >&2
  exit 2
fi

CC_SDD_PR_BASE="${CC_SDD_PR_BASE:-master}" \
  rtk node tools/workflow/cc-sdd-publish.mjs --recover "${feature}"
rtk node tools/workflow/cc-sdd-to-ralph.mjs "${feature}"
rtk ralph-tui run \
  --prd ".ralph-tui/generated/${feature}.json" \
  --serial
