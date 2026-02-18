use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use ares_client::{HtmdCleaner, OpenAiExtractor, OpenAiExtractorFactory, ReqwestFetcher};
use ares_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use ares_core::job::{CreateScrapeJobRequest, JobStatus, WorkerConfig};
use ares_core::job_queue::JobQueue;
use ares_core::traits::Fetcher;
use ares_core::worker::{TracingWorkerReporter, WorkerService};
use ares_core::{NullStore, SchemaResolver, ScrapeService};
use ares_db::{Database, DatabaseConfig, ExtractionRepository};

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

        /// Use headless browser for JS-rendered pages (requires `browser` feature)
        #[arg(long, default_value_t = false)]
        browser: bool,

        /// HTTP fetch timeout in seconds (default: 30)
        #[arg(long)]
        fetch_timeout: Option<u64>,

        /// LLM API timeout in seconds (default: 120)
        #[arg(long)]
        llm_timeout: Option<u64>,

        /// Custom system prompt for LLM extraction
        #[arg(long)]
        system_prompt: Option<String>,

        /// Skip saving when extracted data hasn't changed (requires --save)
        #[arg(long, default_value_t = false)]
        skip_unchanged: bool,
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

        /// Use headless browser for JS-rendered pages (requires `browser` feature)
        #[arg(long, default_value_t = false)]
        browser: bool,

        /// HTTP fetch timeout in seconds (default: 30)
        #[arg(long)]
        fetch_timeout: Option<u64>,

        /// LLM API timeout in seconds (default: 120)
        #[arg(long)]
        llm_timeout: Option<u64>,

        /// Custom system prompt for LLM extraction
        #[arg(long)]
        system_prompt: Option<String>,

        /// Skip saving when extracted data hasn't changed
        #[arg(long, default_value_t = false)]
        skip_unchanged: bool,
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
            browser,
            fetch_timeout,
            llm_timeout,
            system_prompt,
            skip_unchanged,
        } => {
            let resolved = SchemaResolver::new("schemas").resolve(&schema)?;
            let schema_name = schema_name.unwrap_or(resolved.name);
            let schema_value = resolved.schema;

            let opts = ScrapeOpts {
                url: &url,
                schema_value,
                schema_name: &schema_name,
                model: &model,
                base_url: &base_url,
                api_key: &api_key,
                save,
                fetch_timeout: fetch_timeout.map(Duration::from_secs),
                llm_timeout: llm_timeout.map(Duration::from_secs),
                system_prompt: system_prompt.as_deref(),
                skip_unchanged,
            };

            if browser {
                let fetcher = create_browser_fetcher(opts.fetch_timeout).await?;
                cmd_scrape(fetcher, opts).await?;
            } else {
                let fetcher = match opts.fetch_timeout {
                    Some(t) => ReqwestFetcher::with_timeout(t),
                    None => ReqwestFetcher::new(),
                }
                .context("Failed to create HTTP client")?;
                cmd_scrape(fetcher, opts).await?;
            }
        }

        Commands::History {
            url,
            schema_name,
            limit,
        } => {
            let db = Database::connect(&DatabaseConfig::from_env()?).await?;
            db.migrate().await?;
            let repo = db.extraction_repo();
            cmd_history(&url, &schema_name, limit, &repo).await?;
        }

        Commands::Job { action } => {
            let db = Database::connect(&DatabaseConfig::from_env()?).await?;
            db.migrate().await?;
            let job_repo = db.job_repo();

            match action {
                JobCommands::Create {
                    url,
                    schema,
                    model,
                    base_url,
                    schema_name,
                } => {
                    let resolved = SchemaResolver::new("schemas").resolve(&schema)?;
                    let schema_name = schema_name.unwrap_or(resolved.name);
                    let schema_value = resolved.schema;

                    let request = CreateScrapeJobRequest::new(
                        url,
                        schema_name,
                        schema_value,
                        model,
                        base_url,
                    );
                    let job = job_repo.create_job(request).await?;
                    println!("Created job: {}", job.id);
                }

                JobCommands::List { status, limit } => {
                    let status_filter = status
                        .map(|s| {
                            s.parse::<JobStatus>()
                                .map_err(|e| anyhow::anyhow!("Invalid status: {}", e))
                        })
                        .transpose()?;

                    let jobs = job_repo.list_jobs(status_filter, limit).await?;

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
                        .await?
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
                    job_repo.cancel_job(id).await?;
                    println!("Cancelled job: {}", id);
                }
            }
        }

        Commands::Worker {
            worker_id,
            poll_interval,
            api_key,
            browser,
            fetch_timeout,
            llm_timeout,
            system_prompt,
            skip_unchanged,
        } => {
            let worker_opts = WorkerOpts {
                api_key: &api_key,
                worker_id,
                poll_interval,
                fetch_timeout: fetch_timeout.map(Duration::from_secs),
                llm_timeout: llm_timeout.map(Duration::from_secs),
                system_prompt: system_prompt.as_deref(),
                skip_unchanged,
            };

            if browser {
                let fetcher = create_browser_fetcher(worker_opts.fetch_timeout).await?;
                cmd_worker(fetcher, worker_opts).await?;
            } else {
                let fetcher = match worker_opts.fetch_timeout {
                    Some(t) => ReqwestFetcher::with_timeout(t),
                    None => ReqwestFetcher::new(),
                }
                .context("Failed to create HTTP client")?;
                cmd_worker(fetcher, worker_opts).await?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Generic command handlers — pure injection, no business logic.
// ---------------------------------------------------------------------------

/// Options for a one-shot scrape — passed as a single struct to keep the
/// generic `cmd_scrape` below the clippy argument-count threshold.
struct ScrapeOpts<'a> {
    url: &'a str,
    schema_value: serde_json::Value,
    schema_name: &'a str,
    model: &'a str,
    base_url: &'a str,
    api_key: &'a str,
    save: bool,
    fetch_timeout: Option<Duration>,
    llm_timeout: Option<Duration>,
    system_prompt: Option<&'a str>,
    skip_unchanged: bool,
}

/// One-shot scrape: fetch → clean → extract → (optionally) persist.
async fn cmd_scrape<F: Fetcher>(fetcher: F, opts: ScrapeOpts<'_>) -> Result<()> {
    let cleaner = HtmdCleaner::new();
    let mut extractor = OpenAiExtractor::with_base_url(opts.api_key, opts.model, opts.base_url)?;
    if let Some(t) = opts.llm_timeout {
        extractor = extractor.with_timeout(t)?;
    }
    if let Some(p) = opts.system_prompt {
        extractor = extractor.with_system_prompt(p);
    }

    let result = if opts.save {
        let db = Database::connect(&DatabaseConfig::from_env()?).await?;
        db.migrate().await?;
        let repo = db.extraction_repo();
        let service =
            ScrapeService::with_store(fetcher, cleaner, extractor, repo, opts.model.to_string())
                .with_skip_unchanged(opts.skip_unchanged);
        service
            .scrape(opts.url, &opts.schema_value, opts.schema_name)
            .await?
    } else {
        let service = ScrapeService::with_store(
            fetcher,
            cleaner,
            extractor,
            NullStore,
            opts.model.to_string(),
        );
        service
            .scrape(opts.url, &opts.schema_value, opts.schema_name)
            .await?
    };

    println!("{}", serde_json::to_string_pretty(&result.extracted_data)?);
    Ok(())
}

/// Options for the worker command.
struct WorkerOpts<'a> {
    api_key: &'a str,
    worker_id: Option<String>,
    poll_interval: u64,
    fetch_timeout: Option<Duration>,
    llm_timeout: Option<Duration>,
    system_prompt: Option<&'a str>,
    skip_unchanged: bool,
}

