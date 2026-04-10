use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use ares_core::error::AppError;
use ares_core::proxy::ProxyConfig;
use ares_core::traits::Fetcher;
use reqwest::Client;
use url::Url;

use crate::user_agent::UserAgentPool;

/// HTTP fetcher using reqwest.
///
/// Downloads raw HTML from URLs with configurable User-Agent and timeout.
/// By default, SSRF protection is **enabled** — requests to private/reserved
/// IP ranges are blocked. Use [`allow_private_urls`](Self::allow_private_urls)
/// to disable this (e.g., for CLI usage where the user controls the machine).
///
/// Supports optional proxy rotation and User-Agent rotation for anti-bot evasion.
#[derive(Clone)]
pub struct ReqwestFetcher {
    /// Direct (no-proxy) client — used when no proxy pool is configured,
    /// or as a fallback.
    client: Client,
    timeout: Duration,
    ssrf_protection: bool,
    /// Pre-built clients, one per proxy in the pool. Rotating through these
    /// reuses connections per proxy while distributing requests across exit IPs.
    proxy_clients: Option<Arc<ProxyClients>>,
    /// If set, override the default User-Agent with a random one per request.
    ua_pool: Option<UserAgentPool>,
}

/// Holds the proxy pool and pre-built reqwest clients for each proxy.
struct ProxyClients {
    config: ProxyConfig,
    clients: Vec<Client>,
}

impl ReqwestFetcher {
    pub fn new() -> Result<Self, AppError> {
        Self::with_timeout(Duration::from_secs(30))
    }

    pub fn with_timeout(timeout: Duration) -> Result<Self, AppError> {
        let client = build_client(timeout, None)?;

        Ok(Self {
            client,
            timeout,
            ssrf_protection: true,
            proxy_clients: None,
            ua_pool: None,
        })
    }

    /// Configure proxy rotation.
    ///
    /// Pre-builds one `reqwest::Client` per proxy for connection reuse.
    pub fn with_proxies(mut self, config: ProxyConfig) -> Result<Self, AppError> {
        let mut clients = Vec::with_capacity(config.len());
        for _ in 0..config.len() {
            let proxy = config.next();
            let client = build_client(self.timeout, Some(proxy))?;
            clients.push(client);
        }
        self.proxy_clients = Some(Arc::new(ProxyClients { config, clients }));
        Ok(self)
    }

    /// Enable User-Agent rotation using a built-in pool of realistic browser UAs.
    pub fn with_random_ua(mut self) -> Self {
        self.ua_pool = Some(UserAgentPool);
        self
    }

    /// Disable SSRF protection, allowing requests to private/reserved IPs.
    ///
    /// Only use this for CLI usage where the user controls the machine.
    pub fn allow_private_urls(mut self) -> Self {
        self.ssrf_protection = false;
        self
    }

    /// Select the client and optional UA override for the next request.
    fn next_client(&self) -> (&Client, Option<&'static str>) {
        let ua = self.ua_pool.as_ref().map(|pool| pool.next());
        match &self.proxy_clients {
            Some(pc) => {
                let idx = pc.config.next_index();
                (&pc.clients[idx], ua)
            }
            None => (&self.client, ua),
        }
    }
}

/// Build a reqwest::Client with optional proxy.
fn build_client(
    timeout: Duration,
    proxy_entry: Option<&ares_core::proxy::ProxyEntry>,
) -> Result<Client, AppError> {
    let mut builder = Client::builder()
        .user_agent("Ares/0.2 (AI Scraper)")
        .timeout(timeout);

    if let Some(entry) = proxy_entry {
        let proxy_url = entry.authenticated_url();
        let proxy = reqwest::Proxy::all(&proxy_url).map_err(|e| {
            AppError::ConfigError(format!("Invalid proxy URL '{}': {e}", entry.url))
        })?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|e| AppError::HttpError(e.to_string()))
}

impl Fetcher for ReqwestFetcher {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        if self.ssrf_protection {
            validate_url(url).await?;
        }

        let (client, ua_override) = self.next_client();

        let mut request = client.get(url);
        if let Some(ua) = ua_override {
            request = request.header(reqwest::header::USER_AGENT, ua);
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                AppError::Timeout(self.timeout.as_secs())
            } else if e.is_connect() {
                AppError::NetworkError(format!("Connection failed: {e}"))
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
            .map_err(|e| AppError::HttpError(format!("Failed to read response body: {e}")))
    }
}

