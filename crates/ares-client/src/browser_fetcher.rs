use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ares_core::error::AppError;
use ares_core::traits::Fetcher;
use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt;

/// Headless-browser fetcher using Chromium via the Chrome DevTools Protocol.
///
/// Unlike [`super::ReqwestFetcher`], this renders JavaScript before returning
/// the HTML, making it suitable for SPAs (React, Angular, Vue) and pages
/// with lazy-loaded content.
///
/// A single Chromium process is shared across all clones of this struct;
/// each [`Fetcher::fetch`] call opens a new tab, grabs the rendered HTML,
/// and closes the tab.
///
/// # Example
///
/// ```rust,no_run
/// use ares_client::BrowserFetcher;
/// use ares_core::traits::Fetcher;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let fetcher = BrowserFetcher::new().await?;
/// let html = fetcher.fetch("https://example.com").await?;
/// println!("{}", &html[..200]);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct BrowserFetcher {
    browser: Arc<Browser>,
    timeout: Duration,
}

impl BrowserFetcher {
    /// Launches a headless Chromium browser with a **30 s** navigation timeout.
    ///
    /// Requires a Chromium / Chrome binary reachable via `$PATH` (or the
    /// default locations checked by `chromiumoxide`).
    pub async fn new() -> Result<Self, AppError> {
        Self::with_timeout(Duration::from_secs(30)).await
    }

    /// Launches a headless Chromium browser with a custom navigation timeout.
    pub async fn with_timeout(timeout: Duration) -> Result<Self, AppError> {
        let mut builder = BrowserConfig::builder();
        builder = builder.no_sandbox().disable_default_args();

        // Snap-packaged Chromium exposes a wrapper that rejects standard
        // Chrome CLI flags (--headless, --disable-gpu, …).  We try to
        // locate the *real* binary buried inside the snap, falling back
        // to any other Chrome/Chromium the user may have installed.
        // This does NOT force any particular installation method.
        if let Some(bin) = Self::find_chrome_binary() {
            tracing::info!("Using Chrome binary: {}", bin.display());
            builder = builder.chrome_executable(bin);
        }

        let config = builder
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-extensions")
            .arg("--disable-popup-blocking")
            .arg("--disable-translate")
            .arg("--no-first-run")
            .build()
            .map_err(|e| AppError::Generic(format!("Browser config error: {e}")))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| AppError::Generic(format!("Failed to launch browser: {e}")))?;

        // The CDP handler must be polled continuously for the connection to work.
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    tracing::warn!("Browser CDP handler error: {event:?}");
                    break;
                }
            }
        });

        Ok(Self {
            browser: Arc::new(browser),
            timeout,
        })
    }

    /// Tries to locate the real Chrome/Chromium binary.
    ///
    /// On systems where Chromium is installed via **snap**, the wrapper at
    /// `/snap/bin/chromium` strips unknown CLI flags, breaking headless mode.
    /// We look for the real binary inside the snap first, then fall back to
    /// well-known system paths.  If nothing is found we return `None` and let
    /// `chromiumoxide` do its own lookup.
    fn find_chrome_binary() -> Option<PathBuf> {
        let candidates: &[&str] = &[
            // Snap (Ubuntu default)
            "/snap/chromium/current/usr/lib/chromium-browser/chrome",
            // Flatpak
            "/var/lib/flatpak/exports/bin/org.chromium.Chromium",
            // Common apt / manual installs
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ];

        // Also honour an explicit override via env var.
        if let Ok(p) = std::env::var("CHROME_BIN") {
            let path = PathBuf::from(&p);
            if path.exists() {
                return Some(path);
            }
        }

        candidates
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
    }
}

impl Fetcher for BrowserFetcher {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        let timeout = self.timeout;

        let result =
            tokio::time::timeout(timeout, async {
                // Open a new tab and navigate to the URL.
                let page = self.browser.new_page(url).await.map_err(|e| {
                    AppError::HttpError(format!("Failed to navigate to {url}: {e}"))
                })?;

                // Wait until <body> is present — a minimal signal that the page
                // has rendered its main content.
                page.find_element("body")
                    .await
                    .map_err(|e| AppError::HttpError(format!("Page did not render body: {e}")))?;

                // Grab the fully-rendered DOM.
                let html = page.content().await.map_err(|e| {
                    AppError::HttpError(format!("Failed to read page content: {e}"))
                })?;

                // Close the tab to free browser resources.
                let _ = page.close().await;

                Ok::<String, AppError>(html)
            })
            .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(AppError::Timeout(timeout.as_secs())),
        }
    }
}
