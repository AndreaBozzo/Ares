use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use ares_client::{HtmdCleaner, OpenAiExtractor, OpenAiExtractorFactory, ReqwestFetcher};
use ares_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use ares_core::job::{CreateScrapeJobRequest, JobStatus, WorkerConfig};
use ares_core::job_queue::JobQueue;
use ares_core::worker::{TracingWorkerReporter, WorkerService};
use ares_core::{NullStore, ScrapeService};
use ares_db::{ExtractionRepository, ScrapeJobRepository};

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

        /// JSON Schema path or name@version (e.g., schemas/blog/1.0.0.json or blog@1.0.0)
        #[arg(short, long)]
        schema: String,

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

    /// Manage scrape jobs
    Job {
        #[command(subcommand)]
        action: JobCommands,
    },

    /// Start a worker to process scrape jobs
    Worker {
        /// Worker ID (auto-generated if not provided)
        #[arg(long)]
        worker_id: Option<String>,

        /// Poll interval in seconds
        #[arg(long, default_value_t = 5)]
        poll_interval: u64,

        /// API key for LLM calls
        #[arg(short, long, env = "ARES_API_KEY")]
        api_key: String,
    },
}

#[derive(Subcommand)]
enum JobCommands {
    /// Create a new scrape job
    Create {
        /// Target URL to scrape
        #[arg(short, long)]
        url: String,

        /// JSON Schema path or name@version (e.g., schemas/blog/1.0.0.json or blog@1.0.0)
        #[arg(short, long)]
        schema: String,

        /// LLM model to use
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

        /// Schema name (defaults to filename without extension)
        #[arg(long)]
        schema_name: Option<String>,
    },

    /// List scrape jobs
    List {
        /// Filter by status (pending, running, completed, failed, cancelled)
        #[arg(short, long)]
        status: Option<String>,

        /// Number of results
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },

    /// Show details of a specific job
    Show {
        /// Job ID
        #[arg(value_name = "JOB_ID")]
        id: Uuid,
    },

    /// Cancel a pending or running job
    Cancel {
        /// Job ID
        #[arg(value_name = "JOB_ID")]
        id: Uuid,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

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
            let (schema_path, resolved_name) = resolve_schema(&schema)?;
            let schema_name = schema_name.unwrap_or(resolved_name);

            let schema_str = std::fs::read_to_string(&schema_path).with_context(|| {
                format!("Failed to read schema file: {}", schema_path.display())
            })?;
            let schema_value: serde_json::Value =
                serde_json::from_str(&schema_str).context("Invalid JSON in schema file")?;

            let fetcher = ReqwestFetcher::new().context("Failed to create HTTP client")?;
            let cleaner = HtmdCleaner::new();
            let extractor = OpenAiExtractor::with_base_url(&api_key, &model, &base_url)
                .map_err(|e| anyhow::anyhow!(e))?;

            let result = if save {
                let repo = connect_db().await?;
                let service = ScrapeService::with_store(fetcher, cleaner, extractor, repo, model);
                service
                    .scrape(&url, &schema_value, &schema_name)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))?
            } else {
                let service =
                    ScrapeService::with_store(fetcher, cleaner, extractor, NullStore, model);
                service
                    .scrape(&url, &schema_value, &schema_name)
                    .await
                    .map_err(|e| anyhow::anyhow!(e))?
            };

            println!("{}", serde_json::to_string_pretty(&result.extracted_data)?);
        }

        Commands::History {
            url,
            schema_name,
            limit,
        } => {
            let repo = connect_db().await?;
            cmd_history(&url, &schema_name, limit, &repo).await?;
        }

        Commands::Job { action } => {
            let pool = connect_pool().await?;
            let job_repo = ScrapeJobRepository::new(pool);

            match action {
                JobCommands::Create {
                    url,
                    schema,
                    model,
                    base_url,
                    schema_name,
                } => {
                    let (schema_path, resolved_name) = resolve_schema(&schema)?;
                    let schema_str = std::fs::read_to_string(&schema_path).with_context(|| {
                        format!("Failed to read schema file: {}", schema_path.display())
                    })?;
                    let schema_value: serde_json::Value =
                        serde_json::from_str(&schema_str).context("Invalid JSON in schema file")?;
                    let schema_name = schema_name.unwrap_or(resolved_name);

                    let request = CreateScrapeJobRequest::new(
                        url,
                        schema_name,
                        schema_value,
                        model,
                        base_url,
                    );
                    let job = job_repo
                        .create_job(request)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("Created job: {}", job.id);
                }

                JobCommands::List { status, limit } => {
                    let status_filter = status
                        .map(|s| {
                            s.parse::<JobStatus>()
                                .map_err(|e| anyhow::anyhow!("Invalid status: {}", e))
                        })
                        .transpose()?;

                    let jobs = job_repo
                        .list_jobs(status_filter, limit)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;

                    if jobs.is_empty() {
                        println!("No jobs found.");
                        return Ok(());
                    }

                    println!(
                        "{:<38} {:<12} {:<40} {:<20} {:<16}",
                        "ID", "STATUS", "URL", "MODEL", "CREATED"
                    );
                    println!("{}", "-".repeat(120));

                    for job in &jobs {
                        let url_display = if job.url.len() > 38 {
                            format!("{}...", &job.url[..35])
                        } else {
                            job.url.clone()
                        };
                        println!(
                            "{:<38} {:<12} {:<40} {:<20} {}",
                            job.id,
                            job.status,
                            url_display,
                            job.model,
                            job.created_at.format("%Y-%m-%d %H:%M"),
                        );
                    }

                    println!("\nTotal: {} jobs", jobs.len());
                }

                JobCommands::Show { id } => {
                    let job = job_repo
                        .get_job(id)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                        .ok_or_else(|| anyhow::anyhow!("Job not found: {}", id))?;

                    println!("Job: {}", job.id);
                    println!("  Status:      {}", job.status);
                    println!("  URL:         {}", job.url);
                    println!("  Schema:      {}", job.schema_name);
                    println!("  Model:       {}", job.model);
                    println!("  Base URL:    {}", job.base_url);
                    println!("  Created:     {}", job.created_at);
                    println!("  Updated:     {}", job.updated_at);
                    if let Some(started) = job.started_at {
                        println!("  Started:     {}", started);
                    }
                    if let Some(completed) = job.completed_at {
                        println!("  Completed:   {}", completed);
                    }
                    println!("  Retries:     {}/{}", job.retry_count, job.max_retries);
                    if let Some(next) = job.next_retry_at {
                        println!("  Next retry:  {}", next);
                    }
                    if let Some(err) = &job.error_message {
                        println!("  Error:       {}", err);
                    }
                    if let Some(eid) = job.extraction_id {
                        println!("  Extraction:  {}", eid);
                    }
                    if let Some(wid) = &job.worker_id {
                        println!("  Worker:      {}", wid);
                    }
                }

                JobCommands::Cancel { id } => {
                    job_repo
                        .cancel_job(id)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?;
                    println!("Cancelled job: {}", id);
                }
            }
        }

        Commands::Worker {
            worker_id,
            poll_interval,
            api_key,
        } => {
            let pool = connect_pool().await?;
            let job_repo = ScrapeJobRepository::new(pool.clone());
            let extraction_repo = ExtractionRepository::new(pool);

            let config = WorkerConfig::default()
                .with_poll_interval(std::time::Duration::from_secs(poll_interval));
            let config = if let Some(id) = worker_id {
                config.with_worker_id(id)
            } else {
                config
            };

            let fetcher = ReqwestFetcher::new().context("Failed to create HTTP client")?;
            let cleaner = HtmdCleaner::new();
            let extractor_factory = OpenAiExtractorFactory::new(&api_key);
            let cb = CircuitBreaker::new("llm", CircuitBreakerConfig::default());

            let worker = WorkerService::new(
                job_repo,
                fetcher,
                cleaner,
                extractor_factory,
                extraction_repo,
                cb,
                config,
            );

            let cancel = CancellationToken::new();
            let token = cancel.clone();

            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                tracing::info!("Shutdown signal received");
                token.cancel();
            });

            worker
                .run(cancel, &TracingWorkerReporter)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
        }
    }

    Ok(())
}

