# Forge escrow: payment door vs verdict door

**Status:** Design (not implemented). Does not ship the escrow lane by itself.  
**Date:** 2026-08-16  
**Applies to:** `http402-forge-api` (storage + both doors), `http402-forge-web`, `http402-forge-cli`, `oracle-file-delivery` (fetch adapter only).  
**Does not redefine:** sla-escrow on-chain ABI, pr402 fund/verify/settle, or Layer 0 oracle envelopes.

---

## 1. What this fixes

Two rejected models:

1. **Oracle registry as the shop.** Seller uploads to the oracle; buyer downloads from the oracle. That bypasses Forge storage and turns the oracle into a free CDN.
2. **Oracle uses the buyer download URL without paying.** If `/download` is the paid goods channel, an unpaid GET there is theft, not verification.

Required model: **one object on Forge, two authorizations.**

| Door | HTTP | Who | Pays USDC? | Effect |
| --- | --- | --- | --- | --- |
| **Payment** | `GET /api/v1/listings/{id}/download` | Buyer | Yes (x402 `exact` or sla-escrow **after release**) | One sale; stream the asset |
| **Verdict** | `GET /api/v1/oracle/listings/{id}/artifact` | The **payment’s** `oracle_authority` only | No | Stream the **same** object; **not** a sale |

All listed oracle operators use the **same** verdict path on the Forge API host. They do not each host blobs. They differ only by **which Solana key** Forge will accept for **which payment**.

---

## 2. Invariants

1. Forge R2/local is the **only** object store for listings. Oracles do not offer buyer download.
2. `SHA256(bytes served on the payment door) == SHA256(bytes served on the verdict door) == listing.content_hash == on-chain delivery_hash` (when the seller submitted honestly).
3. The verdict door **MUST NOT** be the payment door (no `PAYMENT-SIGNATURE`, no `sales` row).
4. The payment door **MUST NOT** accept oracle signatures as payment.
5. A caller may use the verdict door only after Forge can prove: funded sla-escrow payment **P** is bound to listing **L**, and the request is signed by `P.oracle_authority`.
6. Exact-rail listings never open the verdict door.
7. Object bytes for a listing are **immutable** after publish (storage key = content hash). Overwrite is forbidden.
8. Independent oracles still **re-hash the stream**. They MUST NOT trust `content_hash` from JSON alone.

---

## 3. Sequence (escrow listing)

Goods already exist at publish (Bazaar), same as today.

```
Seller: POST /listings  →  object stored at key=sha256(bytes), content_hash recorded
        delivery_scheme=escrow (only if size ≥ threshold and ORACLE_AUTHORITIES set)

Buyer:  unpaid GET /download  →  HTTP 402, scheme sla-escrow
        FundPayment (pr402)   →  payment_uid, oracle_authority pinned
        Forge persists bind: payment_uid ↔ listing_id ↔ content_hash ↔ oracle_authority

Seller: SubmitDelivery(delivery_hash = listing.content_hash)

Oracle: sees DeliverySubmitted
        GET /api/v1/oracle/listings/{id}/artifact
            + payment_uid + timestamp + Ed25519(oracle key)
        Forge: verify bind + signature + escrow + not exact
        stream object(key=content_hash)
        oracle SHA-256(stream) == delivery_hash?
          yes + SLA size/MIME → ConfirmOracle approve
          no  → ConfirmOracle reject

Keeper/parties: ReleasePayment | RefundPayment

Buyer:  GET /download
        Forge: escrow released (or exact paid) → stream same object
```

Exact-rail listings skip oracle, bind, and verdict door. Unchanged 402 → settle → stream.

---

## 4. Binding (the logic that makes “same door, many oracles” safe)

Forge MUST store, at successful escrow **fund** (not at listing publish):

| Field | Source |
| --- | --- |
| `listing_id` | The 402 `resource` |
| `payment_uid` | FundPayment / facilitator |
| `content_hash` | Listing row (immutable) |
| `oracle_authority` | On-chain `Payment.oracle_authority` (must be in listing/accepts `oracleProfiles` and in `ORACLE_AUTHORITIES`) |

Verdict request:

```
GET /api/v1/oracle/listings/{listing_id}/artifact
X-Forge-Payment-Uid: <hex-64>
X-Forge-Oracle-Ts: <unix-seconds>
X-Forge-Oracle-Sig: <base58 Ed25519>
```

Message to sign (UTF-8, no JSON canonicalization games):

```
forge-oracle-v1|{listing_id}|{payment_uid_hex}|{ts}|{host}
```

