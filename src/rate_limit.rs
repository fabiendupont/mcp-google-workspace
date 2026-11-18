use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(60);

/// Default per-client-IP request budget (requests per minute) used by the
/// HTTP layer when the policy does not set `server.rate_limit_rpm`.
pub const DEFAULT_HTTP_RPM: u32 = 600;

/// Fixed budget for the *pre-authentication* bucket that sits in front of the
/// MCP and webhook routes.
///
/// Its only job is to bound credential guessing and log flooding from a single
/// source. It must stay well below any realistic authenticated workload so it
/// never becomes the limit for a legitimate client — the operator's
/// `server.rate_limit_rpm` governs authenticated traffic instead.
pub const PRE_AUTH_RPM: u32 = 60;

/// Multiplier applied to [`PRE_AUTH_RPM`] for a request that presents a bearer
/// credential.
///
/// Anonymous floods and credential-bearing requests get separate buckets keyed
/// on the same peer address, so `401, 401, …` cannot consume the allowance of
/// the client that is about to authenticate. The multiplier keeps a
/// bogus-token flood bounded (4× the anonymous budget per address) while
/// leaving room for a legitimate client that arrives with a token.
pub const PRE_AUTH_AUTH_MULTIPLIER: u32 = 4;

/// Fixed budget for the unauthenticated `/webhooks/drive` endpoint.
///
/// Google does not send more than a handful of notifications per channel per
/// minute; a forged-notification flood from one source is cut off here before
/// it can acquire per-tenant state mutexes.
pub const WEBHOOK_RPM: u32 = 120;

/// Upper bound on the number of client keys any HTTP-layer limiter tracks.
/// Entries are evicted in least-recently-used order so an attacker cycling
/// source addresses (or credentials) cannot grow the map without bound.
pub const MAX_TRACKED_CLIENTS: usize = 1024;

/// Hard cap on the per-client timestamp vector. A policy that sets an absurdly
/// high `rate_limit_rpm` is clamped to this value so one client cannot force
/// unbounded memory allocation; 2048/min is far above any real MCP workload.
const MAX_TRACKED_REQUESTS_PER_CLIENT: u32 = 2048;

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
        // A zero budget means "disabled" (the HTTP limiter uses the same
        // convention). Without this guard the `self.timestamps[0]` below would
        // index an empty vector and panic; the release profile aborts on panic,
        // so a policy with `rate_limit_rpm: 0` would kill the process.
        if self.limit == 0 {
            return None;
        }
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
    /// Hardcoded per-service defaults (best-effort upstream quota protection).
    defaults: HashMap<String, u32>,
    /// Explicit per-service overrides from `server.rate_limits` in the policy.
    overrides: HashMap<String, u32>,
    /// Policy-wide `server.rate_limit_rpm`.
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
    pub fn new(global_fallback: Option<u32>, overrides: Option<HashMap<String, u32>>) -> Arc<Self> {
        Arc::new(Self {
            buckets: Mutex::new(HashMap::new()),
            defaults: default_limits(),
            overrides: overrides.unwrap_or_default(),
            global_fallback,
        })
    }

    /// Resolve the effective per-minute budget for a service.
    ///
    /// Precedence: explicit `server.rate_limits` entry, then the policy-wide
    /// `server.rate_limit_rpm`, then the hardcoded upstream default. The
    /// policy-provided values must win: an operator setting
    /// `rate_limit_rpm: 60` wants 60 requests/minute for *every* service, not
    /// 600 for Drive because of a built-in default.
    fn limit_for(&self, service: &str) -> u32 {
        self.overrides
            .get(service)
            .copied()
            .or(self.global_fallback)
            .or_else(|| self.defaults.get(service).copied())
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

/// Outcome of an HTTP-layer sliding-window admission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateCheck {
    /// The request is within budget and has been recorded.
    Allowed,
    /// The request is over budget. `retry_after` is when the oldest request in
    /// the window expires and a slot frees up.
    Limited { retry_after: Duration },
}

