//! Extraction benchmark: compare extractors (local-HTTP vs hosted) on validity,
//! latency, and a token/cost proxy across saved page fixtures.
//!
//! It cleans each fixture's HTML to Markdown once, then runs every configured
//! target against every fixture, validating the output against the fixture's
//! JSON Schema (the same `validate_extracted_output` the pipeline uses).
//!
//! Usage:
//! ```text
//! cp bench/targets.example.json bench/targets.json   # then edit
//! export OPENAI_API_KEY=...            # keys for the targets you enabled
//! cargo run --example bench --features "anthropic,local-llm" -- bench/targets.json
//! ```
//! Run from the repository root (schemas are resolved from `./schemas`). Targets
//! whose API key env var is unset are skipped (unless `api_key_optional`, for
//! keyless local servers). Anthropic targets require `--features anthropic`.

use std::time::Instant;

use ares_client::{HtmdCleaner, Provider, ProviderExtractor};
use ares_core::traits::{Cleaner, Extractor};
use ares_core::{SchemaResolver, ungrounded_fields, validate_extracted_output};
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    fixtures: Vec<Fixture>,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Fixture {
    /// Path to a saved HTML page (relative to the repo root).
    file: String,
    /// Schema reference (`name@version`, `name@latest`, or a path).
    schema: String,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    provider: String,
    model: String,
    base_url: String,
    /// Environment variable holding this target's API key. Native local targets
    /// have no upstream key and omit this field.
    api_key_env: Option<String>,
    /// If true and the key env is unset, run anyway with a placeholder key
    /// (for local servers that ignore auth). Defaults to false.
    #[serde(default)]
    api_key_optional: bool,
}

enum Status {
    Valid,
    Invalid,
    Error,
    Skipped,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Status::Valid => "valid",
            Status::Invalid => "INVALID",
            Status::Error => "ERROR",
            Status::Skipped => "skipped",
        }
    }
}

struct Row {
    fixture: String,
    target: String,
    status: Status,
    latency_ms: u128,
    input_chars: usize,
    output_chars: usize,
    /// Count of extracted string values that look absent from the source
    /// (hallucination signal) — only meaningful for `valid` rows.
    ungrounded: usize,
    detail: String,
}

/// Rough token estimate (~4 chars/token) used only as a relative cost proxy.
fn approx_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bench/targets.json".to_string());
    let config: Config = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;

    let resolver = SchemaResolver::new("schemas");
    let cleaner = HtmdCleaner::new();

    // Clean + resolve each fixture once (shared across targets).
    struct Prepared {
        name: String,
        schema: serde_json::Value,
        markdown: String,
    }
    let mut prepared = Vec::new();
    for fixture in &config.fixtures {
        let html = std::fs::read_to_string(&fixture.file)
            .map_err(|e| format!("reading fixture {}: {e}", fixture.file))?;
        let markdown = cleaner.clean(&html)?;
        let schema = resolver.resolve(&fixture.schema)?.schema;
        prepared.push(Prepared {
            name: fixture.file.clone(),
            schema,
            markdown,
        });
    }

    let mut rows: Vec<Row> = Vec::new();

    for fx in &prepared {
        for target in &config.targets {
            let provider = match Provider::parse(&target.provider) {
                Ok(p) => p,
                Err(e) => {
                    rows.push(errored(&fx.name, target, e.to_string()));
                    continue;
                }
            };

            let api_key = match target.api_key_env.as_deref() {
                Some(env) => match std::env::var(env) {
                    Ok(k) if !k.is_empty() => k,
                    _ if target.api_key_optional => "sk-local".to_string(),
                    _ => {
                        rows.push(skipped(&fx.name, target, format!("{env} unset")));
                        continue;
                    }
                },
                None if provider == Provider::Local => String::new(),
                None => {
                    rows.push(skipped(&fx.name, target, "api_key_env unset".to_string()));
                    continue;
                }
            };

            let extractor = match ProviderExtractor::build(
                provider,
                &api_key,
                &target.model,
                &target.base_url,
                None,
                None,
            ) {
                Ok(e) => e,
                Err(e) => {
                    rows.push(errored(&fx.name, target, e.to_string()));
                    continue;
                }
            };

            eprintln!("→ {} on {} ...", target.name, fx.name);
            let start = Instant::now();
            let result = extractor.extract(&fx.markdown, &fx.schema).await;
            let latency_ms = start.elapsed().as_millis();

            let row = match result {
                Ok(value) => {
                    let output = value.to_string();
                    let (status, detail, ungrounded) =
                        match validate_extracted_output(&fx.schema, &value) {
                            Ok(()) => {
                                let u = ungrounded_fields(&fx.markdown, &value);
                                let detail = if u.is_empty() {
                                    String::new()
                                } else {
                                    format!("ungrounded: {}", u.join(", "))
                                };
                                (Status::Valid, detail, u.len())
                            }
                            Err(e) => (Status::Invalid, e.to_string(), 0),
                        };
                    Row {
                        fixture: fx.name.clone(),
                        target: target.name.clone(),
                        status,
                        latency_ms,
                        input_chars: fx.markdown.chars().count(),
                        output_chars: output.chars().count(),
                        ungrounded,
                        detail,
                    }
                }
                Err(e) => Row {
                    fixture: fx.name.clone(),
                    target: target.name.clone(),
                    status: Status::Error,
                    latency_ms,
                    input_chars: fx.markdown.chars().count(),
                    output_chars: 0,
                    ungrounded: 0,
                    detail: e.to_string(),
                },
            };
            rows.push(row);
        }
    }

    print_results(&rows);
    print_summary(&config.targets, &rows);
    Ok(())
}

