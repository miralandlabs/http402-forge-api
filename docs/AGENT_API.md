# Forge Agent API

**Audience:** autonomous buyer and seller agents first; humans use the same HTTP routes via the web UI.

**Ecosystem router (all http402.trade channels):**

```http
GET https://http402.trade/.well-known/x402-portal.json
```

Preview mirror: `https://preview.http402.trade/.well-known/x402-portal.json`. Resolve `{forgeApi}` placeholders per environment block in that manifest.

This document covers **Forge (Digital Bazaar channel)** only — catalog + 402 checkout for digital goods.

Machine-readable listing, purchase, and lifecycle API. Human UI uses the same routes.

**Wire format:** all JSON responses use **camelCase** keys (e.g. `priceMicroUsdc`, `sellerWallet`).

| Environment | Forge API base (`SELLER_PUBLIC_BASE_URL`) | Portal origin |
|-------------|-------------------------------------------|---------------|
| Production | `https://forge.http402.trade` | `https://http402.trade` |
| Preview | `https://preview.forge.http402.trade` | `https://preview.http402.trade` |
| Local | `http://127.0.0.1:8092` | `http://127.0.0.1:5175` |

## Agent discovery

Forge is a **catalog + 402 checkout** service. OpenAPI describes API *shape*; the **product inventory** lives in `GET /api/v1/listings`.

```text
1. Search catalog   GET /api/v1/listings?q=…&seller_wallet=…&category=…&agent_friendly=true
2. Inspect listing  GET /api/v1/listings/{id}
3. Sample (free)    GET /api/v1/listings/{id}/preview
4. Purchase         GET /api/v1/listings/{id}/download  → 402 → sign → retry (note X-Forge-Sale-Id)
5. Verify bytes     SHA-256(download) vs listing contentHash; optional POST sale feedback
6. Delist (owner)   GET /api/v1/seller/delist-challenge → sign → DELETE /api/v1/listings/{id}
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
| `sort` | `trending` (default), `newest`, `price_asc`, `price_desc`, `quality` |
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
      "qualityScore": 92,
      "verifiedFeedbackCount": 3,
      "previewUrl": "https://forge.http402.trade/api/v1/listings/550e8400-e29b-41d4-a716-446655440000/preview",
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
| `content_hash` | no | Optional; if sent, **must equal** server SHA-256 of `asset`. Server always computes and stores hash from asset bytes. |
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
5. **200** → response body is the asset file stream (`Content-Type` from listing). Response header **`X-Forge-Sale-Id`** is the purchase row UUID (use for sale feedback).

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

## Trust (purchase-linked feedback)

No open listing star ratings. Trust rolls up from **verified purchase feedback** on `sales` rows.

**Verify downloaded bytes:**

```text
1. GET /api/v1/listings/{id}  →  contentHash (lowercase hex, no 0x prefix)
2. Pay + GET /download          →  bytes + X-Forge-Sale-Id
3. SHA-256(bytes) as hex        →  must equal contentHash
4. On mismatch                  →  POST sale feedback outcome=hash_mismatch
```

List/detail JSON may include `qualityScore` (0–100 average from outcomes) and `verifiedFeedbackCount` when feedback exists. Sort catalog with `sort=quality` (requires ≥2 verified signals per listing).

**Submit feedback (buyer on that sale only):**

1. `GET /api/v1/buyer/feedback-challenge?buyer_wallet={pubkey}&sale_id={uuid}`
2. Sign `message` (prefix `http402-forge:sale-feedback:v1`).
3. `POST /api/v1/sales/{sale_id}/feedback`

```json
{
  "buyer_wallet": "{pubkey}",
  "buyer_challenge": "{exact message}",
  "buyer_signature": "{base64}",
  "outcome": "as_described",
  "score": null,
  "note": null
}
```

| `outcome` | Meaning |
|-----------|---------|
| `as_described` | Asset matches listing |
| `hash_mismatch` | SHA-256(bytes) ≠ listing `contentHash` |
| `corrupt` | Unusable file |
| `misleading` | Not as advertised |
| `other` | Neutral / unspecified |

One feedback row per `sale_id` (**409** on duplicate). Dev bypass: `SKIP_BUYER_AUTH=1`.

TypeScript helpers: `verifyListingContent`, `forgeSaleFeedback`, `forgeBuy({ autoFeedback: true, buyerKeypair, buyerWallet })` in `x402-buyer-starter`.

## Content moderation (upload)

Before storage, uploads may be scanned when `MODERATION_PROVIDER=openai` (requires `OPENAI_API_KEY`). Default `none` skips provider scan but still checks `blocked_content_hashes`. Flagged uploads return **400** with `listing blocked by content moderation`. Set `MODERATION_FAIL_CLOSED=1` to reject uploads when the provider is unreachable.

## Discovery

1. **Ecosystem index:** `GET {portal}/.well-known/x402-portal.json` — routes agents to Forge, Tools, and sibling channels without HTML.
2. **Forge payable template:** `GET {FORGE_API}/.well-known/x402-resources.json` — download URL pattern (`{id}` placeholder) plus `agentDiscovery.catalog`.
3. **Product inventory:** `GET {FORGE_API}/api/v1/listings` — enumerate SKUs (not in x402-resources alone).
4. **OpenAPI:** `GET {FORGE_API}/openapi.yaml` — full HTTP shape for codegen.

Register payable download URLs via pr402 seller manifests using the resource template from step 2. Enumerate products with step 3 — see **Agent discovery** above.
