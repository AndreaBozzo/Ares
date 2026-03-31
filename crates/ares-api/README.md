# ares-api

REST API for [Ares](https://github.com/AndreaBozzo/Ares), the industrial-grade AI web scraper.

This crate provides an Axum-based HTTP server that exposes endpoints for:
- One-shot scraping, extraction, and persistence
- Background job queuing and management (with retry support)
- Crawl session management (start, status, results)
- Fetching extraction history
- Managing JSON Schemas (CRUD with versioning)

It includes auto-generated OpenAPI documentation available via Swagger UI.

## Overview
Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, rate-limiting, and more.

For full documentation, see the [main Ares repository](https://github.com/AndreaBozzo/Ares).
