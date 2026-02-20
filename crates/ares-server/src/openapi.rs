use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Ares API",
        version = "0.1.0",
        description = "Web scraper with LLM-powered structured data extraction."
    ),
    paths(
        crate::routes::create_job,
        crate::routes::list_jobs,
        crate::routes::get_job,
        crate::routes::cancel_job,
        crate::routes::get_extractions,
        crate::routes::health,
    ),
    components(schemas(
        crate::dto::CreateJobRequest,
        crate::dto::CreateJobResponse,
        crate::dto::JobResponse,
        crate::dto::JobListResponse,
        crate::dto::ExtractionResponse,
        crate::dto::ExtractionHistoryResponse,
        crate::dto::HealthResponse,
        crate::dto::ErrorResponse,
    )),
    tags(
        (name = "jobs", description = "Scrape job management"),
        (name = "extractions", description = "Extraction history"),
        (name = "system", description = "Health and system status"),
    ),
    security(("bearer" = []))
)]
pub struct ApiDoc;
