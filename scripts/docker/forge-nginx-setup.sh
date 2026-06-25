#!/usr/bin/env bash
# nginx TLS reverse proxy for Forge API (preview.* subdomain pattern).
#
#   preview.forge.http402.trade  -> 127.0.0.1:8092  (devnet / preview)
#   forge.http402.trade          -> 127.0.0.1:8093  (mainnet / production)
#
# Usage:
#   sudo bash forge-nginx-setup.sh --certbot-email ops@http402.trade
#
set -euo pipefail

PREVIEW_HOST="${PREVIEW_HOST:-preview.forge.http402.trade}"
PROD_HOST="${PROD_HOST:-forge.http402.trade}"
PREVIEW_PORT="${PREVIEW_PORT:-8092}"
PROD_PORT="${PROD_PORT:-8093}"
CERTBOT_EMAIL=""
RUN_CERTBOT=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --preview-host) PREVIEW_HOST="$2"; shift 2;;
        --preview-host=*) PREVIEW_HOST="${1#*=}"; shift;;
        --prod-host) PROD_HOST="$2"; shift 2;;
        --prod-host=*) PROD_HOST="${1#*=}"; shift;;
        --preview-port) PREVIEW_PORT="$2"; shift 2;;
        --prod-port) PROD_PORT="$2"; shift 2;;
        --certbot-email) CERTBOT_EMAIL="$2"; RUN_CERTBOT=1; shift 2;;
        --certbot-email=*) CERTBOT_EMAIL="${1#*=}"; RUN_CERTBOT=1; shift;;
        -h|--help)
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -20
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 64;;
    esac
done

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 77; }

SITE="forge-api"
SITE_PATH="/etc/nginx/sites-available/${SITE}"

cat >"$SITE_PATH" <<EOF
# Forge API — preview + production (managed by forge-nginx-setup.sh)

server {
    listen 80;
    server_name ${PREVIEW_HOST};
    location / {
        proxy_pass http://127.0.0.1:${PREVIEW_PORT};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        client_max_body_size 55m;
    }
}

server {
    listen 80;
    server_name ${PROD_HOST};
    location / {
        proxy_pass http://127.0.0.1:${PROD_PORT};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        client_max_body_size 55m;
    }
}
EOF

ln -sf "$SITE_PATH" "/etc/nginx/sites-enabled/${SITE}"
nginx -t
systemctl reload nginx

if [[ "$RUN_CERTBOT" -eq 1 ]]; then
    [[ -n "$CERTBOT_EMAIL" ]] || { echo "--certbot-email required" >&2; exit 64; }
    certbot --nginx -n --agree-tos -m "$CERTBOT_EMAIL" \
        -d "$PREVIEW_HOST" -d "$PROD_HOST"
fi

echo "[nginx] preview API: https://${PREVIEW_HOST}"
echo "[nginx] production API: https://${PROD_HOST}"
