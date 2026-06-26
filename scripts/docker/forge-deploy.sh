#!/usr/bin/env bash
# Build + deploy Forge API Docker image for one cluster (devnet preview or mainnet prod).
#
# Mirrors mintforge/scripts/docker/mintforge-deploy.sh:
#   preflight → docker build → tag :current → systemctl restart → /health → auto-rollback
#
# Usage (as root, from repo checkout):
#   sudo bash scripts/docker/forge-deploy.sh --cluster devnet
#   sudo bash scripts/docker/forge-deploy.sh --cluster mainnet
#   sudo bash scripts/docker/forge-deploy.sh --cluster devnet --rollback
#   sudo bash scripts/docker/forge-deploy.sh --cluster devnet --skip-build
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
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -32
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
    REPO_ROOT="${REPO_ROOT:-$API_ROOT}"
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

image_id() {
    docker image inspect --format='{{.Id}}' "$1" 2>/dev/null || true
}

# Avoid Docker/containerd AlreadyExists when retagging :current (untag before promote).
promote_sha_to_current() {
    local image_sha="$1"
    local cur_id sha_id

    sha_id="$(image_id "$image_sha")"
    [[ -n "$sha_id" ]] || { echo "[deploy] missing image ${image_sha}" >&2; exit 65; }

    if docker image inspect "${IMAGE}:current" >/dev/null 2>&1; then
        cur_id="$(image_id "${IMAGE}:current")"
        if [[ "$cur_id" == "$sha_id" ]]; then
            echo "[deploy] ${image_sha} already ${IMAGE}:current"
            return 0
        fi
        docker tag "${IMAGE}:current" "${IMAGE}:previous"
        echo "[deploy] saved ${IMAGE}:current → :previous"
        docker rmi "${IMAGE}:current" >/dev/null 2>&1 || true
    fi

    docker tag "${image_sha}" "${IMAGE}:current"
    echo "[deploy] tagged ${image_sha} → ${IMAGE}:current"
}

restore_previous_as_current() {
    if ! docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
        return 1
    fi
    local cur_id prev_id
    cur_id="$(image_id "${IMAGE}:current")"
    prev_id="$(image_id "${IMAGE}:previous")"
    if [[ -n "$cur_id" && "$cur_id" == "$prev_id" ]]; then
        echo "[deploy] ${IMAGE}:current already matches :previous"
        return 0
    fi
    docker rmi "${IMAGE}:current" >/dev/null 2>&1 || true
    docker tag "${IMAGE}:previous" "${IMAGE}:current"
    echo "[deploy] restored ${IMAGE}:previous → :current"
}

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

sync_systemd_unit() {
    local unit_file="${SCRIPT_DIR}/${UNIT}.service"
    [[ -f "$unit_file" ]] || return 0
    if cmp -s "$unit_file" "/etc/systemd/system/${SERVICE}" 2>/dev/null; then
        return 0
    fi
    echo "[deploy] updating systemd unit ${SERVICE} from repo"
    install -m 0644 "$unit_file" "/etc/systemd/system/${SERVICE}"
    systemctl daemon-reload
}

prepare_service_start() {
    echo "[deploy] stopping ${SERVICE} and removing ${CONTAINER_NAME}…"
    systemctl stop "$SERVICE" 2>/dev/null || true
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
    systemctl reset-failed "$SERVICE" 2>/dev/null || true
    ensure_health_port_free
}

ensure_health_port_free() {
    local port="$HEALTH_PORT"
    if ! command -v ss >/dev/null 2>&1; then
        return 0
    fi
    if ! ss -ltn "sport = :$port" 2>/dev/null | grep -q LISTEN; then
        return 0
    fi
    echo "[deploy] port ${port} still listening after stop; freeing…" >&2
    ss -ltnp "sport = :$port" 2>&1 || true
    if command -v fuser >/dev/null 2>&1; then
        fuser -k "${port}/tcp" >/dev/null 2>&1 || true
        sleep 1
    fi
    if pgrep -x http402-forge-api >/dev/null 2>&1; then
        echo "[deploy] stopping stray http402-forge-api host process…" >&2
        pkill -x http402-forge-api 2>/dev/null || true
        sleep 1
    fi
    if ss -ltn "sport = :$port" 2>/dev/null | grep -q LISTEN; then
        echo "[deploy] ERROR: port ${port} still in use — stale forge process?" >&2
        ss -ltnp "sport = :$port" 2>&1 || true
        exit 65
    fi
}

