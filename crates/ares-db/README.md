# ares-db

PostgreSQL persistence layer for [Ares](https://github.com/AndreaBozzo/Ares), the industrial-grade AI web scraper.

This crate manages database interactions using `sqlx`, providing:
- Schema migrations for Ares tables (extractions, scrape jobs, crawl support)
- Implementations of the `ExtractionStore` trait (`ExtractionRepository`)
- Implementations of the `JobQueue` trait (`ScrapeJobRepository`) with crawl session tracking
- Database connection configuration parsing from environment variables

## Overview
Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, rate-limiting, and more.

For full documentation, see the [main Ares repository](https://github.com/AndreaBozzo/Ares).
