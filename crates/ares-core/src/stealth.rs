//! Browser stealth configuration for anti-bot evasion.
//!
//! Controls which fingerprint-obfuscation techniques are applied when
//! using the headless browser fetcher. All features are **opt-in** —
//! the default `StealthConfig` disables all techniques, and callers can
//! selectively enable individual techniques or use [`StealthConfig::full`]
//! to enable everything.

use crate::rand::random_index;

/// Configuration for browser stealth / anti-fingerprinting.
///
/// When enabled, a browser-based [`Fetcher`](crate::traits::Fetcher) will apply
/// these techniques on each new page before navigation completes.
#[derive(Debug, Clone)]
pub struct StealthConfig {
    /// Hide `navigator.webdriver`, fake plugins, permissions, WebGL vendor,
    /// and set `window.chrome`.  Uses chromiumoxide's built-in stealth mode.
    pub hide_webdriver: bool,

    /// Override the User-Agent header with a realistic browser string.
    /// When combined with `UserAgentPool`, a random UA is chosen per page.
    pub rotate_user_agent: bool,

    /// Randomise the viewport dimensions (`window.innerWidth/Height`,
    /// `screen.width/height`) within common desktop ranges.
    pub randomize_viewport: bool,

    /// Override `navigator.languages` with a realistic value.
    pub spoof_languages: bool,

    /// Override `navigator.platform` with a value matching the User-Agent OS.
    pub spoof_platform: bool,
}

impl StealthConfig {
    /// Full stealth — all techniques enabled.
    pub fn full() -> Self {
        Self {
            hide_webdriver: true,
            rotate_user_agent: true,
            randomize_viewport: true,
            spoof_languages: true,
            spoof_platform: true,
        }
    }

    /// No stealth — stock headless browser behaviour.
    pub fn disabled() -> Self {
        Self {
            hide_webdriver: false,
            rotate_user_agent: false,
            randomize_viewport: false,
            spoof_languages: false,
            spoof_platform: false,
        }
    }
}

impl Default for StealthConfig {
    /// Defaults to [`disabled`](Self::disabled) — stealth is opt-in.
    fn default() -> Self {
        Self::disabled()
    }
}

/// Common desktop viewport dimensions (width, height).
pub const VIEWPORT_SIZES: &[(u32, u32)] = &[
    (1366, 768),
    (1440, 900),
    (1536, 864),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (1280, 720),
    (1280, 800),
    (1680, 1050),
    (1920, 1200),
];

/// Pick a random viewport from the common pool.
pub fn random_viewport() -> (u32, u32) {
    VIEWPORT_SIZES[random_index(VIEWPORT_SIZES.len())]
}

/// Navigator platform strings paired with User-Agent OS patterns they match.
///
/// Used to ensure `navigator.platform` is consistent with the User-Agent.
pub fn platform_for_ua(ua: &str) -> &'static str {
    if ua.contains("Windows") {
        "Win32"
    } else if ua.contains("Macintosh") || ua.contains("Mac OS") {
        "MacIntel"
    } else if ua.contains("Linux") && !ua.contains("Android") {
        "Linux x86_64"
    } else if ua.contains("Android") {
        "Linux armv8l"
    } else if ua.contains("iPhone") {
        "iPhone"
    } else {
        "Win32" // safe default — Windows is most common
    }
}

/// Realistic `navigator.languages` values.
const LANGUAGE_SETS: &[&str] = &[
    r#"["en-US","en"]"#,
    r#"["en-US","en","es"]"#,
    r#"["en-GB","en"]"#,
    r#"["en-US","en","fr"]"#,
    r#"["en-US","en","de"]"#,
    r#"["en"]"#,
];

/// Pick a random `navigator.languages` JSON array string.
pub fn random_languages() -> &'static str {
    LANGUAGE_SETS[random_index(LANGUAGE_SETS.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_enables_everything() {
        let c = StealthConfig::full();
        assert!(c.hide_webdriver);
        assert!(c.rotate_user_agent);
        assert!(c.randomize_viewport);
        assert!(c.spoof_languages);
        assert!(c.spoof_platform);
    }

    #[test]
    fn disabled_disables_everything() {
        let c = StealthConfig::disabled();
        assert!(!c.hide_webdriver);
        assert!(!c.rotate_user_agent);
        assert!(!c.randomize_viewport);
        assert!(!c.spoof_languages);
        assert!(!c.spoof_platform);
    }

    #[test]
    fn default_is_disabled() {
        let c = StealthConfig::default();
        assert!(!c.hide_webdriver);
    }

    #[test]
    fn platform_detection() {
        assert_eq!(
            platform_for_ua("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
            "Win32"
        );
        assert_eq!(
            platform_for_ua("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"),
            "MacIntel"
        );
        assert_eq!(
            platform_for_ua("Mozilla/5.0 (X11; Linux x86_64)"),
            "Linux x86_64"
        );
        assert_eq!(
            platform_for_ua("Mozilla/5.0 (Linux; Android 14; Pixel 8)"),
            "Linux armv8l"
        );
        assert_eq!(
            platform_for_ua("Mozilla/5.0 (iPhone; CPU iPhone OS 18_1)"),
            "iPhone"
        );
    }

    #[test]
    fn random_viewport_is_valid() {
        for _ in 0..50 {
            let (w, h) = random_viewport();
            assert!(w >= 1280);
            assert!(h >= 720);
            assert!(VIEWPORT_SIZES.contains(&(w, h)));
        }
    }

    #[test]
    fn random_languages_is_valid_json_array() {
        for _ in 0..50 {
            let langs = random_languages();
            assert!(langs.starts_with('['));
            assert!(langs.ends_with(']'));
        }
    }
}
