<p align="center">
  <img src="docs/assets/logoares.png" alt="Ares" width="800">
</p>

<h1 align="center">Ares</h1>

<p align="center">
  Web scraper with LLM-powered structured data extraction.
</p>

<p align="center">
  <a href="https://github.com/AndreaBozzo/Ares/actions/workflows/ci.yml"><img src="https://github.com/AndreaBozzo/Ares/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://discord.gg/fztdKSPXSz"><img src="https://img.shields.io/discord/1469399961987711161?color=5865F2&logo=discord&logoColor=white&label=Discord" alt="Discord"></a>
</p>

---

Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It exposes both a CLI and a REST API, supports persistent job queues with retries, circuit breaking, rate-limiting, change detection, and graceful shutdown.

> *Named after the Greek god of war and courage.*

Conceptual sibling of [Ceres](https://github.com/AndreaBozzo/Ceres) — same philosophy, different temperament. Where Ceres is the nurturing goddess of harvest, Ares charges headfirst into the web and *takes* what it needs.

## Architecture

![Architecture diagram](docs/assets/aresarchitecture.png)

```
ares-cli          CLI interface — arg parsing, wiring, delegation
ares-server       REST API — Axum HTTP server, OpenAPI/Swagger UI, Bearer auth
ares-core         Business logic — ScrapeService, WorkerService, CircuitBreaker, Throttle, traits
ares-client       External adapters — HTTP fetcher, headless browser fetcher, HTML cleaner, LLM client
ares-db           PostgreSQL persistence — ExtractionRepository, ScrapeJobRepository, migrations
```

All external dependencies are behind traits (`Fetcher`, `Cleaner`, `Extractor`, `ExtractionStore`, `ExtractorFactory`, `JobQueue`), enabling full mock-based testing. The `Fetcher` trait has two implementations: `ReqwestFetcher` for static pages and `BrowserFetcher` (feature-gated behind `browser`) for JS-rendered SPAs.

## Prerequisites

- **Rust** 1.87+ (edition 2024)
- **Docker** (for PostgreSQL and integration tests)
- An **OpenAI-compatible API key** (OpenAI, Gemini, or any compatible endpoint)
- **Chromium / Chrome** (only when using `--browser` for JS-rendered pages)

## Quick Start

```bash
# Clone and build
git clone <repo-url> && cd Ares
cargo build

# Start PostgreSQL
docker compose up -d

# Configure environment
cp .env.example .env
# Edit .env with your API key and settings

# One-shot scrape (stdout only)
cargo run -- scrape -u https://example.com -s schemas/blog/1.0.0.json

# Scrape a JS-rendered page with headless browser
cargo run --features browser -- scrape -u https://spa-example.com -s blog@latest --browser

# Scrape and persist to database
cargo run -- scrape -u https://example.com -s blog@latest --save

# View extraction history
cargo run -- history -u https://example.com -s blog

# Create a background job
cargo run -- job create -u https://example.com -s blog@latest

# Start a worker to process jobs
cargo run -- worker
```

## CLI Commands

### `ares scrape`

One-shot extraction. Fetches the URL, cleans HTML to Markdown, sends it to the LLM with the JSON Schema, and prints the extracted data to stdout.

| Flag | Env Var | Description |
|---|---|---|
| `-u, --url` | | Target URL |
| `-s, --schema` | | Schema path or `name@version` |
| `-m, --model` | `ARES_MODEL` | LLM model (e.g., `gpt-4o-mini`) |
| `-b, --base-url` | `ARES_BASE_URL` | API base URL (default: OpenAI) |
| `-a, --api-key` | `ARES_API_KEY` | API key |
| `--save` | | Persist result to database |
| `--schema-name` | | Override schema name for storage |
| `--browser` | | Use headless browser for JS-rendered pages (requires `browser` feature) |

### `ares history`

Show extraction history for a URL + schema pair, with change detection.

### `ares job create|list|show|cancel`

Manage persistent scrape jobs in the PostgreSQL queue.

### `ares worker`

Start a background worker that polls the job queue, processes scrape jobs through the circuit breaker, handles retries with exponential backoff, and supports graceful shutdown via Ctrl+C.

| Flag | Env Var | Description |
|---|---|---|
| `--worker-id` | | Custom worker ID (auto-generated if omitted) |
| `--poll-interval` | | Seconds between job queue polls (default: 5) |
| `-a, --api-key` | `ARES_API_KEY` | API key |
| `--browser` | | Use headless browser for JS-rendered pages (requires `browser` feature) |

## REST API

Ares ships a standalone HTTP server (`ares-server`) built on [Axum](https://github.com/tokio-rs/axum) with auto-generated [OpenAPI](https://swagger.io/specification/) documentation.

### Running the server

```bash
# Run locally
cargo run --bin ares-server

# Or with Docker
docker build -t ares-server:latest .
docker run -p 3000:3000 --env-file .env ares-server:latest
```

Once running, interactive API docs are available at **`/swagger-ui`**.

### Endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/v1/jobs` | Bearer | Create a scrape job |
| `GET` | `/v1/jobs` | Bearer | List jobs (filter by status, limit) |
| `GET` | `/v1/jobs/{id}` | Bearer | Get job details |
| `DELETE` | `/v1/jobs/{id}` | Bearer | Cancel a pending job |
| `GET` | `/v1/extractions` | Bearer | Query extraction history |
| `GET` | `/health` | — | Health check (database connectivity) |

### Authentication

Protected endpoints require a `Bearer` token set via `ARES_ADMIN_TOKEN`. Token comparison uses constant-time equality (`subtle` crate) to prevent timing attacks.

```bash
curl -H "Authorization: Bearer $ARES_ADMIN_TOKEN" http://localhost:3000/v1/jobs
```

If `ARES_ADMIN_TOKEN` is not set, all protected endpoints return `403 Forbidden`.

## Schemas

Schemas are versioned JSON Schema files stored in `schemas/`:

```
schemas/
  registry.json           # {"blog": "1.0.0"}
  blog/
    1.0.0.json            # JSON Schema definition
```

Reference by path (`schemas/blog/1.0.0.json`) or by name (`blog@1.0.0`, `blog@latest`).

## Configuration

| Variable | Required | Default | Description |
|---|---|---|---|
| `ARES_API_KEY` | Yes | | LLM API key |
| `ARES_MODEL` | Yes | | LLM model name |
| `ARES_BASE_URL` | No | `https://api.openai.com/v1` | OpenAI-compatible endpoint |
| `DATABASE_URL` | For persistence | | PostgreSQL connection string |
| `DATABASE_MAX_CONNECTIONS` | No | `5` | PostgreSQL connection pool size |
| `ARES_ADMIN_TOKEN` | No | | Bearer token for REST API auth |
| `ARES_SERVER_PORT` | No | `3000` | HTTP server listen port |
| `ARES_CORS_ORIGIN` | No | | Allowed CORS origins (comma-separated, or `*`) |
| `CHROME_BIN` | No | Auto-detected | Override path to Chrome/Chromium binary |

**Gemini** works via the OpenAI-compatible endpoint:

```bash
export ARES_BASE_URL="https://generativelanguage.googleapis.com/v1beta/openai"
export ARES_MODEL="gemini-2.5-flash"
```

## Docker

```bash
# Build the image
docker build -t ares-server:latest .

# Run with docker compose (PostgreSQL + pgAdmin + server)
docker compose up -d
```

The Dockerfile uses a multi-stage build (Rust builder → Debian slim runtime) with Chromium pre-installed for browser-based scraping. The release binary is compiled with LTO and symbol stripping for minimal image size.

## Development

```bash
# Format, lint, and test
make all

# Run unit tests only
make test-unit

# Run integration tests (requires Docker)
make test-integration

# Run database migrations
make migrate

# Start/stop PostgreSQL
make docker-up
make docker-down
```

CI runs on every push and PR via GitHub Actions: formatting, Clippy, unit tests, integration tests (with a Postgres service container), and a `cargo-deny` security audit.

## License

[Apache-2.0](LICENSE)
