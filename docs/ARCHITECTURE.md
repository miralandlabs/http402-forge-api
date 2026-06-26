# http402-forge-api — Architecture

Standalone marketplace API for [http402.trade](https://http402.trade). Creators list digital goods; buyers pay via **pr402** `exact` rail; the API streams files from **Cloudflare R2** (or local disk in dev).

## Components

| Layer | Technology |
|-------|------------|
| HTTP | Rust, Axum 0.8 |
| Database | PostgreSQL (`deadpool-postgres` + `tokio-postgres`) or SQLite (`deadpool-sqlite` + `rusqlite`, dev) |
| Objects | R2 (S3-compatible) or `STORAGE_BACKEND=local` |
| Payments | pr402 facilitator (`verify` + `settle`) |

## Money flow

1. Each listing stores `seller_wallet` and `price_micro_usdc`.
2. `GET /api/v1/listings/{id}/download` returns **HTTP 402** with `accepts[]` where `payTo` is the **creator’s** SplitVault PDA (resolved via pr402 `rails/exact`).
3. Buyer retries with `PAYMENT-SIGNATURE`; API verifies and settles through pr402.
4. On success, API records a `sales` row and streams the asset bytes.

Platform fee (Phase 2): optional SplitVault split via env `PLATFORM_FEE_BPS` — not enabled in Phase 1.

## Listing lifecycle

```text
POST /api/v1/listings (multipart)
  → validate seller_wallet (pr402 provider exists)
  → store preview + asset in R2
  → insert listings row (status=active)

GET /api/v1/listings/{id}/preview
  → public preview object (no payment)

GET /api/v1/listings/{id}/download
  → 402 or paid file stream
```

## Escrow lane (Phase 2)

Listings with `delivery_scheme = escrow` and `byte_size > ESCROW_SIZE_THRESHOLD` use `sla-escrow` accepts (see `routes/listings.rs`). Delivery evidence follows the [x402-buy-spl-token](https://github.com/miralandlabs/x402-buy-spl-token) pattern with file-delivery oracle.

## Environment

| Variable | Required | Description |
|----------|----------|-------------|
| `DATABASE_URL` | no* | `sqlite:./data/forge.db` (devnet default) or `postgres://...` |
| `POSTGRES_POOL_MAX_SIZE` | no | Postgres pool size (default 10) |
| `SQLITE_POOL_MAX_SIZE` | no | SQLite pool size (default 6, mintforge-style) |
| `SELLER_PUBLIC_BASE_URL` | yes | Public API base for 402 `resource.url` |
| `FACILITATOR_BASE_URL` | yes | e.g. `https://preview.ipay.sh` |
| `SOLANA_CLUSTER` | yes | `devnet` or `mainnet` |
| `STORAGE_BACKEND` | no | `r2` (default) or `local` |
| `R2_*` | if r2 | Account, bucket, keys |
| `LOCAL_STORAGE_PATH` | if local | Default `./data/objects` |

## Migrations

SQL files in `migrations/postgres/` and `migrations/sqlite/` run automatically on startup (backend chosen from `DATABASE_URL` prefix).

**Convention (same as pr402 / solrisk):**

- `001_init.sql` — **full** schema for fresh installs.
- `002_*.sql`, … — **delta** migrations for databases created before that change.
- When you add columns or indexes, update `001_init.sql` **and** add (or extend) a numbered delta file so both paths stay correct.
