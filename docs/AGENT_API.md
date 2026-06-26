# Forge Agent API

Machine-readable listing, purchase, and lifecycle API for autonomous agents. Human UI uses the same routes.

**Wire format:** all JSON responses use **camelCase** keys (e.g. `priceMicroUsdc`, `sellerWallet`).

Base URL: `https://api.http402.trade` (or your deployed `SELLER_PUBLIC_BASE_URL`).

## Agent discovery

Forge is a **catalog + 402 checkout** service. OpenAPI describes API *shape*; the **product inventory** lives in `GET /api/v1/listings`.

```text
1. Search catalog   GET /api/v1/listings?q=…&seller_wallet=…&category=…&agent_friendly=true
2. Inspect listing  GET /api/v1/listings/{id}
3. Sample (free)    GET /api/v1/listings/{id}/preview
4. Purchase         GET /api/v1/listings/{id}/download  → 402 → sign → retry
5. Delist (owner)   GET /api/v1/seller/delist-challenge → sign → DELETE /api/v1/listings/{id}
```

**Stable product ID:** listing UUID (`id`). Store that — not OpenAPI paths or slug URLs.

**Payable resource (per listing):**

```http
GET {FORGE_API}/api/v1/listings/{id}/download
```

First request returns **402** with `accepts[]`. Build and sign via pr402, then retry with `PAYMENT-SIGNATURE`.

**Marketplace entry (x402):** `GET /.well-known/x402-resources.json` on this host describes the download URL *pattern* (`{id}` placeholder). It is not a full SKU manifest — use `GET /listings` to enumerate inventory.

**Prompt / agent-oriented assets:** filter with `agent_friendly=true` and `category=prompt_pack`.

**Listing lifecycle:** only `status = active` listings appear in catalog, detail, and preview. Owners soft-delist with seller-signed `DELETE` (`status = removed`). R2 objects are retained; buyers who already paid may re-download with the same `PAYMENT-SIGNATURE` idempotency key.

## List listings

```http
GET /api/v1/listings?q=cyberpunk&seller_wallet=AbC…&category=art&agent_friendly=true&sort=newest&limit=20&offset=0
```

| Param | Notes |
|-------|--------|
| `q` | Optional. Matches **title** or **description** (case-insensitive, max 80 chars). |
| `seller_wallet` | Optional. Exact match on seller pubkey (base58). Combine with `q` to search within one seller's catalog. |
| `category` | `art`, `text`, `audio`, `video`, `prompt_pack` |
| `agent_friendly` | `true` / `false` |
| `sort` | `trending` (default), `newest`, `price_asc`, `price_desc` |
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
      "previewContentType": "text/plain; charset=utf-8",
      "byteSize": 4096,
      "agentFriendly": true,
      "deliveryScheme": "exact",
      "tags": ["prompt", "agent"],
      "license": "personal",
      "contentHash": "a1b2c3…",
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
| `tags` | no | Comma-separated or JSON array (agent-oriented listings) |
| `license` | no | `personal` or `commercial` |
| `content_hash` | no | SHA-256 hex of asset; computed automatically if omitted |
| `asset` | yes | paid download file |
| `preview` | no | optional teaser file (any MIME); PDF uploads are rasterized to JPEG for thumbnails; auto-generated if omitted (see below) |

**Preview vs asset:** `contentType` describes the paid **asset**. `previewContentType` describes the free **preview** object (often different — e.g. asset `application/pdf`, preview `image/jpeg`). Clients must render previews from `previewContentType`, not `contentType`.

| Asset (no custom preview) | Auto preview |
|-----------------------------|--------------|
| Image | JPEG thumbnail |
| Text / JSON | Text snippet (~500 chars) |
| PDF | First page → JPEG (or text placeholder if raster fails) |
| Audio / video | ~30s clip |
| Other | Text placeholder |

Dev-only bypass: set `SKIP_SELLER_AUTH=1` on the API (never in production).

## Delist listing (agent)

Soft-remove a listing from the public catalog. Same **wallet ownership proof** as create — there is no separate agent session. An agent that listed under wallet `W` must sign as `W`; it cannot delist another seller's listing.

1. `GET /api/v1/seller/delist-challenge?seller_wallet={pubkey}&listing_id={uuid}` → `{ "message", "expiresAt" }`
   - Returns **403** if `seller_wallet` is not the listing owner.
   - Returns **404** if the listing is not active (already delisted or unknown id).
2. Sign `message` with the wallet's ed25519 key (Solana `signMessage`). The challenge binds wallet + listing id (`http402-forge:delist-listing:v1` prefix).
3. `DELETE /api/v1/listings/{id}` with JSON body:

```http
DELETE /api/v1/listings/{id}
Content-Type: application/json

{
  "seller_wallet": "{pubkey}",
  "seller_challenge": "{exact message from step 1}",
  "seller_signature": "{base64 ed25519 signature}"
}
```

| Field | Required | Notes |
|-------|----------|-------|
| `seller_wallet` | yes | Must match listing owner and signed challenge |
| `seller_challenge` | yes | Exact `message` from delist-challenge (includes `listing:{uuid}`) |
| `seller_signature` | yes | Base64 ed25519 signature |

**Success:** `204 No Content`. Listing `status` becomes `removed`.

**Effects:**

| Route | After delist |
|-------|----------------|
| `GET /listings`, `GET /listings/{id}`, `GET …/preview` | **404** (hidden from catalog) |
| `GET …/download` without prior payment | **404** (no new purchases) |
| `GET …/download` with prior `PAYMENT-SIGNATURE` (idempotency hit) | **200** — file stream; buyers keep access |

Storage objects (R2/local) are **not** deleted immediately.

Dev-only bypass: `SKIP_SELLER_AUTH=1` skips signature verification but still requires `seller_wallet` to match the listing owner in the database update.

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

1. First request → **402** JSON (`x402Version`, `resource`, `accepts`, `extensions`) for **active** listings.
2. Build tx: `POST {facilitator}/build-exact-payment-tx` with `accepted` line from `accepts[]`.
3. Sign transaction locally.
4. Retry with header `PAYMENT-SIGNATURE: {base64 proof}`.
5. **200** → response body is the asset file stream (`Content-Type` from listing).

**Removed listings:** no **402** — new buyers get **404**. Agents with a stored payment proof retry step 4 with the same `PAYMENT-SIGNATURE`; idempotency returns **200** without charging again.

Idempotency: same payment signature returns the file without double-charging (checked via `payments` table). Works for both active and removed listings.

## Preview

```http
GET /api/v1/listings/{id}/preview
```

Returns a **text snippet** (buffered, max ~500 chars) for `text/*` and `application/json` previews.

For **image, video, audio, and PDF** previews, the response is **streamed**. Use `previewContentType` from the listing JSON to choose a renderer (`<img>`, `<video>`, `<audio>`, or `<iframe>` for PDF). The `previewUrl` can be used directly as a media `src`. Response includes `Accept-Ranges: bytes` for seekable media.

Uploaded PDF previews are normally stored as `image/jpeg` (first-page raster). If rasterization fails, the preview may remain `application/pdf`.

Legacy listings that stored a text placeholder for video/audio fall back to streaming the full asset clip.

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
