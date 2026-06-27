# Forge Mainnet Launch Checklist

Use this checklist before pointing production traffic at `https://forge.http402.trade`.

## Cluster & payments

- [ ] Set `SOLANA_CLUSTER=mainnet` in `/etc/forge/production.env`
- [ ] Confirm `FACILITATOR_BASE_URL` points at production pr402 (mainnet USDC mint)
- [ ] Verify `PLATFORM_FEE_BPS` and `PLATFORM_FEE_WALLET` if protocol fee is enabled
- [ ] Smoke-test a full purchase on mainnet (402 → sign → download)

## Database & storage

- [ ] `DATABASE_URL` → Supabase Postgres with `?sslmode=require`
- [ ] `DATABASE_SSL_ROOT_CERT` → project CA PEM (`/etc/forge/ssl/supabase-prod-ca.crt`)
- [ ] Run migrations (001–004: agent metadata, preview content type, trust/moderation) on production DB
- [ ] `STORAGE_BACKEND=r2` with production bucket and credentials
- [ ] Confirm preview streaming works for large video/audio assets (no full-buffer previews)

## Escrow & oracles

- [ ] Set `ORACLE_AUTHORITIES` to comma-separated oracle signer pubkeys (required for `delivery_scheme=escrow`)
- [ ] Document oracle profile: `ORACLE_PROFILE_ID=x402/oracles/file-delivery/attestation/v1`
- [ ] Verify `ESCROW_SIZE_THRESHOLD_BYTES` — assets at or above this size use `sla-escrow` rail automatically
- [ ] Test a large-file listing end-to-end: escrow 402 → payment → oracle attestation → download

## Network & security

- [ ] `SELLER_PUBLIC_BASE_URL=https://forge.http402.trade`
- [ ] `CORS_ALLOWED_ORIGINS` includes `https://http402.trade` and `https://www.http402.trade`
- [ ] TLS termination (nginx/Caddy) with valid certificate; HSTS enabled
- [ ] `RATE_LIMIT_RPS` set (default 30; set `0` to disable in-process limiter and rely on nginx)
- [ ] Optional nginx rate limits on `/api/v1/listings/*/preview` and `/download` per client IP
- [ ] `SKIP_SELLER_AUTH=0`, `SKIP_SELLER_VAULT_CHECK=0`, `SKIP_BUYER_AUTH=0` in production

## Trust & moderation

- [ ] `contentHash` on listings is server-computed; agents verify SHA-256(bytes) after download (see `AGENT_API.md`)
- [ ] Paid downloads return `X-Forge-Sale-Id` — required for `POST /api/v1/sales/{id}/feedback`
- [ ] Optional: enable upload moderation before open uploads scale
  - [ ] `MODERATION_PROVIDER=openai` + `OPENAI_API_KEY` (or `none` to skip provider scan)
  - [ ] `MODERATION_FAIL_CLOSED=1` if you want uploads rejected when OpenAI is unreachable
  - [ ] Populate `blocked_content_hashes` for known-bad asset hashes (manual SQL or admin script)
- [ ] Terms / prohibited-use policy published on the web app (API moderation alone is not legal compliance)

## Monitoring

- [ ] Health check: `GET /health` (200, database + storage reachable)
- [ ] Log shipping / alerting on 5xx rate, facilitator timeouts, R2 errors (stdout via journald; optional `LOG_FILE_DIR` for host files)
- [ ] Monitor SSE `/api/v1/events` connectivity if live ticker is used in UI

## Frontend

- [ ] `VITE_API_BASE_URL=https://forge.http402.trade`
- [ ] `VITE_FACILITATOR_BASE_URL` → production facilitator
- [ ] `npm run build` succeeds; deploy static assets to CDN/Pages

## Post-launch

- [ ] Update `AGENT_API.md` base URL if changed
- [ ] Publish forge-mcp server env vars for agent operators
- [ ] Announce `sort=quality`, `qualityScore`, and sale feedback API to integrators
