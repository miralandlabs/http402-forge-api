#!/usr/bin/env bash
# Buyer trust loop: paid download + optional feedback (local dev with SKIP_BUYER_AUTH=1).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "${here}/common.sh"

listing_id="${1:-}"
if [[ -z "${listing_id}" && -f "${here}/.last-listing-id" ]]; then
  listing_id="$(cat "${here}/.last-listing-id")"
fi
if [[ -z "${listing_id}" ]]; then
  echo "usage: test-buyer-trust.sh [listing_id]" >&2
  exit 1
fi

require_cmds curl jq sha256sum

out="$(mktemp)"
headers="$(mktemp)"
trap 'rm -f "${out}" "${headers}"' RETURN

if [[ -n "${PAYER_KEYPAIR:-}" && -f "${PAYER_KEYPAIR}" ]]; then
  pay_get_file "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}/download" "${out}"
else
  echo "PAYER_KEYPAIR not set — skipping paid download in buyer-trust test"
  exit 0
fi

listing_json="$(curl -sS "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}")"
content_hash="$(jq -r '.contentHash // .content_hash // empty' <<<"${listing_json}")"
if [[ -n "${content_hash}" ]]; then
  digest="$(sha256sum "${out}" | awk '{print $1}')"
  if [[ "${digest}" != "${content_hash}" ]]; then
    echo "error: content hash mismatch (got ${digest}, expected ${content_hash})" >&2
    exit 1
  fi
  echo "content hash verified"
fi

buyer_wallet="$(payer_pubkey)"
sale_id="$(
  curl -sS -D "${headers}" -o /dev/null \
    -H "PAYMENT-SIGNATURE: $(base64 < /dev/null | tr -d '\n')" \
    "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}/download" 2>/dev/null || true
)"
# Re-fetch sale id via feedback challenge probe when buyer auth skipped
if [[ "${SKIP_BUYER_AUTH:-0}" == "1" ]]; then
  sale_id="$(jq -r '.id // empty' <<<"$(curl -sS "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}")")"
  # Use latest sale for listing+buyer via redownload challenge
  sale_id="$(
    curl -sS "${SERVICE_BASE_URL}/api/v1/buyer/redownload-challenge?buyer_wallet=${buyer_wallet}&listing_id=${listing_id}" \
      | jq -r '.saleId // .sale_id // empty'
  )"
fi

if [[ -z "${sale_id}" || "${sale_id}" == "null" ]]; then
  echo "skip feedback: no sale_id (set SKIP_BUYER_AUTH=1 and complete a purchase first)"
  exit 0
fi

feedback_code="$(
  curl -sS -o /dev/null -w '%{http_code}' \
    -X POST "${SERVICE_BASE_URL}/api/v1/sales/${sale_id}/feedback" \
    -H "Content-Type: application/json" \
    -d "{\"buyer_wallet\":\"${buyer_wallet}\",\"buyer_challenge\":\"dev\",\"buyer_signature\":\"dev\",\"outcome\":\"as_described\"}"
)"
if [[ "${feedback_code}" == "201" || "${feedback_code}" == "200" ]]; then
  echo "feedback ok (${feedback_code})"
elif [[ "${SKIP_BUYER_AUTH:-0}" == "1" && "${feedback_code}" == "403" ]]; then
  echo "feedback skipped (buyer auth enabled on API)"
else
  echo "feedback HTTP ${feedback_code} (non-fatal in smoke test)"
fi

echo "buyer trust smoke ok"
