#!/usr/bin/env bash
# One-time host environment verification before deployment of http402-forge-api.
#
#   sudo bash scripts/docker/forge-bootstrap-verify.sh
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Forge API Host Bootstrap Verifier ===${NC}\n"

ERRORS=0
WARNINGS=0

check_dir() {
    local dir="$1"
    local required="$2"
    if [[ -d "$dir" ]]; then
        echo -e "  [${GREEN}OK${NC}] Directory $dir exists"
    else
        if [[ "$required" -eq 1 ]]; then
            echo -e "  [${RED}FAIL${NC}] Directory $dir is missing (Required)"
            ERRORS=$((ERRORS + 1))
        else
            echo -e "  [${YELLOW}WARN${NC}] Directory $dir is missing (Optional)"
            WARNINGS=$((WARNINGS + 1))
        fi
    fi
}

check_env_file() {
    local cluster="$1"
    local path="/etc/forge/${cluster}.env"
    if [[ ! -f "$path" ]]; then
        echo -e "  [${RED}FAIL${NC}] Environment file $path is missing"
        echo -e "         hint: copy scripts/docker/forge-${cluster}.env.example to $path"
        ERRORS=$((ERRORS + 1))
        return
    fi
    echo -e "  [${GREEN}OK${NC}] Environment file $path exists"

    # Check DATABASE_URL
    local db_url
    db_url="$(grep -E '^DATABASE_URL=' "$path" | head -1 | cut -d= -f2- | tr -d '"' || true)"
    if [[ -z "$db_url" ]]; then
        echo -e "  [${RED}FAIL${NC}] DATABASE_URL is not configured in $path"
        ERRORS=$((ERRORS + 1))
        return
    fi

    if [[ "$db_url" == *supabase.co* || "$db_url" == *supabase.com* ]]; then
        echo -e "         Detected Supabase host in $path"

        # Check DATABASE_SSL_ROOT_CERT
        local cert_path
        cert_path="$(grep -E '^DATABASE_SSL_ROOT_CERT=' "$path" | head -1 | cut -d= -f2- | tr -d '"' || true)"
        if [[ -z "$cert_path" ]]; then
            # Default fallback paths
            if [[ "$cluster" == "mainnet" ]]; then
                cert_path="/etc/forge/ssl/supabase-prod-ca.crt"
            else
                cert_path="/etc/forge/ssl/supabase-preview-ca.crt"
            fi
            echo -e "  [${YELLOW}INFO${NC}] DATABASE_SSL_ROOT_CERT not explicitly set in $path."
            echo -e "         Will fall back to default path inside container: $cert_path"
        fi

        # Check if the CA certificate file exists on the host
        if [[ -f "$cert_path" ]]; then
            echo -e "  [${GREEN}OK${NC}] SSL root certificate file exists at $cert_path"
            if [[ ! -r "$cert_path" ]]; then
                echo -e "  [${RED}FAIL${NC}] Certificate file $cert_path is not readable"
                ERRORS=$((ERRORS + 1))
            fi
        else
            echo -e "  [${RED}FAIL${NC}] SSL root certificate file not found at $cert_path"
            echo -e "         hint: Download CA certificate from Supabase Dashboard → Database → SSL Configuration"
            echo -e "         and save it to $cert_path"
            ERRORS=$((ERRORS + 1))
        fi
    else
        echo -e "         Detected non-Supabase/local database backend"
    fi
}

check_service_file() {
    local cluster="$1"
    local path="/etc/systemd/system/forge-${cluster}.service"
    if [[ ! -f "$path" ]]; then
        echo -e "  [${RED}FAIL${NC}] systemd unit file $path is missing"
        echo -e "         hint: run 'sudo bash scripts/docker/forge-install.sh' to bootstrap"
        ERRORS=$((ERRORS + 1))
        return
    fi
    echo -e "  [${GREEN}OK${NC}] systemd unit file $path exists"

    # Check for volume mount of SSL directory
    if grep -q "forge/ssl" "$path"; then
        echo -e "  [${GREEN}OK${NC}] Volume mount /etc/forge/ssl detected in service file"
    else
        echo -e "  [${RED}FAIL${NC}] Volume mount for /etc/forge/ssl is missing in $path"
        echo -e "         hint: make sure the Docker run command includes: -v /etc/forge/ssl:/etc/forge/ssl:ro"
        ERRORS=$((ERRORS + 1))
    fi
}

# 1. Check directories
echo -e "${BLUE}1. Checking required directories on host:${NC}"
check_dir "/etc/forge" 1
check_dir "/etc/forge/ssl" 1
check_dir "/var/lib/forge/devnet/data" 1
check_dir "/var/lib/forge/mainnet/data" 1
echo

# 2. Check cluster configuration files
echo -e "${BLUE}2. Checking devnet (preview) environment:${NC}"
check_env_file "devnet"
check_service_file "devnet"
echo

echo -e "${BLUE}3. Checking mainnet (production) environment:${NC}"
check_env_file "mainnet"
check_service_file "mainnet"
echo

# Summary
echo -e "${BLUE}=== Verification Summary ===${NC}"
if [[ "$ERRORS" -eq 0 ]]; then
    if [[ "$WARNINGS" -eq 0 ]]; then
        echo -e "${GREEN}PASSED: Host environment is fully configured and ready for deployment.${NC}"
    else
        echo -e "${YELLOW}PASSED WITH WARNINGS: Host is runnable but some optional settings are missing.${NC}"
    fi
    exit 0
else
    echo -e "${RED}FAILED: $ERRORS error(s) found. Fix the highlighted issues before deploying.${NC}"
    exit 1
fi
