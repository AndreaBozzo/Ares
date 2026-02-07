use std::path::PathBuf;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use ares_client::{HtmdCleaner, OpenAiExtractor, ReqwestFetcher};
use ares_core::compute_hash;
use ares_core::models::NewExtraction;
use ares_core::traits::{Cleaner, Extractor, Fetcher};
use ares_db::ExtractionRepository;

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

        /// LLM model to use (e.g., "gpt-4o-mini", "gemini-2.5-flash")
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

        /// Save extraction to database (requires DATABASE_URL)
        #[arg(long, default_value_t = false)]
        save: bool,

        /// Schema name for storage/retrieval (defaults to filename without extension)
        #[arg(long)]
        schema_name: Option<String>,
    },

    /// Show extraction history for a URL
    History {
        /// Target URL
        #[arg(short, long)]
        url: String,

        /// Schema name to filter by
        #[arg(short, long)]
        schema_name: String,

        /// Number of results to show
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
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
            save,
            schema_name,
        } => {
            let repo = if save {
                Some(connect_db().await?)
            } else {
                None
            };
            let schema_name = schema_name.unwrap_or_else(|| derive_schema_name(&schema));
            cmd_scrape(
                &url,
                &schema,
                &schema_name,
                &model,
                &base_url,
                &api_key,
                repo.as_ref(),
            )
            .await?;
        }
        Commands::History {
            url,
            schema_name,
            limit,
        } => {
            let repo = connect_db().await?;
            cmd_history(&url, &schema_name, limit, &repo).await?;
        }
    }

    Ok(())
}

/// Connect to PostgreSQL using DATABASE_URL.
async fn connect_db() -> Result<ExtractionRepository> {
    let database_url = std::env::var("DATABASE_URL")
        .context("DATABASE_URL not set. Required for --save or history command.")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("Failed to connect to database")?;

    let repo = ExtractionRepository::new(pool);
    repo.migrate().await.map_err(|e| anyhow::anyhow!(e))?;

    Ok(repo)
}

/// Derive schema name from file path (e.g., "schema_case.json" -> "schema_case")
fn derive_schema_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s: &std::ffi::OsStr| s.to_str())
        .unwrap_or("default")
        .to_string()
}

async fn cmd_scrape(
    url: &str,
    schema_path: &PathBuf,
    schema_name: &str,
    model: &str,
    base_url: &str,
    api_key: &str,
    repo: Option<&ExtractionRepository>,
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

    // 3. Clean HTML -> Markdown
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

    // 5. Compute hashes
    let content_hash = compute_hash(&markdown);
    let data_hash = compute_hash(&extracted.to_string());

    tracing::info!(
        content_hash = %&content_hash[..8],
        data_hash = %&data_hash[..8],
        "Extraction complete"
    );

    // 6. Save to DB if requested
    if let Some(repo) = repo {
        // Check if data changed compared to previous extraction
        let previous = repo
            .get_latest(url, schema_name)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        let changed = match &previous {
            Some(prev) => prev.data_hash != data_hash,
            None => true,
        };

        let new_extraction = NewExtraction {
            url: url.to_string(),
            schema_name: schema_name.to_string(),
            extracted_data: extracted.clone(),
            raw_content_hash: content_hash,
            data_hash,
            model: model.to_string(),
        };

        let id = repo
            .save(&new_extraction)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        if changed {
            if previous.is_some() {
                tracing::info!(%id, "Data CHANGED — saved new extraction");
            } else {
                tracing::info!(%id, "First extraction — saved");
            }
        } else {
            tracing::info!(%id, "Data unchanged — saved snapshot");
        }
    }

    // 7. Output JSON to stdout
    println!("{}", serde_json::to_string_pretty(&extracted)?);

    Ok(())
}

async fn cmd_history(
    url: &str,
    schema_name: &str,
    limit: usize,
    repo: &ExtractionRepository,
) -> Result<()> {
    let history = repo
        .get_history(url, schema_name, limit)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    if history.is_empty() {
        println!(
            "No extractions found for url={} schema={}",
            url, schema_name
        );
        return Ok(());
    }

    println!(
        "Extraction history for {} (schema: {}):\n",
        url, schema_name
    );

    for (i, extraction) in history.iter().enumerate() {
        let changed = if i + 1 < history.len() {
            extraction.data_hash != history[i + 1].data_hash
        } else {
            true // First extraction is always "new"
        };

        let status = if changed { "CHANGED" } else { "unchanged" };

        println!(
            "  [{}] {} — {} (model: {}, hash: {}...)",
            status,
            extraction.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            extraction.id,
            extraction.model,
            &extraction.data_hash[..8],
        );
    }

    println!("\nTotal: {} extractions", history.len());

    Ok(())
}
