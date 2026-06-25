#!/usr/bin/env bash
# Build + deploy Forge API Docker image for one cluster (devnet preview or mainnet prod).
#
# Usage (as root, from repo checkout):
#   sudo bash scripts/docker/forge-deploy.sh --cluster devnet
#   sudo bash scripts/docker/forge-deploy.sh --cluster mainnet
#   sudo bash scripts/docker/forge-deploy.sh --cluster devnet --rollback
#
set -euo pipefail

CLUSTER="devnet"
SKIP_BUILD=0
NO_CACHE=0
ROLLBACK=0
SKIP_DB_CHECK=0
HEALTH_PORT=""
HEALTH_TIMEOUT=60
REPO_ROOT=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cluster) CLUSTER="$2"; shift 2;;
        --cluster=*) CLUSTER="${1#*=}"; shift;;
        --health-port) HEALTH_PORT="$2"; shift 2;;
        --health-timeout) HEALTH_TIMEOUT="$2"; shift 2;;
        --skip-build) SKIP_BUILD=1; shift;;
        --no-cache) NO_CACHE=1; shift;;
        --rollback) ROLLBACK=1; shift;;
        --skip-db-check) SKIP_DB_CHECK=1; shift;;
        --repo-root) REPO_ROOT="$2"; shift 2;;
        --repo-root=*) REPO_ROOT="${1#*=}"; shift;;
        -h|--help)
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -28
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 64;;
    esac
done

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 77; }

case "$CLUSTER" in
    devnet|mainnet) ;;
    *) echo "CLUSTER must be devnet or mainnet" >&2; exit 64;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
API_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

if [[ -z "$REPO_ROOT" ]]; then
    if command -v git >/dev/null 2>&1; then
        REPO_ROOT="$(git -C "$API_ROOT" rev-parse --show-toplevel 2>/dev/null || true)"
    fi
    REPO_ROOT="${REPO_ROOT:-$(cd "${API_ROOT}/.." && pwd)}"
fi

if [[ ! -f "${API_ROOT}/Cargo.toml" ]]; then
    echo "API root not found: ${API_ROOT}" >&2
    exit 65
fi

UNIT="forge-${CLUSTER}"
SERVICE="${UNIT}.service"
ENV_FILE="/etc/forge/${CLUSTER}.env"
IMAGE="forge-${CLUSTER}"
CONTAINER_NAME="$UNIT"

require_tool() {
    command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 65; }
}

require_tool docker
require_tool curl
require_tool jq

if ! systemctl list-unit-files "${SERVICE}" --no-legend 2>/dev/null | grep -q "^${SERVICE}"; then
    echo "systemd unit not installed: ${SERVICE}" >&2
    echo "run: sudo bash ${SCRIPT_DIR}/forge-install.sh" >&2
    exit 65
fi

detect_health_port() {
    [[ -n "$HEALTH_PORT" ]] && return
    if [[ -f "$ENV_FILE" ]]; then
        local bind
        bind="$(grep -E '^BIND_ADDR=' "$ENV_FILE" | head -1 | cut -d= -f2-)"
        if [[ "$bind" =~ ^([0-9.]+):([0-9]+)$ ]]; then
            HEALTH_PORT="${BASH_REMATCH[2]}"
            return
        fi
    fi
    HEALTH_PORT=$([[ "$CLUSTER" == devnet ]] && echo 8092 || echo 8093)
}

stop_cluster() {
    echo "[deploy] stopping ${SERVICE} and removing ${CONTAINER_NAME}…"
    systemctl stop "$SERVICE" 2>/dev/null || true
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
    systemctl reset-failed "$SERVICE" 2>/dev/null || true
}

warn_port_conflict() {
    detect_health_port
    local port="$HEALTH_PORT"
    if ! command -v ss >/dev/null 2>&1; then
        return 0
    fi
    if ss -ltn "sport = :$port" 2>/dev/null | grep -q LISTEN; then
        echo "[deploy] WARNING: port ${port} is already listening:" >&2
        ss -ltnp "sport = :$port" 2>&1 || true
        echo "[deploy] hint: stop the other forge unit or free port ${port}" >&2
    fi
}

