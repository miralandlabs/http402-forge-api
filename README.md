# http402-forge-api

Backend for [http402.trade](https://http402.trade) — Hashspace digital goods marketplace with pr402 pay-to-download.

## Quick start (local)

```bash
cp .env.example .env
# Devnet defaults to embedded SQLite at ./data/forge.db (no Postgres required)
# Local dev without vault: SKIP_SELLER_VAULT_CHECK=1 cargo run
cargo run
```

Migrations run automatically on startup. With `STORAGE_BACKEND=local`, objects land in `./data/objects`.

## Production / preview (Docker on VPS)

See **[scripts/docker/README.md](scripts/docker/README.md)** for dual-cluster deployment:

| | Web (Vercel) | API (Docker) |
|--|--|--|
| Preview | `preview.http402.trade` | `preview.forge.http402.trade` |
| Production | `http402.trade` | `forge.http402.trade` |

CI/CD: `.github/workflows/http402-forge-build-and-deploy.yml` at the monorepo root.

### Preview generation (upload time)

When no separate preview file is uploaded, the API generates:

| Asset type | Auto preview |
|------------|----------------|
| Image | JPEG thumbnail (400px) |
| Text / JSON | Text snippet |
| PDF | First page as JPEG (requires **poppler**, **mupdf**, or **ghostscript**) |
| Video / audio | First **30s** clip (requires **ffmpeg**; override with `PREVIEW_MEDIA_SECONDS`) |

Optional env: `FFMPEG_BIN`, `PDFTOPPM_BIN`, `GS_BIN`, `MUTOOL_BIN`.

### Ubuntu Linux (production)

Install preview tools on the host (or in your container image) before starting the API:

```bash
sudo apt update
sudo apt install -y ffmpeg poppler-utils
```

| Package | Provides | Used for |
|---------|----------|----------|
| `ffmpeg` | `/usr/bin/ffmpeg` | 30s video/audio preview clips |
| `poppler-utils` | `/usr/bin/pdftoppm` | PDF first-page → JPEG |

Optional fallbacks if poppler is unavailable:

```bash
sudo apt install -y ghostscript          # /usr/bin/gs
sudo apt install -y mupdf-tools          # /usr/bin/mutool
```

Verify on the server:

```bash
ffmpeg -version
pdftoppm -v
```

The API looks up `ffmpeg` and `pdftoppm` on **`PATH`** by default. If binaries live elsewhere, set `FFMPEG_BIN` / `PDFTOPPM_BIN` in your service env (systemd, Docker, etc.).

**Docker:** add the same packages in your image, e.g. `RUN apt-get update && apt-get install -y ffmpeg poppler-utils && rm -rf /var/lib/apt/lists/*`.

**Note:** Ubuntu’s `ffmpeg` package includes H.264/AAC encoders used when stream-copy clipping fails. Listings uploaded before preview generation was added still need to be re-published to get clipped previews.

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/AGENT_API.md](docs/AGENT_API.md)
- [docs/SELLER_GUIDE.md](docs/SELLER_GUIDE.md)

## Devnet E2E

```bash
./scripts/test-devnet.sh
```

See `scripts/env.example` for payer keypair and facilitator overrides.
