#!/usr/bin/env bash
# Local devnet orchestrator for http402-forge-api.
#
# Usage:
#   # terminal 1: SKIP_SELLER_VAULT_CHECK=1 SKIP_SELLER_AUTH=1 cargo run
#   # terminal 2:
#   ./scripts/test-devnet.sh
#   SKIP_PAID_TESTS=1 ./scripts/test-devnet.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "${here}/common.sh"

run_step() {
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  bash "${here}/$1"
}

echo "== http402-forge-api test suite =="
echo "Service: ${SERVICE_BASE_URL}"

run_step test-health.sh
run_step test-list-create.sh

if [[ "${SKIP_PAID_TESTS:-0}" != "1" ]]; then
  if [[ -z "${PAYER_KEYPAIR:-}" ]]; then
    echo "SKIP_PAID_TESTS not set but PAYER_KEYPAIR missing — skipping paid download"
  else
    run_step test-paid-download.sh
  fi
fi

echo ""
echo "Forge API tests passed."
