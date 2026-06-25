#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "${here}/common.sh"

listing_id="${1:-}"
if [[ -z "${listing_id}" && -f "${here}/.last-listing-id" ]]; then
  listing_id="$(cat "${here}/.last-listing-id")"
fi
if [[ -z "${listing_id}" ]]; then
  echo "usage: test-paid-download.sh [listing_id]" >&2
  exit 1
fi

out="$(mktemp)"
trap 'rm -f "${out}"' RETURN

pay_get_file "${SERVICE_BASE_URL}/api/v1/listings/${listing_id}/download" "${out}"
if [[ ! -s "${out}" ]]; then
  echo "error: empty download" >&2
  exit 1
fi
echo "paid download ok ($(wc -c < "${out}") bytes)"
