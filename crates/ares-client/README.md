# ares-client

External adapters for [Ares](https://github.com/AndreaBozzo/Ares), the industrial-grade AI web scraper.

This crate contains the implementations of the traits defined in `ares-core`, including:
- HTTP Fetcher using `reqwest`
- Headless Browser Fetcher using `chromiumoxide` (feature-gated)
- HTML cleaner to Markdown using `htmd`
- LLM Extractor client (OpenAI-compatible)

## Overview
Ares fetches web pages, converts HTML to Markdown, and uses LLM APIs to extract structured data defined by JSON Schemas. It supports persistent job queues with retries, circuit breaking, rate-limiting, and more.

For full documentation, see the [main Ares repository](https://github.com/AndreaBozzo/Ares).
