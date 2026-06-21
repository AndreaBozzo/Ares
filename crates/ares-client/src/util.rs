//! Small shared helpers for the LLM client adapters.

/// Maximum number of characters of an upstream response body to embed in an
/// error message.
///
/// Error messages flow into `AppError`, which the API serializes into HTTP
/// responses — so embedding a full LLM response or scraped page would both leak
/// large amounts of content to clients and produce huge error payloads. This
/// bound keeps errors useful for debugging without either problem.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// Truncate an upstream response body for safe inclusion in an error message.
pub(crate) fn truncate_for_error(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= MAX_ERROR_BODY_CHARS {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!("{prefix}… (truncated)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_body_passes_through() {
        assert_eq!(truncate_for_error("  hello  "), "hello");
    }

    #[test]
    fn long_body_is_truncated() {
        let body = "x".repeat(MAX_ERROR_BODY_CHARS + 50);
        let out = truncate_for_error(&body);
        assert!(out.ends_with("… (truncated)"));
        // Prefix is capped at the limit (plus the appended marker).
        assert_eq!(
            out.chars().count(),
            MAX_ERROR_BODY_CHARS + "… (truncated)".chars().count()
        );
    }
}
