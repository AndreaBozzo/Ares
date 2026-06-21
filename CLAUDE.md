# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Ares is

A web scraper that fetches pages, converts HTML → Markdown, and uses an OpenAI-compatible LLM to extract structured data defined by JSON Schemas. Ships a CLI (`ares`) and a REST API (`ares-api`), backed by a Postgres job queue with retries, circuit breaking, per-domain throttling, change detection, and recursive crawling.

## Commands

```bash
# Build / run
cargo build
cargo run -- scrape -u https://example.com -s schemas/blog/1.0.0.json   # CLI binary is `ares`
cargo run --bin ares-api                                                # REST server

# Tests
cargo test                       # everything
cargo test --lib --bins          # unit tests only (no Docker needed)        == make test-unit
cargo test --test '*'            # integration tests (needs Postgres)        == make test-integration
cargo test --doc                 # doc tests (CI runs these separately)
cargo test -p ares-core --lib scrape::tests::happy_path_without_store        # a single test by path

# Lint / format (must pass for CI)
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings

# Postgres + migrations (integration tests + persistence)
make docker-up                   # docker compose up -d
make migrate                     # applies crates/ares-db/migrations/*.sql, tracked in schema_migrations
make all                         # fmt + clippy + test

# Local-vs-hosted extraction benchmark (validity / latency / cost proxy)
cargo run --example bench --features anthropic -- bench/targets.json   # see docs/local-inference.md
```

**CI gotchas** (`.github/workflows/ci.yml`):
- Clippy runs with `--all-features`, which compiles the `browser` feature → pulls in `chromiumoxide` (heavy). Any new optional dependency added behind a feature will be built on every CI run unless the feature is excluded from `--all-features` (e.g. moved to a separate crate/workflow).
- Integration tests (`cargo test --test '*'`) need `DATABASE_URL` pointing at Postgres. Unit tests do not — they run fully on mocks.
- MSRV is **Rust 1.88, edition 2024**. `cargo fmt --check`, clippy `-D warnings`, and `cargo-deny` are all gating.

## Architecture

Five workspace crates with a strict dependency direction. The key idea: **`ares-core` defines traits and orchestration but contains zero I/O**; concrete adapters live in `ares-client` and `ares-db`, and are injected.

```
ares-cli ─┐
ares-api ─┼─► ares-core (traits + ScrapeService/WorkerService) ◄─ ares-client (adapters)
          └─► ares-db (Postgres adapters) ──────────────────────► ares-core (implements traits)
```

- **`ares-core`** — `ScrapeService`, `WorkerService`, `CircuitBreaker`, `ThrottledFetcher`, caches, `SchemaResolver`, `AppError`, and the traits everything is generic over: `Fetcher`, `Cleaner`, `Extractor`, `ExtractorFactory`, `ExtractionStore`, `JobQueue`, `LinkDiscoverer`, `RobotsChecker`. Has no HTTP/DB/LLM dependencies. Mock implementations of every trait live in `testutil.rs` (cfg(test)), which is why core logic is unit-testable without Docker or network.
- **`ares-client`** — adapter impls: `ReqwestFetcher` (static HTML), `BrowserFetcher` (Chromium, feature `browser`), `HtmdCleaner`, `OpenAiExtractor` + `OpenAiExtractorFactory`, `HtmlLinkDiscoverer`, `CachedRobotsChecker`.
- **`ares-db`** — `ExtractionRepository` (impls `ExtractionStore`) and `ScrapeJobRepository` (impls `JobQueue`) over Postgres via `sqlx`; migrations in `migrations/`.
- **`ares-cli` / `ares-api`** — thin wiring layers. They construct the concrete adapters and hand them to `ScrapeService`/`WorkerService`. Note: **`ares-api` does NOT run a worker** — the worker is a separate process (`ares worker`); the API only enqueues jobs and serves reads.

### The scrape pipeline (`ScrapeService::scrape`, ares-core/src/scrape.rs)

`fetch → clean → (hash content) → extract → validate → hash data → compare with previous → persist`

- `ScrapeService<F, C, E, S>` is generic over the four trait deps and built fresh per request/job. Optional `ContentCache`/`ExtractionCache` (moka, in-memory) short-circuit fetch and extraction by hash.
- **Output validation**: after extraction, the result is validated against the JSON Schema via `validate_extracted_output` (ares-core/src/schema.rs). On mismatch it returns `AppError::ExtractionValidationError` and nothing is persisted. This runs for **all** entrypoints (CLI, API, worker, crawl) because they all funnel through `ScrapeService`. Toggle with `.with_validation(false)` (default on). Distinguish from `SchemaValidationError`, which means the LLM output wasn't even parseable JSON.
- Change detection: data is SHA-256 hashed; `--skip-unchanged` avoids re-saving identical extractions.

### Worker & crawl (ares-core/src/worker.rs)

`WorkerService` polls the queue, and for each job uses `ExtractorFactory::create(model, base_url)` to build a per-job extractor (each job can target a different model/endpoint), then runs a `ScrapeService`. Failures route through `AppError::is_retryable()` (exponential backoff retry) and `AppError::should_trip_circuit()` (CircuitBreaker). **When adding error variants, set those two classifications deliberately** — they drive whether a job retries and whether the LLM circuit opens. Crawl jobs additionally run `LinkDiscoverer` and enqueue child jobs up to `CrawlConfig` depth/page/domain/robots limits.

### Schemas (ares-core/src/schema.rs)

JSON Schema files live in `schemas/<name>/<version>.json` with a `registry.json` mapping name → latest version. `SchemaResolver` accepts a direct path, `name@version`, or `name@latest`. Schema CRUD writes to the **filesystem** (not the DB) and keeps `registry.json` in sync, including semver-aware "latest" recomputation on delete. `validate_schema` checks a document is itself a valid JSON Schema (meta-validation); `validate_extracted_output` checks a value conforms to a schema (used in the pipeline).

## Conventions

- Errors: single `AppError` enum in `ares-core/src/error.rs`, surfaced over HTTP by `ares-api/src/error.rs` mapping variants → status codes. Add new variants there in both places.
- The `Extractor` trait is the seam for new inference backends (the `OpenAiExtractor` already supports any OpenAI-compatible `base_url`, including local servers and Gemini's compat endpoint). New backends implement `Extractor` + `ExtractorFactory`; nothing else in the pipeline needs to change. `AnthropicExtractor` (native Messages API via forced tool use) is gated behind the `anthropic` feature in `ares-client`. Runtime provider selection goes through `ProviderExtractor`/`ProviderExtractorFactory` dispatch enums (ares-client/src/provider.rs), chosen via `--provider`/`ARES_PROVIDER` in the CLI and the `/v1/scrape` request body in the API.
- All trait deps are `Clone + Send + Sync` so services can be cheaply reconstructed per job.