warn_port_conflict() {
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

# Empty 404 = axum route missing (stale binary). JSON 404 = route ok, listing absent.
probe_delist_route() {
    local url="http://127.0.0.1:${HEALTH_PORT}/api/v1/seller/delist-challenge"
    url+="?seller_wallet=buyA5hR1Z9KtHQRBTmLkjsFfjAabDwdZtrRC6edqxAJ"
    url+="&listing_id=00000000-0000-0000-0000-000000000001"
    local attempt body code
    for attempt in 1 2 3 4 5; do
        body="$(curl -sS "$url" 2>/dev/null || true)"
        code="$(curl -sS -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
        if [[ "$code" == "200" ]]; then
            echo "[deploy] delist-challenge route registered"
            return 0
        fi
        if [[ "$code" == "404" && -n "$body" ]]; then
            echo "[deploy] delist-challenge route registered"
            return 0
        fi
        if [[ "$code" == "403" || "$code" == "422" ]]; then
            echo "[deploy] delist-challenge route registered (HTTP ${code})"
            return 0
        fi
        echo "[deploy] delist-challenge probe attempt ${attempt}/5 (HTTP ${code}, body len=${#body})"
        sleep 2
    done
    echo "[deploy] ERROR: delist-challenge probe failed (HTTP ${code}, body len=${#body})" >&2
    echo "[deploy] hint: stale process on port ${HEALTH_PORT} or old image — re-run with --no-cache" >&2
    return 1
}

capture_container_startup_error() {
    echo "[deploy] --- forge startup probe (prints process stderr) ---" >&2
    local probe_name="${CONTAINER_NAME}-probe-$$"
    timeout 12 docker run --rm --name "$probe_name" \
        --network host --pull=never \
        --env-file "$ENV_FILE" \
        -e LOCAL_STORAGE_PATH=/app/data/objects \
        -v "/var/lib/forge/${CLUSTER}/data:/app/data" \
        -v "/etc/forge/ssl:/etc/forge/ssl:ro" \
        --memory 1536m --cpus 1.5 \
        "${IMAGE}:current" 2>&1 | tail -40 >&2 || true
    echo "[deploy] --- journalctl app logs (tag=forge-${CLUSTER}) ---" >&2
    journalctl -t "forge-${CLUSTER}" -n 30 --no-pager >&2 || true
}

show_deploy_failure() {
    echo "[deploy] diagnostics (why /health failed):" >&2
    systemctl status "$SERVICE" --no-pager -l >&2 || true
    echo "[deploy] --- journalctl -u ${SERVICE} (last 40 lines) ---" >&2
    journalctl -u "$SERVICE" -n 40 --no-pager >&2 || true
    if ! systemctl is-active --quiet "$SERVICE"; then
        capture_container_startup_error
    fi
    echo "[deploy] common fixes:" >&2
    echo "[deploy]   sudo bash ${SCRIPT_DIR}/forge-db-check.sh --cluster ${CLUSTER}" >&2
    echo "[deploy]   journalctl -t forge-${CLUSTER} -n 50" >&2
    echo "[deploy]   curl -v http://127.0.0.1:${HEALTH_PORT}/health" >&2
    echo "[deploy]   sudo -e ${ENV_FILE}" >&2
}

verify_built_binary() {
    local image="$1"
    local size
    size="$(docker run --rm --entrypoint stat "$image" --format=%s /usr/local/bin/http402-forge-api 2>/dev/null || echo 0)"
    if [[ "$size" -lt 3000000 ]]; then
        echo "[deploy] ERROR: binary too small (${size} bytes)" >&2
        echo "[deploy] re-run: sudo bash $0 --cluster ${CLUSTER} --no-cache" >&2
        exit 65
    fi
    echo "[deploy] verified binary size=${size} bytes"
}

detect_health_port

if [[ "$ROLLBACK" -eq 1 ]]; then
    if ! docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
        echo "no ${IMAGE}:previous image to roll back to" >&2
        exit 65
    fi
    preflight_database
    restore_previous_as_current
    sync_systemd_unit
    prepare_service_start
    systemctl restart "$SERVICE"
    if probe_health; then
        echo "[deploy] rolled back ${SERVICE} to :previous"
        exit 0
    fi
    show_deploy_failure
    echo "[deploy] rollback health failed" >&2
    exit 1
fi

preflight_database
warn_port_conflict

SHA="$(git -C "$REPO_ROOT" rev-parse --short=12 HEAD 2>/dev/null || true)"
if [[ -z "$SHA" ]]; then
    echo "git SHA required from ${REPO_ROOT}" >&2
    exit 65
fi
IMAGE_SHA="${IMAGE}:${SHA}"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[deploy] building ${IMAGE_SHA} from ${API_ROOT} (cluster=${CLUSTER})"
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
    echo "[deploy] reusing existing image ${IMAGE_SHA}"
fi

promote_sha_to_current "${IMAGE_SHA}"
sync_systemd_unit
prepare_service_start
systemctl restart "$SERVICE"
echo "[deploy] restarted ${SERVICE}; probing /health on port ${HEALTH_PORT}…"

if probe_health; then
    echo "[deploy] /health → healthy"
    if ! probe_delist_route; then
        show_deploy_failure
        exit 1
    fi
    echo "[deploy] done ${SERVICE} sha=${SHA} port=${HEALTH_PORT}"
    echo "[deploy] roll back: sudo bash $0 --cluster ${CLUSTER} --rollback"
    exit 0
fi

echo "[deploy] /health did not flip to healthy within ${HEALTH_TIMEOUT}s" >&2
show_deploy_failure
echo "[deploy] auto-rolling back to :previous (if available)…" >&2
if restore_previous_as_current; then
    sync_systemd_unit
    prepare_service_start
    systemctl restart "$SERVICE"
    if probe_health; then
        echo "[deploy] rolled back; /health → healthy" >&2
        exit 1
    fi
    show_deploy_failure
    echo "[deploy] rollback also failed; manual intervention required" >&2
    exit 2
fi
prepare_service_start
echo "[deploy] no :previous image; manual intervention required" >&2
exit 2
