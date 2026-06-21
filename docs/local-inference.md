# Local inference (OpenAI-compatible)

Ares talks to any OpenAI-compatible `/v1/chat/completions` endpoint, so you can run
extraction against a **local** model with no code changes — just point `ARES_BASE_URL`
at a local server. This is the cheapest way to validate small-model extraction quality
before reaching for a native/embedded backend.

The extractor already sends `response_format: {"type": "json_schema", ...}` with your
schema, and **every extraction is validated against that schema** before it is saved
(see [output validation](../crates/ares-core/src/schema.rs)). So even servers that
ignore the constraint are caught by validation rather than silently returning garbage.

## 1. Run a local server

Any of these expose an OpenAI-compatible API. Pick one:

### llama.cpp (`llama-server`) — recommended for this recipe

`llama-server` supports `response_format: json_schema` (it converts a subset of JSON
Schema to a GBNF grammar for token-level constrained decoding), which is exactly what
the extractor sends.

```bash
# Download a quantized GGUF once (≈2 GB for a 3B at Q4) and serve it:
llama-server -hf Qwen/Qwen2.5-3B-Instruct-GGUF:Q4_K_M \
  --port 8080 --alias qwen2.5-3b-instruct --ctx-size 8192 --temp 0
# OpenAI-compatible endpoint is now at http://localhost:8080/v1
```

The `model` field Ares sends is matched against `--alias` (or ignored). Set `--temp 0`
for deterministic extraction.

### Ollama

```bash
ollama pull qwen2.5:3b
ollama serve            # OpenAI-compatible endpoint at http://localhost:11434/v1
```

With Ollama the `model` field **must** be the exact tag (`qwen2.5:3b`). Its
OpenAI-compatibility layer supports `response_format: {"type":"json_object"}` but not
full `json_schema`; rely on schema validation for shape enforcement.

### LM Studio

Start the local server from the GUI (default `http://localhost:1234/v1`); the `model`
field is the loaded model's id shown in the app.

## 2. Point Ares at it

```bash
export ARES_PROVIDER=openai           # local servers speak the OpenAI dialect
export ARES_BASE_URL=http://localhost:8080/v1
export ARES_MODEL=qwen2.5-3b-instruct # must match your server's model id / alias
export ARES_API_KEY=sk-local          # most local servers ignore the key, but one is required

cargo run -- scrape -u https://example.com -s blog@latest
```

That's the whole recipe — no rebuild, no feature flags.

## Model picks for a laptop

| Model | Size (Q4) | Notes |
|---|---|---|
| [Qwen2.5-3B-Instruct](https://hf.co/Qwen/Qwen2.5-3B-Instruct-GGUF) | ~2 GB | Strong small default; runs CPU-only on any modern laptop |
| Phi-4-mini (3.8B) | ~3 GB | Good reasoner for 8 GB machines |
| Llama-3.1-8B-Instruct | ~5 GB | Higher quality if you have ≥16 GB RAM or a GPU |

A 3B at Q4 needs ~2 GB of RAM/VRAM and runs on CPU, so it is the safe starting point.
Use `temperature 0` for extraction regardless of model.

## 3. Benchmark local vs hosted

The [`bench`](../bench) harness runs every configured endpoint against saved page
fixtures and reports **validity** (schema conformance), **latency**, and a token/cost
proxy — so you can quantify how a local 3B compares to a hosted model before committing
to it.

```bash
cp bench/targets.example.json bench/targets.json   # edit: enable the endpoints you want
export OPENAI_API_KEY=...                           # keys for hosted targets you enabled
# local-llamacpp target uses api_key_optional, so it runs without a key
cargo run --example bench --features anthropic -- bench/targets.json
```

See [bench/README.md](../bench/README.md) for details.
