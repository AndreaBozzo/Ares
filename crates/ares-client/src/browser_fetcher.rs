use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ares_core::error::AppError;
use ares_core::stealth::{self, StealthConfig};
use ares_core::traits::Fetcher;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;

use crate::user_agent::UserAgentPool;

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
/// # Stealth mode
///
/// When a [`StealthConfig`] is provided, each new page gets anti-fingerprinting
/// injections before any site script executes:
///
/// - `navigator.webdriver` hidden
/// - `window.chrome` faked
/// - WebGL vendor/renderer obfuscated
/// - Browser plugins and permissions spoofed
/// - User-Agent rotated per page
/// - Viewport dimensions randomised
/// - `navigator.platform` and `navigator.languages` overridden
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
    stealth: StealthConfig,
    ua_pool: Option<UserAgentPool>,
}

impl BrowserFetcher {
    /// Launches a headless Chromium browser with a **30 s** navigation timeout.
    ///
    /// Requires a Chromium / Chrome binary reachable via `$PATH` (or the
    /// default locations checked by `chromiumoxide`).
    pub async fn new() -> Result<Self, AppError> {
        Self::launch(Duration::from_secs(30), None).await
    }

    /// Launches a headless Chromium browser with a custom navigation timeout.
    pub async fn with_timeout(timeout: Duration) -> Result<Self, AppError> {
        Self::launch(timeout, None).await
    }

    /// Launches a headless Chromium browser that routes traffic through a proxy.
    ///
    /// The proxy URL is passed as `--proxy-server=<url>` to Chromium.
    /// Supports HTTP, HTTPS, and SOCKS5 proxies.
    pub async fn with_proxy(proxy_url: &str) -> Result<Self, AppError> {
        Self::launch(Duration::from_secs(30), Some(proxy_url)).await
    }

    /// Launches a headless Chromium browser with a custom timeout and proxy.
    pub async fn with_timeout_and_proxy(
        timeout: Duration,
        proxy_url: Option<&str>,
    ) -> Result<Self, AppError> {
        Self::launch(timeout, proxy_url).await
    }

    /// Enable stealth mode on this fetcher.
    ///
    /// Anti-fingerprinting techniques will be applied to every new page.
    pub fn with_stealth(mut self, config: StealthConfig) -> Self {
        if config.rotate_user_agent {
            self.ua_pool = Some(UserAgentPool);
        }
        self.stealth = config;
        self
    }

    /// Internal launcher shared by all constructors.
    async fn launch(timeout: Duration, proxy_url: Option<&str>) -> Result<Self, AppError> {
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

        builder = builder
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-extensions")
            .arg("--disable-popup-blocking")
            .arg("--disable-translate")
            .arg("--no-first-run");

        if let Some(proxy) = proxy_url {
            builder = builder.arg(format!("--proxy-server={proxy}"));
            tracing::info!("Browser using proxy: {proxy}");
        }

        let config = builder
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
            stealth: StealthConfig::default(),
            ua_pool: None,
        })
    }

    /// Apply stealth injections to a freshly-opened page.
    ///
    /// Must be called *before* navigating to the target URL so that
    /// `AddScriptToEvaluateOnNewDocument` hooks fire before site JS.
    async fn apply_stealth(&self, page: &Page) -> Result<(), AppError> {
        let map_err = |e| AppError::HttpError(format!("Stealth injection failed: {e}"));

        // 1. Core stealth: webdriver, chrome, WebGL, plugins, permissions
        if self.stealth.hide_webdriver {
            page.enable_stealth_mode_with_agent("")
                .await
                .map_err(map_err)?;
        }

        // 2. User-Agent override (rotated per page)
        let ua = if self.stealth.rotate_user_agent {
            let ua = self.ua_pool.as_ref().map(|p| p.next()).unwrap_or(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            );
            page.set_user_agent(ua).await.map_err(map_err)?;
            Some(ua)
        } else {
            None
        };

        // 3. Viewport randomization
        if self.stealth.randomize_viewport {
            let (width, height) = stealth::random_viewport();
            page.execute(SetDeviceMetricsOverrideParams {
                width: width as i64,
                height: height as i64,
                device_scale_factor: 1.0,
                mobile: false,
                scale: None,
                screen_width: Some(width as i64),
                screen_height: Some(height as i64),
                position_x: None,
                position_y: None,
                dont_set_visible_size: None,
                screen_orientation: None,
                viewport: None,
            })
            .await
            .map_err(map_err)?;
        }

        // 4. navigator.platform spoofing
        if self.stealth.spoof_platform {
            let platform = ua.map(stealth::platform_for_ua).unwrap_or("Win32");
            page.execute(AddScriptToEvaluateOnNewDocumentParams {
                source: format!(
                    "Object.defineProperty(navigator, 'platform', {{ get: () => '{}' }});",
                    platform
                ),
                world_name: None,
                include_command_line_api: None,
                run_immediately: None,
            })
            .await
            .map_err(map_err)?;
        }

        // 5. navigator.languages spoofing
        if self.stealth.spoof_languages {
            let languages = stealth::random_languages();
            page.execute(AddScriptToEvaluateOnNewDocumentParams {
                source: format!(
                    "Object.defineProperty(navigator, 'languages', {{ get: () => {} }});",
                    languages
                ),
                world_name: None,
                include_command_line_api: None,
                run_immediately: None,
            })
            .await
            .map_err(map_err)?;
        }

        Ok(())
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

        candidates.iter().map(PathBuf::from).find(|p| p.exists())
    }
}

impl Fetcher for BrowserFetcher {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        let timeout = self.timeout;
        let has_stealth = self.stealth.hide_webdriver
            || self.stealth.rotate_user_agent
            || self.stealth.randomize_viewport
            || self.stealth.spoof_languages
            || self.stealth.spoof_platform;

        let result =
            tokio::time::timeout(timeout, async {
                // Open a new tab and navigate to the URL.
                let page = self.browser.new_page(url).await.map_err(|e| {
                    AppError::HttpError(format!("Failed to navigate to {url}: {e}"))
                })?;

                // Apply stealth injections before waiting for content.
                if has_stealth {
                    self.apply_stealth(&page).await?;
                }

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