/// Long-running worker: poll job queue → circuit breaker → scrape → persist.
async fn cmd_worker<F: Fetcher>(fetcher: F, opts: WorkerOpts<'_>) -> Result<()> {
    let db = Database::connect(&DatabaseConfig::from_env()?).await?;
    db.migrate().await?;
    let job_repo = db.job_repo();
    let extraction_repo = db.extraction_repo();

    let config = WorkerConfig::default()
        .with_poll_interval(Duration::from_secs(opts.poll_interval))
        .with_skip_unchanged(opts.skip_unchanged);
    let config = if let Some(id) = opts.worker_id {
        config.with_worker_id(id)
    } else {
        config
    };

    let cleaner = HtmdCleaner::new();
    let mut extractor_factory = OpenAiExtractorFactory::new(opts.api_key);
    if let Some(t) = opts.llm_timeout {
        extractor_factory = extractor_factory.with_llm_timeout(t);
    }
    if let Some(p) = opts.system_prompt {
        extractor_factory = extractor_factory.with_system_prompt(p);
    }
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

    worker.run(cancel, &TracingWorkerReporter).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Browser fetcher factory — feature-gated.
// ---------------------------------------------------------------------------

#[cfg(feature = "browser")]
async fn create_browser_fetcher(timeout: Option<Duration>) -> Result<ares_client::BrowserFetcher> {
    Ok(match timeout {
        Some(t) => ares_client::BrowserFetcher::with_timeout(t).await?,
        None => ares_client::BrowserFetcher::new().await?,
    })
}

#[cfg(not(feature = "browser"))]
async fn create_browser_fetcher(_timeout: Option<Duration>) -> Result<ReqwestFetcher> {
    anyhow::bail!(
        "--browser requires the `browser` feature.\n\
         Rebuild with: cargo build --features browser"
    );
}

async fn cmd_history(
    url: &str,
    schema_name: &str,
    limit: usize,
    repo: &ExtractionRepository,
) -> Result<()> {
    let history = repo.get_history(url, schema_name, limit).await?;

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
