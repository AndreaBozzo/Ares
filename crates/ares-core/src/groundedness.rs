//! Heuristic groundedness check: flag short, atomic extracted string values that
//! don't appear in the source text (a signal of model hallucination).
//!
//! Schema validation guarantees *shape*, not *truth*: a small model asked for a
//! field it can't find tends to invent a plausible value (a fake URL, a wrong
//! name) that still satisfies the schema. This check compares extracted string
//! values against the source the model saw.
//!
//! It is deliberately advisory and lenient to avoid false positives:
//! - Long free-text values (summaries are legitimately paraphrased) are skipped.
//! - A value is considered grounded if it is a substring of the source **or**
//!   all of its significant tokens appear in the source — so reformatted dates
//!   (`May 14, 2026` → `2026-05-14`) and canonicalized values don't trip it.
//!
//! It cannot catch a real value placed in the wrong field (e.g. a site name used
//! as the author) — only values absent from the source entirely.

use serde_json::Value;

/// Max word count for a string value to be treated as an "atomic fact" worth
/// checking. Longer values are assumed to be paraphrased free text and skipped
/// (unless they look like a URL).
const MAX_ATOMIC_WORDS: usize = 8;
/// Minimum length for a token to count toward grounding (skips noise like `com`).
const MIN_TOKEN_LEN: usize = 4;

/// Return the paths of extracted string values that appear ungrounded — not
/// supported by `source`. An empty result means everything checkable is
/// grounded. Heuristic signal, not a correctness guarantee.
pub fn ungrounded_fields(source: &str, value: &Value) -> Vec<String> {
    let norm_source = source.to_lowercase();
    let mut out = Vec::new();
    walk(value, &mut String::new(), &norm_source, &mut out);
    out
}

fn walk(value: &Value, path: &mut String, source: &str, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if is_checkable(s) && !is_grounded(s, source) {
                out.push(if path.is_empty() {
                    "<root>".to_string()
                } else {
                    path.clone()
                });
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(item, path, source, out);
                path.truncate(len);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let len = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(k);
                walk(v, path, source, out);
                path.truncate(len);
            }
        }
        _ => {} // numbers / bools / null are not groundedness-checked
    }
}

fn looks_like_url(s: &str) -> bool {
    s.contains("://") || s.contains("www.")
}

/// Whether a string value is worth checking: URLs always, otherwise only short
/// "atomic" values (free-text prose is assumed paraphrased and skipped).
fn is_checkable(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && (looks_like_url(s) || s.split_whitespace().count() <= MAX_ATOMIC_WORDS)
}

fn significant_tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= MIN_TOKEN_LEN)
        .map(|t| t.to_string())
        .collect()
}

fn is_grounded(value: &str, norm_source: &str) -> bool {
    let norm_value = value.to_lowercase();
    if norm_source.contains(norm_value.trim()) {
        return true;
    }
    let tokens = significant_tokens(value);
    // No significant tokens to corroborate and not a substring → ungrounded.
    !tokens.is_empty() && tokens.iter().all(|t| norm_source.contains(t.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SOURCE: &str = "## Page metadata\n\
        - URL: https://blog.example.com/posts/rethinking-backpressure-async-rust\n\
        - Author: Mara Vinçek\n\
        - Published: 2026-05-14\n\
        - Image: https://blog.example.com/img/backpressure-hero.png\n\n\
        Rethinking Backpressure in Async Rust. A post about bounded channels and \
        cooperative yielding in Tokio services, written in Rust.";

    #[test]
    fn grounded_values_are_not_flagged() {
        let v = json!({
            "title": "Rethinking Backpressure in Async Rust",
            "author": "Mara Vinçek",
            "url": "https://blog.example.com/posts/rethinking-backpressure-async-rust",
            "hero_image": "https://blog.example.com/img/backpressure-hero.png",
            "publish_date": "2026-05-14",
            "tags": ["Rust", "Tokio"]
        });
        assert!(ungrounded_fields(SOURCE, &v).is_empty());
    }

    #[test]
    fn fabricated_url_and_author_are_flagged() {
        let v = json!({
            "author": "Foundry",
            "url": "https://foundryblog.com/rethinking-back-pressure",
        });
        let flagged = ungrounded_fields(SOURCE, &v);
        assert!(flagged.contains(&"author".to_string()), "{flagged:?}");
        assert!(flagged.contains(&"url".to_string()), "{flagged:?}");
    }

    #[test]
    fn long_paraphrased_freetext_is_skipped() {
        // A summary the model paraphrased — long, not verbatim — must NOT flag.
        let v = json!({
            "summary": "This article explains why propagating slowness through \
                        bounded queues keeps long running services stable instead \
                        of dropping work under heavy concurrent load."
        });
        assert!(ungrounded_fields(SOURCE, &v).is_empty());
    }

    #[test]
    fn reformatted_date_is_grounded_by_token() {
        // Source has the date as 2026-05-14; an ISO value shares the year token.
        let src = "Published on May 14, 2026 by the team.";
        let v = json!({ "publish_date": "2026" });
        assert!(ungrounded_fields(src, &v).is_empty());
    }

    #[test]
    fn flags_individual_array_item() {
        let v = json!({ "tags": ["Rust", "Haskell"] });
        let flagged = ungrounded_fields(SOURCE, &v);
        assert_eq!(flagged, vec!["tags[1]".to_string()]);
    }

    #[test]
    fn numbers_and_bools_are_ignored() {
        let v = json!({ "stars": 4812, "archived": false, "name": "tempest" });
        // Only "name" is a string; "tempest" is not in SOURCE → flagged. Numbers
        // and bools are not checked.
        let flagged = ungrounded_fields(SOURCE, &v);
        assert_eq!(flagged, vec!["name".to_string()]);
    }
}
