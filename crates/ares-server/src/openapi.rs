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
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Adds Bearer token security scheme to the OpenAPI spec.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer",
                utoipa::openapi::security::SecurityScheme::Http(
                    utoipa::openapi::security::HttpBuilder::new()
                        .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                        .bearer_format("token")
                        .description(Some(
                            "Admin API key. Set via ARES_ADMIN_TOKEN environment variable.",
                        ))
                        .build(),
                ),
            );
        }
    }
}
