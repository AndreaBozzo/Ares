# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-02-28


### Added

- Add PostgreSQL support for extraction persistence
- Implement persistent job queue for scrape jobs
- Add JSON Schema support for blog with versioning and registry
- Add headless browser support with Chromium and futures integration
- Add headless browser support and update CLI options for scraping
- Implement per-domain request throttling for polite fetching
- **schemas**: Add github_repo schema, remove duplicate blog.json
- Add configurable timeouts and system prompt for scraping and extraction
- **server**: Add HTTP API layer (ares-server)
- **server**: Integrate OpenAPI documentation with utoipa and update dependencies
- Add Dockerfile and .dockerignore for containerization support
- Update Rust version in Dockerfile to 1.88
- **server**: Implement admin token for write endpoint protection and update OpenAPI spec
- Add scrape endpoint and API for managing extraction schemas.
- Implement configurable rate limiting and request body size limits for the server.
- Implement SSRF protection in `ReqwestFetcher` to block private IP ranges and add an option to disable it for CLI usage.
- Update CI configuration, enhance Cargo.toml metadata, and improve documentation in README; refactor job handling in database layer
- Add update and delete schema endpoints with corresponding tests
- **docs**: Update architecture diagram with detailed flowchart and components


### Changed

- Extract schema resolution and DB management from CLI
- Fmt
- Fmt
- Rename `ares-server` crate to `ares-api` and update related files.
- **docs**: Enhance architecture diagram with clearer component descriptions and relationships


### Documentation

- Added badges
- Added social docs
- Add issue and pull request templates for better contribution guidelines
- Add architecture diagram and related commentary to README
- Update README with additional commentary and clarification
- Format description of Ares with centered alignment
- Reposition description of Ares for improved emphasis
- Update README to include DATABASE_MAX_CONNECTIONS configuration option and added clear TODOs in codebase
- Document REST API, Docker usage, and CI in README, and update project description and module details.
- Update Rust prerequisite version to 1.88+
- Add a note about the Ares Claude Skill to the README.
- Update release information for Ares 0.1.0 in README


### Fixed

- Update logo size and refine environment configuration instructions in README
- Increase logo size in README
- Update advisories to ignore specific Rust security advisory and remove MPL-2.0 from allowed licenses
- Update logo image in assets
- Simplify formatting in browser smoke test and browser fetcher
- Update actions/checkout version to v6 in CI configuration
- Clippy
- **docs**: Correct subgraph syntax in architecture diagram


### Miscellaneous

- Remove empty util module
- Remove unnecessary comment from README and update architecture diagram
- Bump msrv for clippy to 1.88+, add dry run for crates in release workflow


### Testing

- Add integration tests for job cancellation and listing endpoints

