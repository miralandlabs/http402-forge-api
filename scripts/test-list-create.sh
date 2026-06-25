#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "${here}/common.sh"

require_cmds curl jq

seller="${SELLER_WALLET:-}"
if [[ -z "${seller}" ]]; then
  if seller_from_key="$(payer_pubkey 2>/dev/null)"; then
    seller="${seller_from_key}"
  else
    seller="11111111111111111111111111111111"
  fi
fi

tmp_asset="$(mktemp)"
echo "forge e2e test asset $(date -u +%s)" > "${tmp_asset}"
trap 'rm -f "${tmp_asset}"' RETURN

resp="$(curl -sS -X POST "${SERVICE_BASE_URL}/api/v1/listings" \
  -F "seller_wallet=${seller}" \
  -F "title=E2E prompt pack" \
  -F "description=automated test listing" \
  -F "category=prompt_pack" \
  -F "price_usdc=0.05" \
  -F "agent_friendly=true" \
  -F "asset=@${tmp_asset};type=text/plain")"

echo "${resp}" | jq -e '.id' >/dev/null
listing_id="$(jq -r '.id' <<<"${resp}")"
echo "created listing ${listing_id}"

code="$(curl -sS -o /dev/null -w '%{http_code}' "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}/download")"
if [[ "${code}" != "402" ]]; then
  echo "error: download should return 402, got ${code}" >&2
  exit 1
fi
echo "402 smoke ok for ${listing_id}"
echo "${listing_id}" > "${here}/.last-listing-id"