// ---------------------------------------------------------------------------
// SSRF protection
// ---------------------------------------------------------------------------

/// Validate a URL to prevent server-side request forgery (SSRF).
///
/// 1. Only allow `http` and `https` schemes.
/// 2. Resolve the hostname via DNS.
/// 3. Reject if any resolved IP is private/reserved.
async fn validate_url(url: &str) -> Result<(), AppError> {
    let parsed = Url::parse(url).map_err(|e| AppError::HttpError(format!("Invalid URL: {e}")))?;

    // 1. Scheme check
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(AppError::HttpError(format!(
                "URL scheme '{scheme}' is not allowed (only http/https)"
            )));
        }
    }

    // 2. Extract host
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::HttpError("URL has no host".to_string()))?;

    // 3. If the host is already an IP literal, check it directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(AppError::HttpError(format!(
                "SSRF blocked: {host} resolves to private/reserved IP"
            )));
        }
        return Ok(());
    }

    // 4. DNS resolve and check all addresses
    let port = parsed.port().unwrap_or(match parsed.scheme() {
        "https" => 443,
        _ => 80,
    });
    let addr = format!("{host}:{port}");
    let addrs: Vec<_> = tokio::net::lookup_host(&addr)
        .await
        .map_err(|e| AppError::NetworkError(format!("DNS resolution failed for {host}: {e}")))?
        .collect();

    if addrs.is_empty() {
        return Err(AppError::NetworkError(format!(
            "DNS resolution returned no addresses for {host}"
        )));
    }

    for socket_addr in &addrs {
        if is_private_ip(socket_addr.ip()) {
            return Err(AppError::HttpError(format!(
                "SSRF blocked: {host} resolves to private/reserved IP {}",
                socket_addr.ip()
            )));
        }
    }

    Ok(())
}

/// Check if an IP address is in a private/reserved/link-local range.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()     // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()  // 169.254.0.0/16 (cloud metadata!)
                || v4.is_unspecified() // 0.0.0.0
                || v4.is_broadcast()   // 255.255.255.255
                || v4.is_documentation() // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // 100.64.0.0/10 (CGN)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()       // ::1
                || v6.is_unspecified() // ::
                // fe80::/10 (link-local)
                || (v6.segments()[0] & 0xFFC0) == 0xFE80
                // fc00::/7 (unique local)
                || (v6.segments()[0] & 0xFE00) == 0xFC00
                // IPv4-mapped IPv6 (::ffff:x.x.x.x) — check the embedded v4
                || match v6.to_ipv4_mapped() {
                    Some(v4) => is_private_ip(IpAddr::V4(v4)),
                    None => false,
                }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("169.254.169.254".parse().unwrap())); // cloud metadata
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("100.64.0.1".parse().unwrap())); // CGN
    }

    #[test]
    fn test_public_ipv4() {
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("93.184.216.34".parse().unwrap())); // example.com
    }

    #[test]
    fn test_private_ipv6() {
        assert!(is_private_ip("::1".parse().unwrap()));
        assert!(is_private_ip("::".parse().unwrap()));
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap())); // v4-mapped loopback
        assert!(is_private_ip("::ffff:169.254.169.254".parse().unwrap())); // v4-mapped metadata
    }

    #[test]
    fn test_public_ipv6() {
        assert!(!is_private_ip("2001:4860:4860::8888".parse().unwrap())); // Google DNS
    }

    #[tokio::test]
    async fn test_validate_url_rejects_private_ip() {
        let result = validate_url("http://127.0.0.1/admin").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SSRF blocked"));
    }

    #[tokio::test]
    async fn test_validate_url_rejects_metadata_ip() {
        let result = validate_url("http://169.254.169.254/latest/meta-data/").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("SSRF blocked"));
    }

    #[tokio::test]
    async fn test_validate_url_rejects_bad_scheme() {
        let result = validate_url("file:///etc/passwd").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[tokio::test]
    async fn test_validate_url_accepts_public() {
        // example.com should resolve to a public IP
        let result = validate_url("https://example.com").await;
        assert!(result.is_ok());
    }
}