`host` is the Forge API public host (`forge.http402.trade` / preview). Replay window: **±60s**. One successful stream per `(payment_uid, listing_id)` is enough; further GETs may be allowed for oracle retry but MUST be rate-limited and audited.

Forge accepts iff **all** hold:

1. Listing exists, `delivery_scheme` is escrow (not `exact`).
2. Bind row exists for `(listing_id, payment_uid)`.
3. `ts` in window.
4. Signature verifies as bind `oracle_authority`.
5. On-chain (or last indexed) payment still has that `oracle_authority`, matching `listing_id` resource, and is not refunded/closed.
6. Object key `content_hash` exists.

Otherwise **403**. No body bytes.

A key that is a valid oracle for payment **B** cannot fetch listing **A**.

---

## 5. Why this is not “oracle downloads for free”

USDC is the **buyer’s** price for the **goods channel**.

The verdict channel is a **capability** Forge grants to the **named judge of that escrow**, the same way the API process already reads R2 with bucket credentials and does not pay 402 to itself.

The oracle is not a second customer. It cannot call `/download`. The buyer cannot call `/oracle/.../artifact`.

If no privileged verdict door exists, a third-party oracle **cannot** independently hash Forge bytes. Then you only have upload-time `content_hash` (merchant self-report). That is exact-rail trust, not sla-escrow.

---

## 6. Immutability (replacement attack)

If the seller can replace R2 bytes after `content_hash` is published:

- Oracle might hash file 1;
- Buyer might later receive file 2.

**Rule:** storage key = `sha256` hex of the bytes at publish. `PUT` to an existing key is rejected. Download and verdict both open **that** key. `content_hash` on the listing never changes.

---

## 7. Oracle binary (adapter only)

`oracle-file-delivery` keeps: stream hash, size, MIME, `ConfirmOracle`, Active Guardian.

**Change:** evidence fetch URL is Forge’s verdict door, not `POST/GET /v1/registry/blob` as the shop.

- SLA still commits size/MIME/`profile_id`/`payment_uid`.
- `delivery_hash` still = SHA-256 of **raw bytes**.
- Fetcher config: `FORGE_API_BASE`, oracle keypair (already the ConfirmOracle key).

Oracle operator **storage** (Postgres ledger, optional SLA JSON) is bookkeeping. It is **not** the marketplace blob store and **not** a buyer URL.

v1 `POST /v1/registry/blob` MUST NOT be advertised as Forge download. New Forge listings MUST NOT instruct sellers to upload the asset there.

---

## 8. Product surfaces

| Repo | Change |
| --- | --- |
| **forge-api** | Escrow publish (lift 100 MiB reject); fund bind row; verdict route; immutable object keys; unlock `/download` only after release (escrow) or exact settle (as today) |
| **forge-web** | Escrow 402 / wait-for-release / then download; do not send buyers to oracle hosts |
| **forge-cli** | Same buy/publish on sla-escrow; `forge buy` polls release then payment-door GET |
| **oracles** | Forge verdict fetcher for file-delivery; do not use registry blob as CDN |

Exact-rail UI and `forge buy` for small files stay as they are.

---

## 9. Threat notes

| Attempt | Outcome |
| --- | --- |
| Buyer hits verdict door | 403 (no oracle key) |
| Oracle hits `/download` | 402 (not a paid buyer); must not be used |
| Oracle of payment B fetches listing A | 403 (bind mismatch) |
| Stale signature | 403 (ts window) |
| Seller SubmitDelivery with wrong hash | Oracle hash ≠ `delivery_hash` → reject → refund; buyer never unlocked |
| Seller withholds upload | Listing with no object cannot be escrow-published; or 404 → guardian reject |
| Public `GET /v1/registry/{hash}` as download | Out of scope / forbidden for this channel |

Oracle **can** see the file for payments where it is the named authority. That is the job. It is not a global free library.

---

## 10. Out of scope

- Commission / “work not yet done” (no file at publish).
- Semantic file quality (the oracle does not watch the video).
- Confidentiality against the designated oracle (the judge sees bytes).
- Making Conduit an `oracle_authority`.
- Auto-merge of this design by Conduit.

---

## 11. Implementation order

1. API: immutable keys + verdict route + bind row (preview cluster).
2. `oracle-file-delivery` Forge fetcher + one funded escrow on preview.
3. Unlock `/download` after release; web then CLI.
4. Lift size reject; production `ORACLE_AUTHORITIES` + listed operator pubkey.

Do not enable production escrow until (2) has a recorded approve and a recorded reject (wrong hash) on preview.
