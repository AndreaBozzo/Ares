//! Lightweight randomness utilities without external dependencies.
//!
//! Uses xorshift64 seeded from `SystemTime` nanos, mixed with an atomic
//! counter to avoid same-nanos collisions under high concurrency.

use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter mixed into the seed to disambiguate calls that land on
/// the same nanosecond timestamp.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return a pseudo-random index in `0..len`.
///
/// Not cryptographically secure — intended for rotation / load balancing.
pub fn random_index(len: usize) -> usize {
    debug_assert!(len > 0, "random_index requires len > 0");
    let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut x = nanos.wrapping_add(tick);
    // xorshift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as usize) % len
}
