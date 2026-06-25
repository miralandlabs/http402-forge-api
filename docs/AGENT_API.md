# Forge Agent API

Machine-readable listing and purchase API. Same routes as the human UI.

**Wire format:** all JSON responses use **camelCase** keys (e.g. `priceMicroUsdc`, `sellerWallet`).

Base URL: `https://api.http402.trade` (or your deployed `SELLER_PUBLIC_BASE_URL`).

## Agent discovery

Forge is a **catalog + 402 checkout** service. OpenAPI describes API *shape*; the **product inventory** lives in `GET /api/v1/listings`.

```text
1. Search catalog   GET /api/v1/listings?q=…&category=…&agent_friendly=true
2. Inspect listing  GET /api/v1/listings/{id}
3. Sample (free)    GET /api/v1/listings/{id}/preview
4. Purchase         GET /api/v1/listings/{id}/download  → 402 → sign → retry
```

**Stable product ID:** listing UUID (`id`). Store that — not OpenAPI paths or slug URLs.

**Payable resource (per listing):**

```http
GET {FORGE_API}/api/v1/listings/{id}/download
```

First request returns **402** with `accepts[]`. Build and sign via pr402, then retry with `PAYMENT-SIGNATURE`.

**Marketplace entry (x402):** `GET /.well-known/x402-resources.json` on this host describes the download URL *pattern* (`{id}` placeholder). It is not a full SKU manifest — use `GET /listings` to enumerate inventory.

**Prompt / agent-oriented assets:** filter with `agent_friendly=true` and `category=prompt_pack`.

## List listings

```http
GET /api/v1/listings?q=cyberpunk&category=art&agent_friendly=true&sort=newest&limit=20&offset=0
```

| Param | Notes |
|-------|--------|
| `q` | Optional. Matches **title** or **description** (case-insensitive, max 80 chars). |
| `category` | `art`, `text`, `audio`, `video`, `prompt_pack` |
| `agent_friendly` | `true` / `false` |
| `sort` | `newest` (default), `price_asc`, `price_desc` |
| `limit` | 1–100 (default 20) |
| `offset` | Pagination offset |

Categories: `art`, `text`, `audio`, `video`, `prompt_pack`.

Response:

```json
{
  "items": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "sellerWallet": "buyA5hR1Z9KtHQRBTmLkjsFfjAabDwdZtrRC6edqxAJ",
      "title": "Cyberpunk prompt pack",
      "description": "10 system prompts for agents",
      "category": "prompt_pack",
      "priceMicroUsdc": 50000,
      "contentType": "application/json",
      "byteSize": 4096,
      "agentFriendly": true,
      "deliveryScheme": "exact",
      "previewUrl": "https://api.http402.trade/api/v1/listings/550e8400-e29b-41d4-a716-446655440000/preview",
      "createdAt": "2026-06-24T12:00:00Z"
    }
  ],
  "total": 1
}
```

## Create listing (agent)

Listing creation requires **wallet ownership proof**:

1. `GET /api/v1/seller/challenge?seller_wallet={pubkey}` → `{ "message", "expiresAt" }`
2. Sign `message` with the wallet's ed25519 key (Solana `signMessage`).
3. `POST /api/v1/listings` with multipart fields including `seller_challenge` (exact message) and `seller_signature` (base64 signature).

```http
POST /api/v1/listings
Content-Type: multipart/form-data
```

| Field | Required | Notes |
|-------|----------|-------|
| `seller_wallet` | yes | Must match signed challenge |
| `seller_challenge` | yes | Exact `message` from challenge endpoint |
| `seller_signature` | yes | Base64 ed25519 signature |
| `title` | yes | max 120 chars |
| `description` | no | max 2000 |
| `category` | yes | `art`, `text`, `audio`, `video`, `prompt_pack` |
| `price_usdc` | yes | UI amount, e.g. `0.05` |
| `agent_friendly` | no | default false |
| `asset` | yes | paid download file |
| `preview` | no | optional; auto JPEG thumbnail for images if omitted |

Dev-only bypass: set `SKIP_SELLER_AUTH=1` on the API (never in production).

## Seller vault (required before listing)

Sellers must have an activated pr402 SplitVault unless `SKIP_SELLER_VAULT_CHECK=1` (local dev only).

```http
GET /api/v1/seller/status?seller_wallet={pubkey}
```

Response (camelCase): `vaultActivated`, `canSell`, `vaultPda`, `feeBps`, `protocolFeePercent`, `sellerDashboardUrl`, `vaultCheckEnforced`.

```http
POST /api/v1/seller/provision-tx
Content-Type: application/json

{ "sellerWallet": "{pubkey}", "asset": "USDC" }
```

Returns a base64 Solana transaction to sign and broadcast. Listing creation returns **403** if the vault is not active.

When posting multipart listings, send `seller_wallet` before `asset` / `preview` so the API can reject inactive sellers before reading large files.

## Purchase (402 flow)

```http
GET /api/v1/listings/{id}/download
```

1. First request → **402** JSON (`x402Version`, `resource`, `accepts`, `extensions`).
2. Build tx: `POST {facilitator}/build-exact-payment-tx` with `accepted` line from `accepts[]`.
3. Sign transaction locally.
4. Retry with header `PAYMENT-SIGNATURE: {base64 proof}`.
5. **200** → response body is the asset file stream (`Content-Type` from listing).

Idempotency: same payment signature returns the file without double-charging (checked via `payments` table).

## Preview

```http
GET /api/v1/listings/{id}/preview
```

Returns preview bytes (image/jpeg thumbnail, uploaded preview, or text snippet).

## Leaderboards

```http
GET /api/v1/leaderboards
```

```json
{
  "top_earners_24h": [{"wallet": "...", "amount_micro_usdc": 150000, "sales_count": 3}],
  "top_payers_24h": [{"wallet": "...", "amount_micro_usdc": 80000, "sales_count": 5}],
  "hottest_listings_24h": [{"listing_id": "...", "title": "...", "sales_count": 12}]
}
```

## Live sales (SSE)

```http
GET /api/v1/events
Accept: text/event-stream
```

Events: `sale` with JSON payload `{listing_id, seller_wallet, buyer_wallet, amount_micro_usdc}`.

## Discovery

Register payable download URLs via `/.well-known/x402-resources.json` on this service (URL template only). Enumerate products with `GET /api/v1/listings` — see **Agent discovery** above.

## OpenAPI

Full machine-readable spec: [openapi.yaml](./openapi.yaml).
