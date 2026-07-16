use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(60);

static GLOBAL: OnceLock<Arc<ServiceRateLimiter>> = OnceLock::new();

pub fn init_global(global_fallback: Option<u32>, overrides: Option<HashMap<String, u32>>) {
    let _ = GLOBAL.set(ServiceRateLimiter::new(global_fallback, overrides));
}

pub async fn acquire_global(service: &str) {
    if let Some(limiter) = GLOBAL.get() {
        limiter.acquire(service).await;
    }
}

struct ServiceBucket {
    limit: u32,
    timestamps: Vec<Instant>,
}

impl ServiceBucket {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            timestamps: Vec::with_capacity(limit as usize),
        }
    }

    fn prune(&mut self, now: Instant) {
        let cutoff = now - WINDOW;
        self.timestamps.retain(|t| *t > cutoff);
    }

    fn try_acquire(&mut self, now: Instant) -> Option<Duration> {
        self.prune(now);
        if (self.timestamps.len() as u32) < self.limit {
            self.timestamps.push(now);
            None
        } else {
            let oldest = self.timestamps[0];
            let wait = (oldest + WINDOW) - now;
            Some(wait)
        }
    }
}

pub struct ServiceRateLimiter {
    buckets: Mutex<HashMap<String, ServiceBucket>>,
    defaults: HashMap<String, u32>,
    global_fallback: Option<u32>,
}

fn default_limits() -> HashMap<String, u32> {
    let mut m = HashMap::new();
    m.insert("drive".into(), 600);
    m.insert("sheets".into(), 240);
    m.insert("docs".into(), 300);
    m.insert("slides".into(), 300);
    m.insert("gmail".into(), 200);
    m.insert("calendar".into(), 300);
    m
}

impl ServiceRateLimiter {
    pub fn new(
        global_fallback: Option<u32>,
        overrides: Option<HashMap<String, u32>>,
    ) -> Arc<Self> {
        let mut defaults = default_limits();
        if let Some(ovr) = overrides {
            for (k, v) in ovr {
                defaults.insert(k, v);
            }
        }
        Arc::new(Self {
            buckets: Mutex::new(HashMap::new()),
            defaults,
            global_fallback,
        })
    }

    fn limit_for(&self, service: &str) -> u32 {
        self.defaults
            .get(service)
            .copied()
            .or(self.global_fallback)
            .unwrap_or(600)
    }

    pub async fn acquire(&self, service: &str) {
        loop {
            let wait = {
                let mut buckets = self.buckets.lock().await;
                let bucket = buckets
                    .entry(service.to_string())
                    .or_insert_with(|| ServiceBucket::new(self.limit_for(service)));
                bucket.try_acquire(Instant::now())
            };
            match wait {
                None => return,
                Some(duration) => {
                    tracing::debug!(
                        service = service,
                        wait_ms = duration.as_millis() as u64,
                        "Rate limit: waiting"
                    );
                    crate::metrics::record_rate_limit_wait(service, duration.as_secs_f64());
                    tokio::time::sleep(duration).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn under_limit_passes_immediately() {
        let limiter = ServiceRateLimiter::new(None, None);
        for _ in 0..5 {
            limiter.acquire("drive").await;
        }
    }

    #[tokio::test]
    async fn respects_custom_override() {
        let mut overrides = HashMap::new();
        overrides.insert("test_svc".into(), 2);
        let limiter = ServiceRateLimiter::new(None, Some(overrides));

        let start = Instant::now();
        limiter.acquire("test_svc").await;
        limiter.acquire("test_svc").await;
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn global_fallback_used_for_unknown_service() {
        let limiter = ServiceRateLimiter::new(Some(100), None);
        assert_eq!(limiter.limit_for("unknown_service"), 100);
    }

    #[test]
    fn default_limits_set() {
        let limiter = ServiceRateLimiter::new(None, None);
        assert_eq!(limiter.limit_for("sheets"), 240);
        assert_eq!(limiter.limit_for("drive"), 600);
        assert_eq!(limiter.limit_for("docs"), 300);
    }
}