impl RateCheck {
    pub fn is_allowed(self) -> bool {
        matches!(self, RateCheck::Allowed)
    }

    /// Value for a `Retry-After` header (whole seconds, at least 1).
    pub fn retry_after_secs(self) -> u64 {
        match self {
            RateCheck::Allowed => 0,
            RateCheck::Limited { retry_after } => retry_after.as_secs().max(1),
        }
    }
}

/// HTTP-layer sliding-window limiter keyed by an opaque client key.
///
/// The key is derived by the caller: a peer IP (`ConnectInfo`) for the
/// pre-authentication buckets, or the authenticated subject for the
/// post-authentication bucket. The limiter itself never inspects the request.
///
/// The tracked-key map is bounded: when it is full the least-recently-used key
/// is evicted, so a spray of spoofed source addresses or credentials cannot
/// exhaust memory.
pub struct ClientRateLimiter {
    window: Duration,
    capacity: usize,
    state: Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    /// Recent request timestamps per client key.
    entries: HashMap<String, Vec<Instant>>,
    /// Recency order, oldest first. Used for LRU eviction.
    order: VecDeque<String>,
}

impl ClientRateLimiter {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            window: WINDOW,
            capacity: capacity.max(1),
            state: Mutex::new(LimiterState::default()),
        })
    }

    /// Record a request for `key` and report whether it is within `limit`
    /// requests per window. A `limit` of 0 disables the check.
    pub async fn check(&self, key: &str, limit: u32) -> RateCheck {
        if limit == 0 {
            return RateCheck::Allowed;
        }
        let limit = limit.min(MAX_TRACKED_REQUESTS_PER_CLIENT);

        let now = Instant::now();
        let mut state = self.state.lock().await;

        if !state.entries.contains_key(key) {
            if state.entries.len() >= self.capacity {
                evict_lru(&mut state, self.capacity);
            }
            state.entries.insert(key.to_string(), Vec::new());
        }

        touch(&mut state.order, key);

        let cutoff = now - self.window;
        let timestamps = state
            .entries
            .get_mut(key)
            .expect("entry inserted or present above");
        timestamps.retain(|t| *t > cutoff);

        if (timestamps.len() as u32) < limit {
            timestamps.push(now);
            RateCheck::Allowed
        } else {
            // The oldest recorded request frees its slot when it leaves the
            // window. This is exact for a sliding window, unlike a fixed 60 s.
            let oldest = timestamps[0];
            RateCheck::Limited {
                retry_after: (oldest + self.window).saturating_duration_since(now),
            }
        }
    }

    /// Number of client keys currently tracked (bounded by `capacity`).
    pub async fn tracked_clients(&self) -> usize {
        self.state.lock().await.entries.len()
    }

    /// Whether `key` currently has an entry (used by tests).
    #[cfg(test)]
    async fn tracks(&self, key: &str) -> bool {
        self.state.lock().await.entries.contains_key(key)
    }
}

/// Move `key` to the back of the recency order.
fn touch(order: &mut VecDeque<String>, key: &str) {
    if let Some(pos) = order.iter().position(|candidate| candidate == key) {
        order.remove(pos);
    }
    order.push_back(key.to_string());
}

