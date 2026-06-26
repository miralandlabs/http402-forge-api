#!/usr/bin/env bash
# Quick DATABASE_URL sanity check before forge-deploy (run on VPS).
#
#   sudo bash scripts/docker/forge-db-check.sh --cluster devnet
#
set -euo pipefail

CLUSTER="devnet"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --cluster) CLUSTER="$2"; shift 2;;
        --cluster=*) CLUSTER="${1#*=}"; shift;;
        -h|--help)
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -16
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 64;;
    esac
done

ENV_FILE="/etc/forge/${CLUSTER}.env"
[[ -f "$ENV_FILE" ]] || { echo "missing $ENV_FILE" >&2; exit 65; }

DATABASE_URL="$(grep -E '^DATABASE_URL=' "$ENV_FILE" | head -1 | cut -d= -f2- | tr -d '"')"
[[ -n "$DATABASE_URL" ]] || { echo "DATABASE_URL not set in $ENV_FILE" >&2; exit 65; }

SSL_ROOT_CERT="$(grep -E '^DATABASE_SSL_ROOT_CERT=' "$ENV_FILE" | head -1 | cut -d= -f2- | tr -d '"' || true)"
if [[ -z "$SSL_ROOT_CERT" ]]; then
    if [[ "$CLUSTER" == devnet ]]; then
        SSL_ROOT_CERT=/etc/forge/ssl/supabase-preview-ca.crt
    else
        SSL_ROOT_CERT=/etc/forge/ssl/supabase-prod-ca.crt
    fi
fi

echo "[db-check] cluster=${CLUSTER} env=${ENV_FILE}"

if [[ "$DATABASE_URL" == sqlite:* ]]; then
    path="${DATABASE_URL#sqlite:}"
    [[ "$path" == ./* ]] && path="/app/data/${path#./}"
    echo "[db-check] backend=sqlite path=${path} (no network probe; file created on first run)"
    exit 0
fi

python3 - <<'PY' "$DATABASE_URL"
import sys
from urllib.parse import urlparse, parse_qs

url = sys.argv[1]
parsed = urlparse(url)
host = parsed.hostname or ""
port = parsed.port or 5432
qs = parse_qs(parsed.query)
ssl = qs.get("sslmode", [""])[0]
user = parsed.username or ""

print(f"[db-check] backend=postgres host={host} port={port} user={user} sslmode={ssl or '(missing)'}")

if ("supabase.co" in host or "supabase.com" in host) and ssl not in ("require", "verify-full", "verify-ca"):
    print("[db-check] ERROR: Supabase URLs need ?sslmode=require", file=sys.stderr)
    sys.exit(2)

if not host:
    print("[db-check] ERROR: could not parse DATABASE_URL host", file=sys.stderr)
    sys.exit(2)
PY

hostport="$(python3 - <<'PY' "$DATABASE_URL"
import sys
from urllib.parse import urlparse
p = urlparse(sys.argv[1])
print(f"{p.hostname}:{p.port or 5432}")
PY
)"

host="${hostport%:*}"
port="${hostport#*:}"

if command -v nc >/dev/null 2>&1; then
    echo "[db-check] probing TCP ${host}:${port} …"
    if nc -z -w 5 "$host" "$port" 2>/dev/null; then
        echo "[db-check] TCP OK"
    else
        echo "[db-check] ERROR: cannot reach ${host}:${port}" >&2
        echo "[db-check] hint: Supabase → Database → Network → allow this VPS public IP" >&2
        echo "[db-check] hint: if Direct host is IPv6-only, use Session pooler (port 5432) or IPv4 add-on" >&2
        exit 3
    fi
else
    echo "[db-check] install netcat-openbsd for TCP probe (optional)"
fi

if [[ "$DATABASE_URL" == *supabase.co* || "$DATABASE_URL" == *supabase.com* ]]; then
    if [[ -z "$SSL_ROOT_CERT" || ! -f "$SSL_ROOT_CERT" ]]; then
        echo "[db-check] ERROR: Supabase needs DATABASE_SSL_ROOT_CERT in ${ENV_FILE}" >&2
        echo "[db-check] hint: devnet → /etc/forge/ssl/supabase-preview-ca.crt, mainnet → /etc/forge/ssl/supabase-prod-ca.crt" >&2
        echo "[db-check] hint: Supabase Dashboard → Database → SSL Configuration → download CA" >&2
        exit 4
    fi
    echo "[db-check] ssl root cert=${SSL_ROOT_CERT}"

    if command -v docker >/dev/null 2>&1; then
        echo "[db-check] probing Postgres TLS (psql via docker) …"
        if docker run --rm \
            --env-file "$ENV_FILE" \
            -e PGSSLMODE=require \
            -e "PGSSLROOTCERT=${SSL_ROOT_CERT}" \
            -v /etc/forge/ssl:/etc/forge/ssl:ro \
            postgres:16-alpine \
            psql "$DATABASE_URL" -c 'SELECT 1' >/dev/null 2>&1; then
            echo "[db-check] Postgres TLS OK"
        else
            echo "[db-check] ERROR: Postgres TLS handshake failed (same failure the API would hit)" >&2
            echo "[db-check] hint: verify password is URL-encoded; use Session pooler host; check IP allowlist" >&2
            exit 5
        fi
    else
        echo "[db-check] install Docker for full TLS probe (TCP-only check passed)"
    fi
fi

echo "[db-check] done"
