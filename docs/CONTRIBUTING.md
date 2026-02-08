# Contributing to Ares

Thanks for your interest in contributing to Ares!

## Quick Start

- Fork the repo and create your branch from `master`.
- Keep changes focused and small where possible.
- Make sure tests and linting pass before opening a PR.

## Development Setup

```bash
cargo build
cargo test
cargo clippy --all-targets
```

## Code Style

- Run `cargo fmt` before committing.
- Keep public APIs documented.
- Avoid introducing new dependencies unless they are clearly justified.

## Commit & PR Guidelines

- Use clear commit messages.
- Explain the problem and the solution in the PR description.
- Link related issues (e.g., "Closes #123").
- Add tests when behavior changes or new functionality is added.

## Reporting Bugs

- Search existing issues first.
- Include steps to reproduce, expected behavior, and actual behavior.
- Attach logs or minimal examples if possible.

## Feature Requests

- Open an issue with context, use cases, and constraints.
- If possible, propose an implementation plan.

## License

By contributing, you agree that your contributions will be licensed under the
Apache-2.0 license.
