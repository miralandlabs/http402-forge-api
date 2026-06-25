#!/usr/bin/env bash
# One-time VPS bootstrap for http402-forge-api (dual-cluster Docker + systemd).
#
#   sudo bash scripts/docker/forge-install.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
API_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
REPO_ROOT="$(cd "${API_ROOT}" && pwd)"

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 77; }

echo "[install] host packages"
bash "${SCRIPT_DIR}/forge-host-prereqs.sh"

echo "[install] directories"
install -d -m 0750 /etc/forge
install -d -m 0755 /etc/forge/ssl
install -d -m 0750 /var/lib/forge/devnet/data
install -d -m 0750 /var/lib/forge/mainnet/data

if [[ ! -f /etc/forge/devnet.env ]]; then
    install -m 0640 "${SCRIPT_DIR}/forge-devnet.env.example" /etc/forge/devnet.env
    echo "[install] created /etc/forge/devnet.env — edit before deploy"
fi
if [[ ! -f /etc/forge/mainnet.env ]]; then
    install -m 0640 "${SCRIPT_DIR}/forge-mainnet.env.example" /etc/forge/mainnet.env
    echo "[install] created /etc/forge/mainnet.env — edit before deploy"
fi

install -m 0644 "${SCRIPT_DIR}/forge-devnet.service" /etc/systemd/system/forge-devnet.service
install -m 0644 "${SCRIPT_DIR}/forge-mainnet.service" /etc/systemd/system/forge-mainnet.service
systemctl daemon-reload
systemctl enable forge-devnet.service forge-mainnet.service

cat <<EOF

[install] Forge API host ready.
Repo expected at: ${REPO_ROOT}

Next:
  1. sudo -e /etc/forge/devnet.env
  2. sudo -e /etc/forge/mainnet.env
  3. sudo bash ${SCRIPT_DIR}/forge-deploy.sh --cluster devnet
  4. sudo bash ${SCRIPT_DIR}/forge-deploy.sh --cluster mainnet
  5. sudo bash ${SCRIPT_DIR}/forge-nginx-setup.sh --certbot-email ops@example.com

EOF
