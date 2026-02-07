use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use ares_client::{HtmdCleaner, OpenAiExtractor, ReqwestFetcher};
use ares_core::compute_hash;
use ares_core::traits::{Cleaner, Extractor, Fetcher};

#[derive(Parser)]
#[command(name = "ares", version, about = "Industrial Grade AI Scraper")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract structured data from a web page
    Scrape {
        /// Target URL to scrape
        #[arg(short, long)]
        url: String,

        /// Path to JSON Schema file defining the extraction schema
        #[arg(short, long)]
        schema: PathBuf,

        /// LLM model to use (e.g., "gpt-4o-mini", "gemini-2.0-flash")
        #[arg(short, long, env = "ARES_MODEL")]
        model: String,

        /// OpenAI-compatible API base URL
        #[arg(
            short,
            long,
            env = "ARES_BASE_URL",
            default_value = "https://api.openai.com/v1"
        )]
        base_url: String,

        /// API key (reads from ARES_API_KEY env var if not provided)
        #[arg(short, long, env = "ARES_API_KEY")]
        api_key: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present
    let _ = dotenvy::dotenv();

    // Setup tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("ares=info".parse()?))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Scrape {
            url,
            schema,
            model,
            base_url,
            api_key,
        } => {
            cmd_scrape(&url, &schema, &model, &base_url, &api_key).await?;
        }
    }

    Ok(())
}

async fn cmd_scrape(
    url: &str,
    schema_path: &PathBuf,
    model: &str,
    base_url: &str,
    api_key: &str,
) -> Result<()> {
    // 1. Load schema from file
    let schema_str = std::fs::read_to_string(schema_path)
        .with_context(|| format!("Failed to read schema file: {}", schema_path.display()))?;
    let schema: serde_json::Value =
        serde_json::from_str(&schema_str).context("Invalid JSON in schema file")?;

    tracing::info!("Fetching {}", url);

    // 2. Fetch HTML
    let fetcher = ReqwestFetcher::new().context("Failed to create HTTP client")?;
    let html = fetcher.fetch(url).await.map_err(|e| anyhow::anyhow!(e))?;

    tracing::info!("Fetched {} bytes of HTML", html.len());

    // 3. Clean HTML → Markdown
    let cleaner = HtmdCleaner::new();
    let markdown = cleaner.clean(&html).map_err(|e| anyhow::anyhow!(e))?;

    tracing::info!(
        "Cleaned to {} bytes of Markdown ({}% reduction)",
        markdown.len(),
        if html.is_empty() {
            0
        } else {
            100 - (markdown.len() * 100 / html.len())
        }
    );

    // 4. Extract structured data via LLM
    let extractor =
        OpenAiExtractor::with_base_url(api_key, model, base_url).map_err(|e| anyhow::anyhow!(e))?;

    tracing::info!("Extracting with model {} ...", model);

    let extracted = extractor
        .extract(&markdown, &schema)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // 5. Compute hashes for future change detection
    let content_hash = compute_hash(&markdown);
    let data_hash = compute_hash(&extracted.to_string());

    tracing::info!(content_hash = %&content_hash[..8], data_hash = %&data_hash[..8], "Extraction complete");

    // 6. Output JSON to stdout
    println!("{}", serde_json::to_string_pretty(&extracted)?);

    Ok(())
}
