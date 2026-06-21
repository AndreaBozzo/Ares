# Extraction benchmark

Compares extractors (local-HTTP vs hosted) on **validity**, **latency**, and a
token/cost proxy across saved page fixtures. Use it to decide whether a small local
model is good enough before adopting it — the companion to
[docs/local-inference.md](../docs/local-inference.md).

## Layout

- `fixtures/` — saved HTML pages, one per schema (`blog.html`, `github_repo.html`).
- `targets.example.json` — example config; copy to `targets.json` and edit.

## Run

```bash
cp bench/targets.example.json bench/targets.json
# Edit targets.json: keep the endpoints you want, set the api_key_env names.

export OPENAI_API_KEY=...        # only for the hosted targets you enabled
# A target with "api_key_optional": true (e.g. a local llama.cpp server) runs
# without a key. Anthropic targets require building with --features anthropic.

# Run from the repo root (schemas resolve from ./schemas):
cargo run --example bench --features anthropic -- bench/targets.json
```

Each fixture's HTML is cleaned to Markdown once, then every target extracts against it
and the result is validated with the same `validate_extracted_output` the pipeline uses.
Output is a Markdown results table plus a per-target summary (valid/total, mean latency,
avg output tokens≈).

## Config (`targets.json`)

```json
{
  "fixtures": [
    { "file": "bench/fixtures/blog.html", "schema": "blog@latest" }
  ],
  "targets": [
    {
      "name": "local-llamacpp",
      "provider": "openai",
      "model": "qwen2.5-3b-instruct",
      "base_url": "http://localhost:8080/v1",
      "api_key_env": "LOCAL_API_KEY",
      "api_key_optional": true
    }
  ]
}
```

- `provider` — `openai` (OpenAI-compatible, incl. local servers) or `anthropic`.
- `model` — must match the server's model id (for llama.cpp this is `--alias`; for
  Ollama the exact tag, e.g. `qwen2.5:3b`).
- `api_key_env` — env var holding the key; `api_key_optional: true` lets keyless local
  servers run anyway.

Token counts are a ~4-chars/token proxy for relative comparison, not billing-accurate.
Add more fixtures by dropping an HTML file in `fixtures/` and referencing a schema.
