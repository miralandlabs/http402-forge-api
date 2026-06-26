use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    max_rps: u32,
}

impl RateLimiter {
    pub fn from_env() -> Self {
        let max_rps = std::env::var("RATE_LIMIT_RPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max_rps,
        }
    }

    pub fn check(&self, key: &str) -> bool {
        if self.max_rps == 0 {
            return true;
        }
        let now = Instant::now();
        let window = Duration::from_secs(1);
        let mut guard = self.inner.lock().expect("rate limit lock");
        let entries = guard.entry(key.to_string()).or_default();
        entries.retain(|t| now.duration_since(*t) < window);
        if entries.len() >= self.max_rps as usize {
            return false;
        }
        entries.push(now);
        true
    }
}