fn resolve_schema(schema_arg: &str) -> Result<(PathBuf, String)> {
    let path_candidate = PathBuf::from(schema_arg);
    if path_candidate.exists() {
        let name = schema_name_from_path(&path_candidate)
            .unwrap_or_else(|| ares_core::derive_schema_name(&path_candidate));
        return Ok((path_candidate, name));
    }

    let (name, version) = schema_arg
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("Schema not found: {}", schema_arg))?;
    if name.is_empty() || version.is_empty() {
        return Err(anyhow::anyhow!(
            "Schema must be in the form name@version, got: {}",
            schema_arg
        ));
    }

    let resolved_version = if version == "latest" {
        let registry = load_schema_registry()?;
        registry
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No latest version for schema {}", name))?
    } else {
        version.to_string()
    };

    let schema_path = Path::new("schemas")
        .join(name)
        .join(format!("{}.json", resolved_version));
    if !schema_path.exists() {
        return Err(anyhow::anyhow!(
            "Schema file not found: {}",
            schema_path.display()
        ));
    }

    Ok((schema_path, format!("{}@{}", name, resolved_version)))
}

fn load_schema_registry() -> Result<HashMap<String, String>> {
    let registry_path = Path::new("schemas").join("registry.json");
    let registry_str = std::fs::read_to_string(&registry_path).with_context(|| {
        format!("Failed to read schema registry: {}", registry_path.display())
    })?;
    let registry: HashMap<String, String> =
        serde_json::from_str(&registry_str).context("Invalid JSON in schema registry")?;
    Ok(registry)
}

fn schema_name_from_path(path: &Path) -> Option<String> {
    let components: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let schemas_index = components.iter().position(|c| c == "schemas")?;
    let name = components.get(schemas_index + 1)?;
    let file_name = components.get(schemas_index + 2)?;
    let version = Path::new(file_name).file_stem()?.to_str()?;
    Some(format!("{}@{}", name, version))
}

/// Connect to PostgreSQL and return an ExtractionRepository (runs migrations).
async fn connect_db() -> Result<ExtractionRepository> {
    let pool = connect_pool().await?;
    let repo = ExtractionRepository::new(pool);
    repo.migrate().await.map_err(|e| anyhow::anyhow!(e))?;
    Ok(repo)
}

/// Connect to PostgreSQL and return the pool (runs migrations).
async fn connect_pool() -> Result<sqlx::PgPool> {
    let database_url = std::env::var("DATABASE_URL")
        .context("DATABASE_URL not set. Required for database operations.")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("Failed to connect to database")?;

    // Run migrations
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("Migration failed: {}", e))?;

    Ok(pool)
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
