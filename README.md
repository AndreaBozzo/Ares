<p align="center">
  <img src="docs/assets/logoares.png" alt="Ares" width="800">
</p>

<h1 align="center">Ares</h1>

<p align="center">
  Web scraper with LLM-powered structured data extraction.
</p>

---

Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, change detection, and graceful worker shutdown.

Conceptual sibling of [Ceres](https://github.com/AndreaBozzo/Ceres) — same philosophy, different temperament. Where Ceres is the nurturing goddess of harvest, Ares charges headfirst into the web and *takes* what it needs.

## Architecture

```
ares-cli          CLI interface — arg parsing, wiring, delegation
ares-core         Business logic — ScrapeService, WorkerService, CircuitBreaker, traits
ares-client       External adapters — HTTP fetcher, HTML cleaner, OpenAI-compatible LLM client
ares-db           PostgreSQL persistence — ExtractionRepository, ScrapeJobRepository
```

All external dependencies are behind traits (`Fetcher`, `Cleaner`, `Extractor`, `ExtractionStore`, `ExtractorFactory`, `JobQueue`), enabling full mock-based testing.

## Prerequisites

- **Rust** 1.87+ (edition 2024)
- **Docker** (for PostgreSQL and integration tests)
- An **OpenAI-compatible API key** (OpenAI, Gemini, or any compatible endpoint)

## Quick Start

```bash
# Clone and build
git clone <repo-url> && cd Ares
cargo build

# Start PostgreSQL
docker compose up -d

# Set environment variables
export ARES_API_KEY="your-api-key"
export ARES_MODEL="gpt-4o-mini"
export DATABASE_URL="postgresql://postgres:postgres@localhost:5432/ares"

# One-shot scrape (stdout only)
cargo run -- scrape -u https://example.com -s schemas/blog/1.0.0.json

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

### `ares history`

Show extraction history for a URL + schema pair, with change detection.

### `ares job create|list|show|cancel`

Manage persistent scrape jobs in the PostgreSQL queue.

### `ares worker`

Start a background worker that polls the job queue, processes scrape jobs through the circuit breaker, handles retries with exponential backoff, and supports graceful shutdown via Ctrl+C.

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

**Gemini** works via the OpenAI-compatible endpoint:

```bash
export ARES_BASE_URL="https://generativelanguage.googleapis.com/v1beta/openai"
export ARES_MODEL="gemini-2.5-flash"
```

## Development

```bash
# Format, lint, and test
make all

# Run unit tests only
make test-unit

# Run integration tests (requires Docker)
make test-integration

# Start/stop PostgreSQL
make docker-up
make docker-down
```

## License

[Apache-2.0](LICENSE)
