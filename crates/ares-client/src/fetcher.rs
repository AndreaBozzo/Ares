use ares_core::error::AppError;
use ares_core::traits::Fetcher;
use reqwest::Client;

/// HTTP fetcher using reqwest.
///
/// Downloads raw HTML from URLs with configurable User-Agent and timeout.
#[derive(Clone)]
pub struct ReqwestFetcher {
    client: Client,
}

impl ReqwestFetcher {
    pub fn new() -> Result<Self, AppError> {
        let client = Client::builder()
            .user_agent("Ares/0.1 (AI Scraper)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::HttpError(e.to_string()))?;

        Ok(Self { client })
    }
}

impl Default for ReqwestFetcher {
    fn default() -> Self {
        Self::new().expect("Failed to create HTTP client")
    }
}

impl Fetcher for ReqwestFetcher {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        let response = self.client.get(url).send().await.map_err(|e| {
            if e.is_timeout() {
                AppError::Timeout(30)
            } else if e.is_connect() {
                AppError::NetworkError(format!("Connection failed: {}", e))
            } else {
                AppError::HttpError(e.to_string())
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::HttpError(format!(
                "HTTP {} for {}",
                status.as_u16(),
                url
            )));
        }

        response
            .text()
            .await
            .map_err(|e| AppError::HttpError(format!("Failed to read response body: {}", e)))
    }
}
