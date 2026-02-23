# ares-cli

Command-line interface for [Ares](https://github.com/AndreaBozzo/Ares), the industrial-grade AI web scraper.

This crate provides a CLI to interact with Ares core features:
- `ares scrape`: Perform a one-shot scrape and extraction
- `ares worker`: Start a background worker to process scrape jobs from the queue
- `ares job`: Create, list, show, or cancel jobs
- `ares history`: View extraction history for specific URLs

## Overview
Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, rate-limiting, and more.

For full documentation, see the [main Ares repository](https://github.com/AndreaBozzo/Ares).