fn skipped(fixture: &str, target: &Target, detail: String) -> Row {
    Row {
        fixture: fixture.to_string(),
        target: target.name.clone(),
        status: Status::Skipped,
        latency_ms: 0,
        input_chars: 0,
        output_chars: 0,
        ungrounded: 0,
        detail,
    }
}

fn errored(fixture: &str, target: &Target, detail: String) -> Row {
    Row {
        fixture: fixture.to_string(),
        target: target.name.clone(),
        status: Status::Error,
        latency_ms: 0,
        input_chars: 0,
        output_chars: 0,
        ungrounded: 0,
        detail,
    }
}

fn print_results(rows: &[Row]) {
    println!("\n## Results\n");
    println!(
        "| fixture | target | status | latency (ms) | in tokens≈ | out tokens≈ | ungrounded | detail |"
    );
    println!("|---|---|---|---:|---:|---:|---:|---|");
    for r in rows {
        let detail = truncate_detail(&r.detail);
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            short_path(&r.fixture),
            r.target,
            r.status.label(),
            r.latency_ms,
            approx_tokens(r.input_chars),
            approx_tokens(r.output_chars),
            r.ungrounded,
            detail,
        );
    }
}

fn print_summary(targets: &[Target], rows: &[Row]) {
    println!("\n## Summary per target\n");
    println!("| target | valid/total | ungrounded (total) | mean latency (ms) | avg out tokens≈ |");
    println!("|---|---:|---:|---:|---:|");
    for target in targets {
        let tr: Vec<&Row> = rows.iter().filter(|r| r.target == target.name).collect();
        let ran: Vec<&&Row> = tr
            .iter()
            .filter(|r| !matches!(r.status, Status::Skipped))
            .collect();
        let valid = ran
            .iter()
            .filter(|r| matches!(r.status, Status::Valid))
            .count();
        let ungrounded_total: usize = ran.iter().map(|r| r.ungrounded).sum();
        let mean_latency = if ran.is_empty() {
            0
        } else {
            ran.iter().map(|r| r.latency_ms).sum::<u128>() / ran.len() as u128
        };
        let avg_out = if ran.is_empty() {
            0
        } else {
            ran.iter()
                .map(|r| approx_tokens(r.output_chars))
                .sum::<usize>()
                / ran.len()
        };
        println!(
            "| {} | {}/{} | {} | {} | {} |",
            target.name,
            valid,
            ran.len(),
            ungrounded_total,
            mean_latency,
            avg_out,
        );
    }
    println!(
        "\n_Token counts are a ~4-chars/token proxy. Local endpoints have no per-token cost; \
         compare hosted token counts against your provider's price sheet._"
    );
}

fn short_path(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Char-safe, single-line truncation for a Markdown table cell.
fn truncate_detail(s: &str) -> String {
    const MAX: usize = 60;
    let oneline = s.replace('\n', " ");
    if oneline.chars().count() <= MAX {
        oneline
    } else {
        let prefix: String = oneline.chars().take(MAX).collect();
        format!("{prefix}…")
    }
}
