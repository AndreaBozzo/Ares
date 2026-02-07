/// Smoke-test for `BrowserFetcher`.
///
/// Launches a headless Chromium, fetches <https://example.com>, and verifies
/// the rendered HTML contains the expected `<h1>`.
///
/// Run with:
///   cargo run --example browser_smoke --features browser
use ares_client::BrowserFetcher;
use ares_core::traits::Fetcher;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("Launching headless browser…");
    let fetcher = BrowserFetcher::new().await?;

    let url = "https://example.com";
    println!("Fetching {url} …");
    let html = fetcher.fetch(url).await?;

    // Basic sanity checks
    assert!(
        html.contains("<h1>Example Domain</h1>"),
        "Expected <h1> not found in rendered HTML"
    );
    assert!(
        html.len() > 500,
        "HTML suspiciously short ({} bytes)",
        html.len()
    );

    println!("OK — got {} bytes of rendered HTML", html.len());
    println!("First 300 chars:\n{}", &html[..html.len().min(300)]);
    Ok(())
}
