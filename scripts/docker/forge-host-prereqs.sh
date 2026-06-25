#!/usr/bin/env bash
# Host packages for Forge API VPS (Docker CE + nginx helpers).
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

apt-get update
apt-get install -y --no-install-recommends \
    ca-certificates curl git jq nginx certbot python3-certbot-nginx

if ! command -v docker >/dev/null 2>&1; then
    apt-get install -y --no-install-recommends docker.io
    systemctl enable --now docker
fi
