# VPS deploy handover — http402-forge-api (preview / production)

Handover for the next engineer or agent continuing work on **Supabase + Docker + systemd** deployment to `polystrike`.

**Status as of handover:** Preview (devnet) deploy has **never succeeded end-to-end** with Supabase Postgres. GitHub Actions CI runs deploy automatically but fails at health check because the API container exits on startup.

---

## Scope

| Stack | Branch (CI) | Cluster | Env file | API URL |
|-------|-------------|---------|----------|---------|
| Preview | `develop` (any non-`main`) | `devnet` | `/etc/forge/devnet.env` | `https://preview.forge.http402.trade` |
| Production | `main` | `mainnet` | `/etc/forge/mainnet.env` | `https://forge.http402.trade` |

**Repo on VPS:** `/opt/src/http402-forge-api` (override via GitHub secret `FORGE_VPS_REPO_PATH`)

**CI workflow:** `.github/workflows/deploy.yml` — SSH → `git pull` → `sudo bash scripts/docker/forge-deploy.sh --cluster …`

Routine deploys are **push-to-deploy**. Manual SSH is only for one-time bootstrap or break-glass recovery.

---

## Current blocker (CI failure)

App exits immediately with:

```text
Supabase requires DATABASE_SSL_ROOT_CERT in /etc/forge/*.env
(preview: /etc/forge/ssl/supabase-preview-ca.crt,
 production: /etc/forge/ssl/supabase-prod-ca.crt).
Download CA from Supabase Dashboard → Database → SSL Configuration
```

Then `forge-devnet.service` enters a restart loop → `/health` never binds on `:8092` → workflow exits 1.

**Meaning:** For Supabase URLs, the binary requires:

1. `DATABASE_SSL_ROOT_CERT=…` set in the cluster env file, **and**
2. That path must be a readable file **inside the container** (not just on the host).

Validation lives in `src/db/postgres.rs` → `validate_database_url()` / `supabase_ca_path()`.

---

## Failure timeline (what was hit, what was fixed in code)

| # | Symptom | Root cause | Fix in repo | Verified on VPS? |
|---|---------|------------|-------------|------------------|
| 1 | `Network unreachable` to `db.*.supabase.co` | Supabase Direct host often IPv6-only on VPS | Docs: use Session pooler `*.pooler.supabase.com:5432` | Likely yes (URL updated) |
| 2 | `error performing TLS handshake` | Supabase server cert chains to **Supabase CA**, not public WebPKI | Load PEM from `DATABASE_SSL_ROOT_CERT` via `rustls` | **No** — never confirmed green |
| 3 | TLS still failed / wrong cert path | Single `supabase-ca.crt` wrong for two Supabase projects | Per-cluster files: `supabase-preview-ca.crt`, `supabase-prod-ca.crt` | **No** — VPS may still have old name or missing env |
| 4 | CA not loaded for pooler host | Code only matched `supabase.co` | `is_supabase_host()` also matches `supabase.com` | In code only |
| 5 | Docker `AlreadyExists` on `:current` tag | containerd retag race after long build | `forge-deploy.sh`: untag `:current` before promote | Fix in repo; may not be on VPS yet |
| 6 | **Current** — startup validation error | Missing `DATABASE_SSL_ROOT_CERT` and/or CA not mounted into container | Strict validation + systemd ssl volume | **Blocked here** |

---

## Preview vs production databases

**Two separate Supabase projects** (correct design). Shared on VPS:

- Same Forge API binary / Docker pattern
- Same Supabase **platform** CA (usually identical PEM from both dashboards — verify with `diff`)

**Must differ:**

| | Preview | Production |
|--|---------|------------|
| `DATABASE_URL` | Preview project ref, password, pooler host | Prod project ref, password, pooler host |
| CA file | `/etc/forge/ssl/supabase-preview-ca.crt` | `/etc/forge/ssl/supabase-prod-ca.crt` |
| Env var | In `/etc/forge/devnet.env` | In `/etc/forge/mainnet.env` |
| IP allowlist | VPS public IPv4 on **preview** project | VPS public IPv4 on **prod** project |

Example preview env:

```env
DATABASE_URL=postgresql://postgres.[PREVIEW_REF]:[URL_ENCODED_PASS]@aws-1-us-west-1.pooler.supabase.com:5432/postgres?sslmode=require
DATABASE_SSL_ROOT_CERT=/etc/forge/ssl/supabase-preview-ca.crt
```

---

## Host vs container trap (important)

`forge-db-check.sh` runs on the **host**. The API runs in **Docker** via systemd.

If the CA file exists on the host but the systemd unit does not mount `/etc/forge/ssl`, db-check can pass while the container still fails.

Required systemd mount (in `scripts/docker/forge-{devnet,mainnet}.service`):

```text
-v /etc/forge/ssl:/etc/forge/ssl:ro
```

After updating service files in git, run on VPS:

