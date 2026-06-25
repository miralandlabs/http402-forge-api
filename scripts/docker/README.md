# Docker deployment — http402-forge-api

Dual-cluster layout (mirrors **mintforge** / **oracles**): one Docker image per cluster, systemd on a single Ubuntu VPS, nginx TLS in front.

## URL map (preview.* pattern)

| Stack | Web (Vercel) | API (this VPS) | Solana | Facilitator |
|-------|----------------|----------------|--------|-------------|
| **Preview** | `https://preview.http402.trade` | `https://preview.forge.http402.trade` | devnet | `preview.ipay.sh` |
| **Production** | `https://http402.trade` | `https://forge.http402.trade` | mainnet | `ipay.sh` |

## Fresh VPS (once)

```bash
git clone https://github.com/miralandlabs/http402-forge-api.git /opt/src/http402-forge-api
cd /opt/src/http402-forge-api
sudo bash scripts/docker/forge-install.sh
sudo -e /etc/forge/devnet.env
sudo -e /etc/forge/mainnet.env
sudo bash scripts/docker/forge-deploy.sh --cluster devnet
sudo bash scripts/docker/forge-deploy.sh --cluster mainnet
sudo bash scripts/docker/forge-nginx-setup.sh \
  --certbot-email ops@http402.trade
```

Point DNS A records at the VPS for `preview.forge.http402.trade` and `forge.http402.trade`.

Verify:

```bash
curl -fsS http://127.0.0.1:8092/health | jq .
curl -fsS https://preview.forge.http402.trade/health | jq .
```

## CI/CD (GitHub Actions)

Each app has its own repo and workflow:

| Repo | Workflow | Target |
|------|----------|--------|
| **http402-forge-web** | `.github/workflows/deploy.yml` | Vercel (`preview.http402.trade` / `http402.trade`) |
| **http402-forge-api** | `.github/workflows/deploy.yml` | SSH → `forge-deploy.sh` on VPS |

### GitHub secrets — web repo

`VERCEL_TOKEN`, `ORG_ID`, `PROJECT_ID`

Optional RPC (Helius recommended): `VITE_RPC_URL_PRODUCTION`, `VITE_RPC_URL_PREVIEW`

### GitHub secrets — API repo

`FORGE_VPS_HOST`, `FORGE_VPS_USER`, `FORGE_VPS_SSH_KEY`

Optional: `FORGE_VPS_REPO_PATH` (default `/opt/src/http402-forge-api`)

### Vercel dashboard (web repo)

- Production domain: `http402.trade`
- Preview domain: `preview.http402.trade`
- Build env is injected by CI; optional Vercel env vars only needed for manual dashboard deploys

## Files

| File | Role |
|------|------|
| `Dockerfile` | Rust build + runtime (`ffmpeg`, `poppler-utils`) |
| `forge-install.sh` | Host bootstrap (Docker, dirs, systemd) |
| `forge-db-check.sh` | Pre-deploy DATABASE_URL / TCP probe |
| `forge-deploy.sh` | Build, tag `:current`, restart, health probe, rollback |
| `forge-nginx-setup.sh` | TLS reverse proxy for both API hostnames |
| `forge-{devnet,mainnet}.service` | systemd units |
| `forge-{devnet,mainnet}.env.example` | Templates → `/etc/forge/*.env` |

## Database

The Docker image does **not** ship `psql` or `sqlite3` CLI tools. Both backends are compiled into the Rust binary:

| Backend | Driver | When chosen |
|---------|--------|-------------|
| **SQLite** | `rusqlite` (bundled) | `DATABASE_URL=sqlite:/app/data/forge.db` |
| **PostgreSQL** | `tokio-postgres` + TLS (`native-tls`, `ca-certificates` in image) | any other `DATABASE_URL` prefix |

Migrations run automatically on container start (`migrations/postgres/` or `migrations/sqlite/`).

### Supabase on the VPS

Use the **Direct connection** string (port **5432**, `db.<project>.supabase.co`) with TLS:

```env
DATABASE_URL=postgresql://postgres.[PROJECT_REF]:[PASSWORD]@db.[PROJECT_REF].supabase.co:5432/postgres?sslmode=require
```

**Required:** `?sslmode=require` — without it the app uses plain TCP and Supabase rejects the connection (`postgres conn: error connecting to server`).

**Checklist if deploy health fails on Postgres:**

1. **Network allowlist** — Supabase dashboard → Database → Network → add your VPS **public IPv4**
2. **IPv4** — Direct host may be IPv6-only; if `nc -zv db….supabase.co 5432` fails, use **Session pooler** (port 5432 on `*.pooler.supabase.com`) or enable Supabase IPv4 add-on
3. **Password** — URL-encode special characters in the connection string
4. **Probe before deploy:**
   ```bash
   sudo bash scripts/docker/forge-db-check.sh --cluster devnet
   ```

Preview devnet can stay on SQLite (default in `forge-devnet.env.example`) — no Supabase required until you opt in.

Production mainnet should use Supabase Postgres (same `sslmode=require` rule).

## Seller vault gate

Preview and production stacks require sellers to activate a pr402 SplitVault before listing (`SKIP_SELLER_VAULT_CHECK=0` in `forge-{devnet,mainnet}.env.example`).

| Env | When |
|-----|------|
| `SKIP_SELLER_VAULT_CHECK=0` | Preview VPS, production (default) |
| `SKIP_SELLER_VAULT_CHECK=1` | **Local dev only** — skip on-chain vault check |

The Sell UI calls `GET /api/v1/seller/status` and `POST /api/v1/seller/provision-tx`. Listing uploads are signature-only (no USDC); vault activation costs a small amount of **SOL** once per wallet.

See [docs/SELLER_GUIDE.md](../docs/SELLER_GUIDE.md).

## Rollback

```bash
sudo bash http402-forge-api/scripts/docker/forge-deploy.sh --cluster mainnet --rollback
```
