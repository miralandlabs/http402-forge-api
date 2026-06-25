#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "${here}/common.sh"

require_cmds curl jq

body="$(curl -sS "${SERVICE_BASE_URL}/health")"
echo "${body}" | jq -e '.status == "ok"' >/dev/null
echo "health ok"

resources="$(curl -sS "${SERVICE_BASE_URL}/.well-known/x402-resources.json")"
echo "${resources}" | jq -e '.resources | length >= 1' >/dev/null
echo "x402-resources ok"
