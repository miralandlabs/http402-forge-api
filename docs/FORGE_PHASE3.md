# Forge Phase 3 (deferred)

Full escrow lane, platform fees, buyer purchase history, and seller analytics are **intentionally deferred** until the oracle attestation path ships end-to-end.

## Escrow lane (sla-escrow)

- Oracle authorities configured on `http402-forge-api`
- Web wallet flow for `sla-escrow` **or** CLI-only path documented
- Remove upload rejection above `escrow_size_threshold` when oracle settlement is live

## Platform fee

- `platform_fee_bps` / `platform_fee_wallet` enforced in facilitator settle path
- Seller dashboard surfaces effective fee tier

## Buyer purchase history

- `GET /api/v1/buyer/purchases?buyer_wallet=…` — wallet-indexed sales list (paginated; web **My purchases** at `/forge/purchases`)
- ~~Web “My purchases” page~~ **shipped**

## Seller analytics

- Volume, conversion, feedback rollup per seller wallet
- Optional SSE “listing sold” toast when connected wallet matches seller

## Current state (Phase 0–2)

- Uploads above escrow threshold: **HTTP 400** with clear message
- `delivery_scheme` pinned to `exact` on publish
- Atomic `record_payment_and_sale` in gate path
- Extended `/health`: database + storage head + facilitator `/supported`

Track progress in [MAINNET_LAUNCH.md](./MAINNET_LAUNCH.md).