preflight_database() {
    [[ "$SKIP_DB_CHECK" -eq 1 ]] && return 0
    if [[ -x "${SCRIPT_DIR}/forge-db-check.sh" ]]; then
        echo "[deploy] preflight DATABASE_URL…"
        bash "${SCRIPT_DIR}/forge-db-check.sh" --cluster "$CLUSTER"
    fi
}

prepare_service_start() {
    stop_cluster
}

probe_health() {
    local deadline=$((SECONDS + HEALTH_TIMEOUT))
    while (( SECONDS < deadline )); do
        if curl -fsS "http://127.0.0.1:${HEALTH_PORT}/health" 2>/dev/null \
            | jq -e '.status == "healthy" or .status == "ok"' >/dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    return 1
}

show_service_logs() {
    journalctl -u "$SERVICE" -n 40 --no-pager >&2 || true
}

verify_built_binary() {
    local image="$1"
    local size
    size="$(docker run --rm --entrypoint stat "$image" --format=%s /usr/local/bin/http402-forge-api 2>/dev/null || echo 0)"
    if [[ "$size" -lt 3000000 ]]; then
        echo "[deploy] ERROR: binary too small (${size} bytes)" >&2
        exit 65
    fi
    echo "[deploy] verified binary size=${size} bytes"
}

detect_health_port
stop_cluster
warn_port_conflict

if [[ "$ROLLBACK" -eq 1 ]]; then
    if ! docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
        echo "no ${IMAGE}:previous image to roll back to" >&2
        exit 65
    fi
    docker tag "${IMAGE}:previous" "${IMAGE}:current"
    preflight_database
    prepare_service_start
    systemctl restart "$SERVICE"
    probe_health || {
        show_service_logs
        echo "[deploy] rollback health failed" >&2
        stop_cluster
        exit 1
    }
    echo "[deploy] rolled back ${SERVICE} to :previous"
    exit 0
fi

SHA="$(git -C "$REPO_ROOT" rev-parse --short=12 HEAD 2>/dev/null || true)"
if [[ -z "$SHA" ]]; then
    echo "git SHA required from ${REPO_ROOT}" >&2
    exit 65
fi
IMAGE_SHA="${IMAGE}:${SHA}"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[deploy] building ${IMAGE_SHA} (cluster=${CLUSTER})"
    build_args=(--network host -f "${SCRIPT_DIR}/Dockerfile"
        --label "forge.cluster=${CLUSTER}"
        --label "forge.sha=${SHA}"
        -t "${IMAGE_SHA}"
        "$API_ROOT")
    [[ "$NO_CACHE" -eq 1 ]] && build_args=(--no-cache "${build_args[@]}")
    DOCKER_BUILDKIT=1 docker build "${build_args[@]}"
    verify_built_binary "${IMAGE_SHA}"
else
    docker image inspect "${IMAGE_SHA}" >/dev/null 2>&1 || {
        echo "--skip-build but ${IMAGE_SHA} missing" >&2
        exit 65
    }
fi

if docker image inspect "${IMAGE}:current" >/dev/null 2>&1; then
    docker tag "${IMAGE}:current" "${IMAGE}:previous"
fi
docker tag "${IMAGE_SHA}" "${IMAGE}:current"
preflight_database
prepare_service_start
systemctl restart "$SERVICE"
echo "[deploy] probing /health on port ${HEALTH_PORT}…"

if probe_health; then
    echo "[deploy] done ${SERVICE} sha=${SHA}"
    exit 0
fi

echo "[deploy] health check failed; rolling back…" >&2
show_service_logs
if docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
    docker tag "${IMAGE}:previous" "${IMAGE}:current"
    prepare_service_start
    systemctl restart "$SERVICE"
    if probe_health; then
        echo "[deploy] rolled back to :previous (new image failed health)" >&2
        exit 1
    fi
fi
stop_cluster
show_service_logs
exit 2
