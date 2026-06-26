# Forge Seller Guide

## Costs (what you pay)

| Action | USDC | SOL |
|--------|------|-----|
| **Activate SplitVault** (one-time) | — | Small network fee + rent (wallet transaction) |
| **Upload / publish listing** | — | — (wallet **signMessage** only) |
| **Buyer download** | Paid by buyer via x402 | — |

Publishing does **not** charge USDC. The only seller on-chain cost is optional-but-required **vault activation** (paid in SOL once per wallet).

## 1. Activate your pr402 SplitVault

**Web (recommended):** [http402.trade/sell](https://http402.trade/sell) — connect wallet, complete **Step 1 — Activate payout vault** in the UI (signs a provision transaction in your wallet).

**Manual / ipay.sh:** Open the pr402 seller onboarding flow (preview: `https://preview.ipay.sh`, production: `https://ipay.sh`), connect the wallet that will receive USDC, and activate on-chain.

**API check:**

```http
GET /api/v1/seller/status?seller_wallet={pubkey}
```

Returns `canSell: true` when the wallet has an activated SplitVault on the `exact` rail.

**Provision transaction (for wallets / agents):**

```http
POST /api/v1/seller/provision-tx
Content-Type: application/json

{ "sellerWallet": "{pubkey}", "asset": "USDC" }
```

Sign and submit the returned base64 transaction. Activate **before** first upload — proactive activation keeps your protocol fee at **0.90%** vs **1.00%** for lazy activation on first buyer payment.

## 2. List on Forge

**Human:** Step 2 on the Sell page — upload asset + optional preview, set price, **Publish listing** (signs the seller challenge message).

**Agent:** `POST /api/v1/listings` (`multipart/form-data`) — see [AGENT_API.md](AGENT_API.md).

Required: `seller_wallet`, `seller_challenge`, `seller_signature`, `title`, `category`, `price_usdc`, `asset` file.

Send `seller_wallet` **before** `asset` / `preview` fields so the API can verify vault status before accepting large uploads.

Categories: `art`, `text`, `audio`, `video`, `prompt_pack`.

**Content moderation:** when `MODERATION_PROVIDER=openai`, uploads are scanned before storage. Flagged content returns **400** (`listing blocked by content moderation`). Sellers cannot bypass via a mismatched `content_hash` — the server always stores SHA-256 of the asset bytes.

## 3. Get paid

Buyers hit `GET /api/v1/listings/{id}/download`. Payment settles to **your** SplitVault PDA — Forge is not custodian.

## Local development

Set `SKIP_SELLER_VAULT_CHECK=1` on the API **only** on your machine to skip vault enforcement. Preview and production VPS must use `SKIP_SELLER_VAULT_CHECK=0`.

## Large files (escrow lane)

Listings over `ESCROW_SIZE_THRESHOLD_BYTES` (default 10 MB) or with `delivery_scheme=escrow` use the **sla-escrow** rail. Configure oracle authorities on the API host before enabling escrow listings in production.