```bash
sudo bash scripts/docker/forge-install.sh
sudo systemctl daemon-reload
```

---

## One-time VPS bootstrap (CI cannot do this)

GitHub secrets (`FORGE_VPS_HOST`, `FORGE_VPS_USER`, `FORGE_VPS_SSH_KEY`) only enable SSH deploy. These remain **manual on the VPS**:

1. Clone repo: `/opt/src/http402-forge-api`
2. `sudo bash scripts/docker/forge-install.sh`
3. Edit env files: `sudo -e /etc/forge/devnet.env`, `sudo -e /etc/forge/mainnet.env`
4. Download Supabase CA(s):
   - Dashboard → **Database** → **SSL Configuration** → **Download certificate**
   - Preview project → `/etc/forge/ssl/supabase-preview-ca.crt`
   - Prod project → `/etc/forge/ssl/supabase-prod-ca.crt`
5. Set `DATABASE_SSL_ROOT_CERT` in each env file (paths above)
6. Supabase → Database → Network → allowlist **VPS public IPv4** on **both** projects
7. URL-encode special characters in `DATABASE_URL` password

---

## Verification checklist (do in order)

### 1. Host config

```bash
sudo grep -E '^DATABASE_URL=|^DATABASE_SSL_ROOT_CERT=' /etc/forge/devnet.env
sudo ls -la /etc/forge/ssl/supabase-preview-ca.crt
grep 'forge/ssl' /etc/systemd/system/forge-devnet.service
```

### 2. Preflight (must pass before deploy)

```bash
cd /opt/src/http402-forge-api
git pull origin develop
sudo bash scripts/docker/forge-db-check.sh --cluster devnet
```

Required output: **`Postgres TLS OK`** — not just `TCP OK`.

### 3. Deploy

Either push to `develop` (CI) or:

```bash
sudo bash scripts/docker/forge-deploy.sh --cluster devnet
```

### 4. Success criteria

- [ ] `systemctl is-active forge-devnet` → `active`
- [ ] `curl -fsS http://127.0.0.1:8092/health | jq .` → `"status":"healthy"`
- [ ] `curl -fsS https://preview.forge.http402.trade/health | jq .` → healthy
- [ ] No TLS / `DATABASE_SSL_ROOT_CERT` errors in `journalctl -t forge-devnet -n 50`
- [ ] GitHub Actions job **Deploy API → VPS** green

---

## Break-glass recovery

### Docker `:current` tag stuck (`AlreadyExists`)

If build succeeded but tag failed:

```bash
SHA=$(git rev-parse --short=12 HEAD)
sudo docker tag forge-devnet:current forge-devnet:previous 2>/dev/null || true
sudo docker rmi forge-devnet:current 2>/dev/null || true
sudo docker tag "forge-devnet:${SHA}" forge-devnet:current
sudo systemctl restart forge-devnet.service
```

Or after pulling fixed `forge-deploy.sh`:

```bash
sudo bash scripts/docker/forge-deploy.sh --cluster devnet --skip-build
```

### Preview-only escape (no Supabase parity)

```env
DATABASE_URL=sqlite:/app/data/forge.db
# remove or comment DATABASE_SSL_ROOT_CERT
```

Use only to unblock the API shell — **does not validate production Postgres path**.

---

## Key files

| Area | Path |
|------|------|
| Postgres + TLS | `src/db/postgres.rs` |
| Deploy script | `scripts/docker/forge-deploy.sh` |
| DB preflight | `scripts/docker/forge-db-check.sh` |
| Systemd units | `scripts/docker/forge-{devnet,mainnet}.service` |
| Env templates | `scripts/docker/forge-{devnet,mainnet}.env.example` |
| Install/bootstrap | `scripts/docker/forge-install.sh` |
| Deploy docs | `scripts/docker/README.md` |
| CI | `.github/workflows/deploy.yml` |

**Sibling references (working patterns):**

- `mintforge/scripts/docker/mintforge-deploy.sh`
- `oracles/scripts/docker/oracle-deploy.sh`

---

## Suggested follow-ups (not implemented)

1. **CI preflight** — SSH `forge-db-check.sh` before build; fail workflow early
2. **Container-aware db-check** — probe with same Docker mounts as systemd (not host-only)
3. **Bootstrap verifier** — script that checks env + CA + systemd ssl mount before first deploy
4. **Stricter CI smoke test** — fail job on health timeout (today only `::warning::`)

---

## Bottom line

CI deploy wiring is correct. Preview never reached healthy because **VPS-side Supabase configuration** (CA files, env vars, systemd ssl mount, allowlist) was never fully applied and **TLS to Supabase was never confirmed** after the rustls + CA changes.

**Next agent:** Fix VPS host config → `forge-db-check.sh` must show **Postgres TLS OK** → redeploy → confirm `/health`. Do not iterate on Rust TLS until preflight passes.
