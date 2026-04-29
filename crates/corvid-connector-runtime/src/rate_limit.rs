use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRateLimit {
    pub key: String,
    pub limit: u64,
    pub window_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRateLimitDecision {
    pub allowed: bool,
    pub retry_after_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectorRateLimiter {
    buckets: BTreeMap<String, RateBucket>,
}

#[derive(Debug, Clone, Default)]
struct RateBucket {
    window_start_ms: u64,
    used: u64,
}

impl ConnectorRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(&mut self, limit: &ConnectorRateLimit, now_ms: u64) -> ConnectorRateLimitDecision {
        if limit.limit == 0 || limit.window_ms == 0 {
            return ConnectorRateLimitDecision {
                allowed: false,
                retry_after_ms: limit.window_ms,
            };
        }
        let bucket = self.buckets.entry(limit.key.clone()).or_default();
        if now_ms.saturating_sub(bucket.window_start_ms) >= limit.window_ms {
            bucket.window_start_ms = now_ms;
            bucket.used = 0;
        }
        if bucket.used >= limit.limit {
            return ConnectorRateLimitDecision {
                allowed: false,
                retry_after_ms: limit
                    .window_ms
                    .saturating_sub(now_ms.saturating_sub(bucket.window_start_ms)),
            };
        }
        bucket.used += 1;
        ConnectorRateLimitDecision {
            allowed: true,
            retry_after_ms: 0,
        }
    }
}
