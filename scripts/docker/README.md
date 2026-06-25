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
| `forge-deploy.sh` | Build, tag `:current`, restart, health probe, rollback |
| `forge-nginx-setup.sh` | TLS reverse proxy for both API hostnames |
| `forge-{devnet,mainnet}.service` | systemd units |
| `forge-{devnet,mainnet}.env.example` | Templates → `/etc/forge/*.env` |

## Database

Production mainnet should use **Supabase Postgres** (`DATABASE_URL=postgresql://...?sslmode=require`).

Preview devnet can use Supabase or embedded SQLite under `/var/lib/forge/devnet/data`.

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
