# Schemas

This directory stores versioned JSON Schemas used by the CLI.

## Layout

- schemas/registry.json
  - Map of schema name to the latest version string.
- schemas/<name>/<version>.json
  - Versioned schema files.

## Examples

- schemas/blog/1.0.0.json
- schemas/registry.json contains { "blog": "1.0.0" }

## CLI usage

- ares scrape --schema blog@1.0.0
- ares scrape --schema blog@latest
- ares scrape --schema schemas/blog/1.0.0.json
