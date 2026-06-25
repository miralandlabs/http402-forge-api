#!/usr/bin/env bash
# Shared helpers for http402-forge-api integration scripts.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FORGE_ROOT="$(cd "${here}/.." && pwd)"

if [[ -f "${here}/test.env" ]]; then
  # shellcheck source=/dev/null
  source "${here}/test.env"
fi

SERVICE_BASE_URL="${SERVICE_BASE_URL:-http://127.0.0.1:8092}"
PAYER_KEYPAIR="${PAYER_KEYPAIR:-}"
SELLER_WALLET="${SELLER_WALLET:-}"
SIGN_JS="${SIGN_JS:-${here}/sign.js}"
PAY_ASSET_MINT="${PAY_ASSET_MINT:-4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU}"
DEFAULT_PR402="${PR402_FACILITATOR_URL:-https://preview.ipay.sh}"
DEFAULT_PR402="${DEFAULT_PR402%/}"

require_cmds() {
  local missing=0
  for cmd in "$@"; do
    if ! command -v "${cmd}" >/dev/null 2>&1; then
      echo "error: required command not found: ${cmd}" >&2
      missing=1
    fi
  done
  [[ "${missing}" -eq 0 ]]
}

require_payer_keypair() {
  if [[ -z "${PAYER_KEYPAIR}" || ! -f "${PAYER_KEYPAIR}" ]]; then
    echo "error: set PAYER_KEYPAIR to a valid keypair json path" >&2
    exit 1
  fi
}

require_sign_js() {
  if [[ ! -f "${SIGN_JS}" ]]; then
    echo "error: sign.js not found" >&2
    exit 1
  fi
  if [[ ! -d "${here}/node_modules/@solana/web3.js" ]]; then
    echo "run: npm install --prefix \"${here}\"" >&2
    exit 1
  fi
}

payer_pubkey() {
  require_cmds solana-keygen
  require_payer_keypair
  solana-keygen pubkey "${PAYER_KEYPAIR}"
}

mktemp_json() {
  mktemp "${TMPDIR:-/tmp}/forge.XXXXXX.json"
}

pay_get_file() {
  local url="$1"
  local out_file="$2"
  local mint="${3:-${PAY_ASSET_MINT}}"

  require_cmds curl jq
  require_payer_keypair
  require_sign_js

  local tmp402
  tmp402="$(mktemp_json)"
  trap 'rm -f "${tmp402}"' RETURN

  local http_code
  http_code="$(
    curl -sS -o "${tmp402}" -w '%{http_code}' "${url}"
  )"
  if [[ "${http_code}" != "402" ]]; then
    echo "error: expected 402, got ${http_code}" >&2
    cat "${tmp402}" >&2 || true
    return 1
  fi

  local accept_line facilitator resource build_accept payer_pubkey
  accept_line="$(jq -c --arg m "${mint}" '.accepts[] | select(.asset == $m)' "${tmp402}" | head -n 1)"
  facilitator="$(jq -r '.extra.capabilitiesUrl // empty' <<<"${accept_line}")"
  if [[ -z "${facilitator}" || "${facilitator}" == "null" ]]; then
    facilitator="${DEFAULT_PR402}/api/v1/facilitator"
  else
    facilitator="${facilitator%/capabilities}"
  fi
  resource="$(jq -c '.resource' "${tmp402}")"
  build_accept="$(jq -c 'if .scheme == "v2:solana:exact" then . + {"scheme":"exact"} else . end' <<<"${accept_line}")"
  payer_pubkey="$(payer_pubkey)"

  local build_res unsigned_tx verify_template signed_tx proof proof_b64
  build_res="$(
    curl -sS -X POST "${facilitator}/build-exact-payment-tx" \
      -H "Content-Type: application/json" \
      -d "{
        \"payer\": \"${payer_pubkey}\",
        \"accepted\": ${build_accept},
        \"resource\": ${resource}
      }"
  )"
  unsigned_tx="$(jq -r '.transaction // empty' <<<"${build_res}")"
  verify_template="$(jq -c '.verifyBodyTemplate' <<<"${build_res}")"
  signed_tx="$(
    node --no-deprecation "${SIGN_JS}" "${PAYER_KEYPAIR}" "${unsigned_tx}"
  )"
  proof="$(jq -c --arg tx "${signed_tx}" '.paymentPayload.payload.transaction = $tx' <<<"${verify_template}")"
  proof_b64="$(printf '%s' "${proof}" | base64 | tr -d '\n')"

  local paid_code
  paid_code="$(
    curl -sS -o "${out_file}" -w '%{http_code}' \
      -H "PAYMENT-SIGNATURE: ${proof_b64}" \
      "${url}"
  )"
  if [[ "${paid_code}" != "200" ]]; then
    echo "error: paid download expected 200, got ${paid_code}" >&2
    cat "${out_file}" >&2 || true
    return 1
  fi
}