/// Drop the least-recently-used key. There is always at least one entry when
/// this is called (the map is at capacity).
fn evict_lru(state: &mut LimiterState, capacity: usize) {
    while let Some(victim) = state.order.pop_front() {
        if state.entries.remove(&victim).is_some() {
            break;
        }
    }
    // Defensive: order and entries drifted apart (or the order queue is empty
    // because the map was filled before any `touch`). Clear arbitrary entries
    // until the map is back under capacity so insertion can proceed without
    // exceeding the bound.
    while state.entries.len() >= capacity {
        match state.entries.keys().next().cloned() {
            Some(victim) => {
                state.entries.remove(&victim);
            }
            None => break,
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
    fn zero_limit_is_treated_as_disabled() {
        // `server.rate_limit_rpm: 0` previously won over the built-in default
        // and then indexed an empty timestamp vector inside `try_acquire`,
        // aborting the process in release builds (panic = "abort").
        let limiter = ServiceRateLimiter::new(Some(0), None);
        assert_eq!(limiter.limit_for("drive"), 0);
        let mut bucket = ServiceBucket::new(0);
        assert!(bucket.try_acquire(Instant::now()).is_none());
    }

    #[test]
    fn default_limits_set() {
        let limiter = ServiceRateLimiter::new(None, None);
        assert_eq!(limiter.limit_for("sheets"), 240);
        assert_eq!(limiter.limit_for("drive"), 600);
        assert_eq!(limiter.limit_for("docs"), 300);
    }

    #[test]
    fn policy_rpm_overrides_builtin_defaults() {
        let limiter = ServiceRateLimiter::new(Some(30), None);
        // The policy-wide value must beat the hardcoded 600 for Drive.
        assert_eq!(limiter.limit_for("drive"), 30);
        assert_eq!(limiter.limit_for("sheets"), 30);
        assert_eq!(limiter.limit_for("unknown_service"), 30);
    }

    #[test]
    fn per_service_override_beats_policy_wide_rpm() {
        let mut overrides = HashMap::new();
        overrides.insert("drive".into(), 7);
        let limiter = ServiceRateLimiter::new(Some(30), Some(overrides));
        assert_eq!(limiter.limit_for("drive"), 7);
        assert_eq!(limiter.limit_for("gmail"), 30);
    }

    #[tokio::test]
    async fn ip_limiter_enforces_sliding_window() {
        let limiter = ClientRateLimiter::new(16);
        let key = "203.0.113.5";
        assert!(limiter.check(key, 2).await.is_allowed());
        assert!(limiter.check(key, 2).await.is_allowed());
        let limited = limiter.check(key, 2).await;
        assert!(!limited.is_allowed());
        // The retry window is derived from the sliding window, not hardcoded.
        assert!(limited.retry_after_secs() >= 1);
        assert!(limited.retry_after_secs() <= 60);
        // Other clients are unaffected.
        assert!(limiter.check("203.0.113.6", 2).await.is_allowed());
    }

    #[tokio::test]
    async fn ip_limiter_is_bounded_and_evicts_lru() {
        let limiter = ClientRateLimiter::new(2);
        let (a, b, c, d) = ("10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4");

        assert!(limiter.check(a, 10).await.is_allowed());
        assert!(limiter.check(b, 10).await.is_allowed());
        assert_eq!(limiter.tracked_clients().await, 2);

        // Inserting a third address evicts the least recently used (`a`).
        assert!(limiter.check(c, 10).await.is_allowed());
        assert_eq!(limiter.tracked_clients().await, 2);
        assert!(!limiter.tracks(a).await);
        assert!(limiter.tracks(b).await);
        assert!(limiter.tracks(c).await);

        // Touching `b` makes `c` the least recently used.
        assert!(limiter.check(b, 10).await.is_allowed());
        assert!(limiter.check(d, 10).await.is_allowed());
        assert_eq!(limiter.tracked_clients().await, 2);
        assert!(limiter.tracks(b).await);
        assert!(!limiter.tracks(c).await);
    }

    /// A capacity of 1 is the smallest legal value and must never grow the map
    /// beyond it, even though eviction and re-insertion race in the same call.
    #[tokio::test]
    async fn capacity_one_stays_bounded() {
        let limiter = ClientRateLimiter::new(1);
        for key in ["a", "b", "c", "d"] {
            assert!(limiter.check(key, 10).await.is_allowed());
            assert_eq!(limiter.tracked_clients().await, 1);
        }
    }

    #[tokio::test]
    async fn zero_limit_never_limits() {
        let limiter = ClientRateLimiter::new(4);
        for _ in 0..100 {
            assert!(limiter.check("k", 0).await.is_allowed());
        }
        assert_eq!(limiter.tracked_clients().await, 0);
    }
}
