# ares-core

Core business logic and traits for [Ares](https://github.com/AndreaBozzo/Ares), the industrial-grade AI web scraper.

This crate is the heart of Ares, containing:
- Abstract traits for dependencies (`Fetcher`, `Cleaner`, `Extractor`, `JobQueue`, `ExtractionStore`, `LinkDiscoverer`, `RobotsChecker`)
- Implementations of the core services: `ScrapeService` and `WorkerService`
- Circuit Breaker and Throttle primitives
- In-memory caching: `ContentCache` (fetched HTML) and `ExtractionCache` (LLM results)
- `CrawlConfig` for recursive web crawling with depth, page limits, and domain filtering
- `SchemaResolver` with CRUD operations, `name@version` resolution, and validation
- Common domain models and models for scraping instructions and jobs

## Overview
Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, rate-limiting, and more.

For full documentation, see the [main Ares repository](https://github.com/AndreaBozzo/Ares).
