use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;

use google_workspace::error::GwsError;

use crate::handler::GwsHandler;
use crate::policy::Policy;
use crate::rate_limit::{
    ClientRateLimiter, DEFAULT_HTTP_RPM, MAX_TRACKED_CLIENTS, PRE_AUTH_AUTH_MULTIPLIER,
    PRE_AUTH_RPM, RateCheck, WEBHOOK_RPM,
};
use crate::shared::ServerState;
use crate::subscriptions::{DEFAULT_CHANNEL_OWNER, constant_time_eq};
use crate::users::UserInstance;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

type McpService = StreamableHttpService<GwsHandler, LocalSessionManager>;

#[derive(Clone)]
struct AppState {
    /// Single-user mode: the shared state
    single: Option<Arc<Mutex<ServerState>>>,
    /// Multi-user mode: per-user instances
    users: Vec<Arc<UserInstance>>,
    /// Cached `channel_id -> owner` map used by the Drive webhook route.
    webhook_index: Arc<std::sync::Mutex<WebhookOwnerIndex>>,
}

/// Cache of the tenant that owns a Drive channel.
///
/// The webhook route used to lock *every* tenant's `ServerState` mutex on every
/// request to find the owner. That is a free CPU amplifier for unauthenticated
/// traffic: with N users, one forged notification cost N mutex acquisitions.
/// The index is rebuilt on a cache miss, but at most once per
/// [`WEBHOOK_INDEX_MIN_INTERVAL`], so a flood of forged channel ids can force
/// at most two sweeps per second instead of one per request.
#[derive(Default)]
struct WebhookOwnerIndex {
    owners: HashMap<String, String>,
    built_at: Option<Instant>,
}

/// Minimum spacing between owner-index rebuilds triggered by a cache miss.
/// Long enough to defeat amplification, short enough that a genuinely new
/// channel is routed on the next notification after at most one interval.
const WEBHOOK_INDEX_MIN_INTERVAL: Duration = Duration::from_millis(500);

impl AppState {
    fn single(state: Arc<Mutex<ServerState>>) -> Self {
        Self {
            single: Some(state),
            users: Vec::new(),
            webhook_index: Arc::new(std::sync::Mutex::new(WebhookOwnerIndex::default())),
        }
    }

    fn multi(users: Vec<UserInstance>) -> Self {
        Self {
            single: None,
            users: users.into_iter().map(Arc::new).collect(),
            webhook_index: Arc::new(std::sync::Mutex::new(WebhookOwnerIndex::default())),
        }
    }

    /// Every state together with the identity that owns its channel secrets.
    ///
    /// Multi-user mode uses the user's URL name; single-user mode uses
    /// [`DEFAULT_CHANNEL_OWNER`]. Webhook routing uses the owner so a channel
    /// belonging to one tenant can only be verified with that tenant's secret.
    fn states_with_owners(&self) -> Vec<(String, Arc<Mutex<ServerState>>)> {
        let mut states: Vec<_> = self
            .users
            .iter()
            .map(|user| (user.name.clone(), user.state.clone()))
            .collect();
        if let Some(state) = &self.single {
            states.push((DEFAULT_CHANNEL_OWNER.to_string(), state.clone()));
        }
        states
    }

    /// Every state that may hold subscriptions, for webhook fan-out and
    /// readiness checks.
    fn all_states(&self) -> Vec<Arc<Mutex<ServerState>>> {
        self.states_with_owners()
            .into_iter()
            .map(|(_, state)| state)
            .collect()
    }

    fn index_lock(&self) -> std::sync::MutexGuard<'_, WebhookOwnerIndex> {
        // The index is a pure cache: a poisoned lock is recovered, never
        // propagated, because losing it must not take the webhook route down.
        self.webhook_index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cached_owner(&self, channel_id: &str) -> Option<String> {
        self.index_lock().owners.get(channel_id).cloned()
    }

    /// Claim the right to rebuild the index, honouring the rate limit.
    fn claim_index_rebuild(&self) -> bool {
        let mut index = self.index_lock();
        let now = Instant::now();
        if index
            .built_at
            .is_some_and(|built| now.duration_since(built) < WEBHOOK_INDEX_MIN_INTERVAL)
        {
            return false;
        }
        index.built_at = Some(now);
        true
    }

    async fn rebuild_index(&self) {
        let mut owners = HashMap::new();
        for (owner, state) in self.states_with_owners() {
            let subscriptions = {
                let guard = state.lock().await;
                guard.subscriptions.clone()
            };
            let subscriptions = subscriptions.lock().await;
            for subscription in subscriptions.values() {
                owners
                    .entry(subscription.channel_id.clone())
                    .or_insert_with(|| owner.clone());
            }
        }
        self.index_lock().owners = owners;
    }

    /// Owner of `channel_id`, using the cache and rebuilding it on a miss.
    async fn owner_of_channel(&self, channel_id: &str) -> Option<String> {
        if let Some(owner) = self.cached_owner(channel_id) {
            return Some(owner);
        }
        if !self.claim_index_rebuild() {
            return None;
        }
        self.rebuild_index().await;
        self.cached_owner(channel_id)
    }
}

/// Bearer-token state shared by the `/mcp` and `/metrics` middlewares.
///
/// Policies are re-read on every request so that a SIGHUP reload which
/// rotates or revokes `server.api_token` takes effect immediately instead of
/// being pinned to the value captured at boot.
#[derive(Clone)]
struct AuthState {
    /// `--api-token` / `GWS_API_TOKEN`. When set it replaces every policy
    /// token (single-user and multi-user modes alike).
    override_token: Option<String>,
    /// Policies whose `server.api_token` is accepted.
    policies: Vec<Arc<RwLock<Policy>>>,
    /// Whether authentication was required at construction time.
    ///
    /// The *requirement* is latched; the *secret* stays live so rotation and
    /// revocation keep working. Without the latch a SIGHUP that removed
    /// `server.api_token` silently turned the listener into an unauthenticated
    /// one (the boot guard in `main.rs` only runs once).
    auth_required: bool,
}

/// Normalise a token: whitespace-trimmed, and empty means "not configured".
fn normalize_token(token: Option<String>) -> Option<String> {
    token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Whether a policy currently carries a usable `server.api_token`.
fn policy_token(policy: &Policy) -> Option<&str> {
    policy
        .api_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

impl AuthState {
    async fn new(override_token: Option<String>, policies: Vec<Arc<RwLock<Policy>>>) -> Self {
        let override_token = normalize_token(override_token);
        let mut auth_required = override_token.is_some();
        if !auth_required {
            for policy in &policies {
                let current = policy.read().await;
                if policy_token(&current).is_some() {
                    auth_required = true;
                    break;
                }
            }
        }
        Self {
            override_token,
            policies,
            auth_required,
        }
    }

    /// The effective token, if authentication is required *right now*.
    async fn configured_token(&self) -> Option<String> {
        if let Some(token) = &self.override_token {
            return Some(token.clone());
        }
        for policy in &self.policies {
            let policy = policy.read().await;
            if let Some(token) = policy_token(&policy) {
                return Some(token.to_string());
            }
        }
        None
    }

    async fn is_configured(&self) -> bool {
        self.configured_token().await.is_some()
    }

    /// Constant-time check of a presented token against every accepted token.
    async fn accepts(&self, presented: &str) -> bool {
        if let Some(expected) = &self.override_token {
            return constant_time_eq(presented, expected);
        }
        for policy in &self.policies {
            let policy = policy.read().await;
            if let Some(expected) = policy_token(&policy)
                && constant_time_eq(presented, expected)
            {
                return true;
            }
        }
        false
    }
}

/// One entry of a trusted-proxy CIDR list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrustedProxy {
    network: IpAddr,
    prefix: u8,
}

impl TrustedProxy {
    /// Parse `10.0.0.0/8`, `192.168.1.5` or `::1/128`. Invalid entries are
    /// reported by the caller, never silently trusted.
    fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim();
        if entry.is_empty() {
            return None;
        }
        let (address, prefix) = match entry.split_once('/') {
            Some((address, prefix)) => (address.trim(), Some(prefix.trim())),
            None => (entry, None),
        };
        let address: IpAddr = address.parse().ok()?;
        let max_prefix = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix.parse::<u8>().ok()?,
            None => max_prefix,
        };
        if prefix > max_prefix {
            return None;
        }
        Some(Self {
            network: mask_address(address, prefix),
            prefix,
        })
    }

    fn contains(&self, candidate: IpAddr) -> bool {
        match (self.network, candidate) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                mask_v4(network, self.prefix) == mask_v4(candidate, self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                mask_v6(network, self.prefix) == mask_v6(candidate, self.prefix)
            }
            _ => false,
        }
    }
}

fn mask_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (!0u32 << (32 - prefix))
    }
}

fn mask_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (!0u128 << (128 - prefix))
    }
}

fn mask_address(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(v4) => IpAddr::V4(Ipv4Addr::from(mask_v4(v4, prefix))),
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(mask_v6(v6, prefix))),
    }
}

/// Parse a comma-separated list of CIDR blocks / addresses.
///
/// Tolerance here is a second line of defence only: `Policy::validate_server_knobs`
/// rejects malformed `server.trusted_proxies` entries at load time, but policies
/// built programmatically (`from_services`) bypass that validation. An entry
/// that cannot be parsed is warned about and **dropped** — never treated as
/// "trust everything" — so a typo can only narrow trust, not widen it.
fn parse_trusted_proxies(raw: &str) -> Vec<TrustedProxy> {
    let mut trusted = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match TrustedProxy::parse(entry) {
            Some(proxy) => trusted.push(proxy),
            None => tracing::warn!(
                entry = entry,
                "Ignoring invalid trusted-proxy entry (expected CIDR or IP address)"
            ),
        }
    }
    trusted
}

/// Resolve the listener's trusted proxies from the boot-time policy value.
///
/// `X-Forwarded-For` is client-controlled. It is only honoured when the
/// immediate peer is a *trusted* proxy, so a direct internet client cannot
/// spoof its way into another client's rate-limit bucket. With no trusted
/// proxies configured, the header is never read.
///
/// Trusted proxies are a transport property of this listener, not of a tenant.
/// They are resolved **once, at listener construction**: a SIGHUP reload
/// replaces the policy contents but does not rebuild the middleware stack, so
/// changing `server.trusted_proxies` requires a restart. This is deliberate —
/// rebuilding the stack on reload would race in-flight requests — and the
/// documentation states the same.
///
/// `policy_entries` holds one `server.trusted_proxies` list per served policy.
/// Multi-user mode can only honour one listener-wide set, so when tenants
/// disagree the **intersection** is used: a peer is trusted only if every
/// tenant trusts it. That can never widen the trust set beyond what any single
/// tenant wrote, unlike a union.
fn configured_trusted_proxies(policy_entries: &[Vec<String>]) -> Arc<Vec<TrustedProxy>> {
    let mut policies = policy_entries.iter();
    let Some(first) = policies.next() else {
        return Arc::new(Vec::new());
    };

    let mut trusted = parse_trusted_proxies(&first.join(","));
    for entries in policies {
        let other = parse_trusted_proxies(&entries.join(","));
        if other.len() != trusted.len() || other.iter().any(|proxy| !trusted.contains(proxy)) {
            tracing::warn!(
                "Multi-user mode: server.trusted_proxies differs between users; using the \
                 intersection so one tenant's setting cannot widen another's trust"
            );
        }
        trusted.retain(|proxy| other.contains(proxy));
    }
    Arc::new(trusted)
}

/// How the client key for rate limiting is derived.
#[derive(Clone)]
struct ClientKeyConfig {
    trusted_proxies: Arc<Vec<TrustedProxy>>,
}

impl ClientKeyConfig {
    fn new(trusted_proxies: Arc<Vec<TrustedProxy>>) -> Self {
        Self { trusted_proxies }
    }

    /// The socket peer, or the client address from `X-Forwarded-For` when the
    /// peer is a trusted proxy. `None` when neither is available (embedded
    /// routers / tests without `ConnectInfo`).
    fn client_ip(&self, req: &Request) -> Option<IpAddr> {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)?;
        Some(self.resolve(peer, req.headers()))
    }

    fn resolve(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        let peer_ip = peer.ip();
        if self.trusted_proxies.is_empty()
            || !self
                .trusted_proxies
                .iter()
                .any(|proxy| proxy.contains(peer_ip))
        {
            return peer_ip;
        }
        forwarded_client_ip(headers, &self.trusted_proxies).unwrap_or(peer_ip)
    }
}

/// Client address from an `X-Forwarded-For` chain.
///
/// The list is walked right to left (nearest hop first) and the first
/// non-trusted address is the client: entries to its left are client-supplied
/// and would otherwise let a caller pick any bucket it likes.
fn forwarded_client_ip(headers: &HeaderMap, trusted: &[TrustedProxy]) -> Option<IpAddr> {
    let value = headers.get("x-forwarded-for")?.to_str().ok()?;
    let hops: Vec<IpAddr> = value
        .split(',')
        .filter_map(|hop| hop.trim().parse::<IpAddr>().ok())
        .collect();
    let first = *hops.first()?;
    for hop in hops.iter().rev() {
        if !trusted.iter().any(|proxy| proxy.contains(*hop)) {
            return Some(*hop);
        }
    }
    Some(first)
}

/// Process-wide keyed pseudonyms for credentials and channel ids.
///
/// A `RandomState` is seeded once per process, so the mapping is stable for
/// logging and rate-limit keys but cannot be reversed to the bearer token and
/// is not comparable across restarts.
fn pseudonym_keys() -> &'static std::collections::hash_map::RandomState {
    static KEYS: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    KEYS.get_or_init(std::collections::hash_map::RandomState::new)
}

fn pseudonym(value: &str) -> String {
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = pseudonym_keys().build_hasher();
    hasher.write(value.as_bytes());
    format!("{:016x}", hasher.finish())
}

/// Rate-limit key for an authenticated request.
///
/// The bearer token itself is never stored in the limiter map; a per-process
/// keyed hash identifies the credential. Unauthenticated requests fall back to
/// the client IP (only reachable when no token is required at all).
fn credential_key(headers: &HeaderMap, client_ip: Option<IpAddr>) -> Option<String> {
    match extract_bearer_token(headers) {
        Some(token) => Some(format!("cred:{}", pseudonym(&token))),
        None => client_ip.map(|ip| format!("ip:{ip}")),
    }
}

/// Per-client rate limit scoped to one owner (tenant) and the authenticated
/// credential.
///
/// Each state carries its own limiter instance, so the effective key is
/// `(owner, credential)`: two users sharing a source IP — or behind one L7
/// proxy, where every client shares the peer address — have independent
/// budgets, and one tenant's endpoint settings can never exhaust another's
/// window. Pinning the whole process to one limiter keyed only by IP did
/// exactly that.
#[derive(Clone)]
struct IpLimitState {
    limiter: Arc<ClientRateLimiter>,
    policy: Arc<RwLock<Policy>>,
    /// Owner the budget belongs to (user name, or the single-user sentinel).
    /// Used for logging and to document the effective key.
    owner: String,
    client_keys: ClientKeyConfig,
}

/// Pre-authentication bucket: bounds credential guessing and log flooding
/// before any token is compared.
#[derive(Clone)]
struct PreAuthLimitState {
    limiter: Arc<ClientRateLimiter>,
    limit: u32,
    client_keys: ClientKeyConfig,
}

pub async fn serve(
    policy: Arc<RwLock<Policy>>,
    policy_path: Option<PathBuf>,
    state: Arc<Mutex<ServerState>>,
    addr: &str,
    api_token_override: Option<String>,
) -> Result<(), GwsError> {
    let ct = CancellationToken::new();

    let (config, max_request_bytes) = {
        let policy = policy.clone();
        let policy = policy.read().await;
        let max_request_bytes =
            effective_body_limit(policy.max_request_bytes).map_err(GwsError::Validation)?;
        (
            build_http_config(ct.child_token(), &policy, addr, max_request_bytes),
            max_request_bytes,
        )
    };

    let shared_state = state.clone();
    let shared_policy = policy.clone();

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(GwsHandler::from_shared(
                shared_state.clone(),
                shared_policy.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let app_state = AppState::single(state);

    // Trusted proxies come from the boot policy and are fixed for the life of
    // the listener (see `configured_trusted_proxies`).
    let trusted_proxies = {
        let policy = policy.read().await;
        policy.trusted_proxies.clone()
    };
    let client_keys = ClientKeyConfig::new(configured_trusted_proxies(&[trusted_proxies]));

    // The authentication *requirement* is captured here, from the boot policy.
    // The secret stays live (see `AuthState`), so SIGHUP rotation still works
    // while revocation can no longer open the listener.
    let auth = AuthState::new(api_token_override, vec![policy.clone()]).await;

    if let Some(ref reload_path) = policy_path {
        spawn_policy_reload(
            policy.clone(),
            app_state.single.clone(),
            reload_path.clone(),
            auth.auth_required,
        );
    }

    let mcp_router = protected_mcp_router(
        mcp_service,
        auth.clone(),
        policy.clone(),
        IpLimitState {
            limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
            policy: policy.clone(),
            owner: DEFAULT_CHANNEL_OWNER.to_string(),
            client_keys: client_keys.clone(),
        },
        PreAuthLimitState {
            limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
            limit: PRE_AUTH_RPM,
            client_keys: client_keys.clone(),
        },
        max_request_bytes,
    );

    // Probes stay unauthenticated so kubelet can reach them. `/metrics` is
    // protected whenever a token is configured: it exposes service names,
    // tool-call counts, and rate-limit pressure.
    let app = Router::new()
        .nest_service("/mcp", mcp_router)
        .route(
            "/webhooks/drive",
            post(handle_drive_webhook).route_layer(middleware::from_fn_with_state(
                WebhookLimitState {
                    limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
                    limit: WEBHOOK_RPM,
                    client_keys,
                },
                rate_limit_webhook,
            )),
        )
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez))
        .route(
            "/metrics",
            get(handle_metrics)
                .route_layer(middleware::from_fn_with_state(auth, require_bearer_token)),
        )
        .with_state(app_state);

    run_server(app, addr, ct).await
}

/// Multi-user mode: each user is served under `/mcp/<user>` with an
/// isolated state and policy. Users that configure `server.api_token`
/// must authenticate with `Authorization: Bearer <token>`.
pub async fn serve_users(
    users: Vec<UserInstance>,
    addr: &str,
    external_url: Option<String>,
    api_token_override: Option<String>,
) -> Result<(), GwsError> {
    let ct = CancellationToken::new();
    let api_token_override = normalize_token(api_token_override);

    // Trusted proxies are listener-wide: collect every tenant's boot-time list
    // and let `configured_trusted_proxies` intersect them (see its docs). A
    // SIGHUP reload does not rebuild this stack; changing the field requires a
    // restart.
    let trusted_proxy_lists: Vec<Vec<String>> = {
        let mut lists = Vec::with_capacity(users.len());
        for user in &users {
            lists.push(user.policy.read().await.trusted_proxies.clone());
        }
        lists
    };
    let client_keys = ClientKeyConfig::new(configured_trusted_proxies(&trusted_proxy_lists));

    let mut app = Router::new()
        .route(
            "/webhooks/drive",
            post(handle_drive_webhook).route_layer(middleware::from_fn_with_state(
                WebhookLimitState {
                    limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
                    limit: WEBHOOK_RPM,
                    client_keys: client_keys.clone(),
                },
                rate_limit_webhook,
            )),
        )
        .route("/healthz", get(handle_health))
        .route("/readyz", get(handle_readyz))
        .route("/livez", get(handle_livez));

    // `/metrics` is process-wide: accept the override token or any user's
    // configured token. Probes remain open for kubelet.
    let metrics_auth = AuthState::new(
        api_token_override.clone(),
        users.iter().map(|u| u.policy.clone()).collect(),
    )
    .await;
    app = app.route(
        "/metrics",
        get(handle_metrics).route_layer(middleware::from_fn_with_state(
            metrics_auth,
            require_bearer_token,
        )),
    );

    // Do NOT share one limiter across the per-user routers: its map is keyed by
    // the client credential (or the peer IP when no token is required), while
    // the budget comes from each tenant's own policy. Sharing it made every
    // tenant consume the others' budget. Each user gets its own limiter
    // instead, which makes the effective key (user name, credential).

    for user in &users {
        let (config, max_request_bytes) = {
            let policy = user.policy.read().await;
            let max_request_bytes = effective_body_limit(policy.max_request_bytes)
                .map_err(|e| GwsError::Validation(format!("User '{}': {e}", user.name)))?;
            (
                build_http_config(ct.child_token(), &policy, addr, max_request_bytes),
                max_request_bytes,
            )
        };
        let user_state = user.state.clone();
        let user_policy = user.policy.clone();
        let handler_owner = user.name.clone();

        let mcp_service = StreamableHttpService::new(
            move || {
                Ok(GwsHandler::from_shared_for(
                    user_state.clone(),
                    user_policy.clone(),
                    &handler_owner,
                ))
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );

        let auth = AuthState::new(api_token_override.clone(), vec![user.policy.clone()]).await;
        let auth_required = auth.auth_required;

        let user_router = protected_mcp_router(
            mcp_service,
            auth,
            user.policy.clone(),
            IpLimitState {
                limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
                policy: user.policy.clone(),
                owner: user.name.clone(),
                client_keys: client_keys.clone(),
            },
            PreAuthLimitState {
                limiter: ClientRateLimiter::new(MAX_TRACKED_CLIENTS),
                limit: PRE_AUTH_RPM,
                client_keys: client_keys.clone(),
            },
            max_request_bytes,
        );

        app = app.nest_service(&format!("/mcp/{}", user.name), user_router);

        // Per-user policy reload on SIGHUP
        spawn_policy_reload(
            user.policy.clone(),
            Some(user.state.clone()),
            user.policy_path.clone(),
            auth_required,
        );

        tracing::info!(
            user = %user.name,
            endpoint = format!("/mcp/{}", user.name),
            auth = if auth_required { "bearer token" } else { "none" },
            "User endpoint mounted"
        );
    }

    let _ = external_url; // webhook URL is already set per-user at load time
    let app_state = AppState::multi(users);
    let app = app.with_state(app_state);

    run_server(app, addr, ct).await
}

/// Build the MCP routes with the full middleware stack.
///
/// axum executes the *last* added layer first, so the execution order
/// (outermost first) is:
///
/// 1. `RequestBodyLimitLayer` — boot-time backstop, cheap rejection of huge
///    declared bodies before anything else runs;
/// 2. [`rate_limit_pre_auth`] — **fixed** per-client-IP budget that bounds
///    credential guessing. It must not consume the authenticated budget:
///    unauthenticated requests sharing a bucket with a real client let an
///    attacker exhaust that client's `rate_limit_rpm` for free;
/// 3. Origin validation;
/// 4. bearer-token authentication;
/// 5. [`rate_limit_authenticated`] — the operator's `server.rate_limit_rpm`,
///    keyed on the authenticated credential (not the IP, which behind an L7
///    proxy is shared by every client);
/// 6. [`enforce_body_limit`] — re-reads the policy and buffers the body so an
///    oversized chunked body gets `413` instead of rmcp's `500`.
///
/// rmcp's own Host validation still runs inside the service.
fn protected_mcp_router(
    service: McpService,
    auth: AuthState,
    policy: Arc<RwLock<Policy>>,
    ip_limit: IpLimitState,
    pre_auth: PreAuthLimitState,
    max_request_bytes: usize,
) -> Router {
    Router::new()
        .fallback_service(service)
        .layer(middleware::from_fn_with_state(
            BodyLimitState {
                policy: policy.clone(),
                boot_limit: max_request_bytes,
            },
            enforce_body_limit,
        ))
        .layer(middleware::from_fn_with_state(
            ip_limit,
            rate_limit_authenticated,
        ))
        .layer(middleware::from_fn_with_state(auth, require_bearer_token))
        .layer(middleware::from_fn_with_state(
            policy,
            require_allowed_origin,
        ))
        .layer(middleware::from_fn_with_state(
            pre_auth,
            rate_limit_pre_auth,
        ))
        .layer(RequestBodyLimitLayer::new(max_request_bytes))
}

/// Derive rmcp's `allowed_hosts` list.
///
/// rmcp defaults to loopback only, which rejects the Service DNS name used by
/// the shipped Kubernetes deployment. Include loopback, the bind address, any
/// host from `server.allowed_origins`, and finally `GWS_ALLOWED_HOSTS`
/// (comma-separated) so operators can extend the list without changing code.
fn derive_allowed_hosts(
    bind_addr: &str,
    allowed_origins: &[String],
    env_hosts: Option<&str>,
) -> Vec<String> {
    let mut hosts: Vec<String> = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];

    let bind_host = bind_host_of(bind_addr);
    if !bind_host.is_empty() {
        hosts.push(bind_host.to_string());
    }

    for origin in allowed_origins {
        if let Some(host) = host_from_origin_entry(origin) {
            hosts.push(host);
        }
    }

    if let Some(env_hosts) = env_hosts {
        for host in env_hosts.split(',') {
            let host = host.trim();
            if !host.is_empty() {
                hosts.push(host.to_string());
            }
        }
    }

    let mut seen = HashSet::new();
    hosts.retain(|host| seen.insert(host.to_ascii_lowercase()));
    hosts
}

/// Host part of a `host:port` / `[v6]:port` bind address.
pub fn bind_host_of(addr: &str) -> &str {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((host, _)) => host,
            None => rest,
        };
    }
    match addr.rsplit_once(':') {
        Some((host, _)) => host,
        None => addr,
    }
}

/// Extract a hostname from an `allowed_origins` entry. Entries may be bare
/// hosts (`internal.corp.com`) or full origins (`https://app.example.com`).
fn host_from_origin_entry(entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if entry.contains("://") {
        return url::Url::parse(entry)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
    }
    let host = bind_host_of(entry);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty()).then(|| host.to_string())
}

fn build_http_config(
    cancellation_token: CancellationToken,
    policy: &Policy,
    addr: &str,
    max_request_bytes: usize,
) -> StreamableHttpServerConfig {
    let env_hosts = std::env::var("GWS_ALLOWED_HOSTS").ok();
    let allowed_hosts = derive_allowed_hosts(addr, &policy.allowed_origins, env_hosts.as_deref());

    StreamableHttpServerConfig::default()
        .with_cancellation_token(cancellation_token)
        .with_allowed_hosts(allowed_hosts)
        // The (validated) policy value is authoritative. rmcp's 4 MiB default
        // only applies when the policy itself asks for at least that much; a
        // smaller value (e.g. 64 KiB) is enforced here and by the axum layer.
        // Like `RequestBodyLimitLayer`, this is fixed at construction time, so
        // no SIGHUP reload changes the chunked-body cap until restart.
        .with_max_request_body_bytes(max_request_bytes)
}

/// Spawn the SIGHUP policy reload task (single-user and per-user).
///
/// `auth_required` is the latched boot-time requirement. A reload may rotate
/// the token (fine) or remove it (the listener stays locked out); either way
/// the effective authentication state is logged again so the operator is never
/// left guessing what the running process requires.
fn spawn_policy_reload(
    policy: Arc<RwLock<Policy>>,
    state: Option<Arc<Mutex<ServerState>>>,
    reload_path: PathBuf,
    auth_required: bool,
) {
    tokio::spawn(async move {
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("failed to register SIGHUP handler");
        loop {
            sighup.recv().await;
            tracing::info!("Received SIGHUP, reloading policy");
            match Policy::from_file(&reload_path) {
                Ok(new_policy) => {
                    let svcs = new_policy.allowed_services().join(", ");
                    let (token_before, trusted_before) = {
                        let current = policy.read().await;
                        (
                            policy_token(&current).is_some(),
                            current.trusted_proxies.clone(),
                        )
                    };
                    let token_after = policy_token(&new_policy).is_some();
                    let trusted_after = new_policy.trusted_proxies.clone();
                    // Re-evaluate at reload time: a policy edit can introduce
                    // an unscoped service without a restart.
                    let scoping = new_policy.scoping_warnings();
                    *policy.write().await = new_policy;
                    if let Some(ref state) = state {
                        let mut st = state.lock().await;
                        st.tools = None;
                    }

                    for warning in &scoping {
                        tracing::warn!(
                            path = %reload_path.display(),
                            "{warning}"
                        );
                    }
                    // The trusted-proxy set is baked into the listener's
                    // middleware at construction, so a reload cannot apply it.
                    // Say so instead of leaving the operator to discover it.
                    if trusted_before != trusted_after {
                        tracing::warn!(
                            path = %reload_path.display(),
                            "Policy reload changed server.trusted_proxies. The trusted-proxy \
                             list is fixed at listener construction and is NOT applied by a \
                             SIGHUP reload; restart the process to change which peers can set \
                             X-Forwarded-For"
                        );
                    }
                    if token_before != token_after {
                        if token_after {
                            tracing::warn!(
                                path = %reload_path.display(),
                                "Policy reload added server.api_token: clients must now \
                                 authenticate; previously accepted requests will get 401"
                            );
                        } else if auth_required {
                            tracing::error!(
                                path = %reload_path.display(),
                                "Policy reload removed the only server.api_token. Authentication \
                                 is still required (latched at startup), so every request will \
                                 get 401 until a token is restored in the policy or the process \
                                 is restarted with --api-token. The listener is locked out, not \
                                 opened up."
                            );
                        } else {
                            tracing::warn!(
                                path = %reload_path.display(),
                                "Policy reload removed server.api_token; this endpoint was \
                                 unauthenticated at startup and stays unauthenticated"
                            );
                        }
                    }

                    tracing::info!(
                        services = %svcs,
                        auth = if token_after || auth_required {
                            "bearer token required"
                        } else {
                            "none"
                        },
                        token_configured = token_after,
                        "Policy reloaded, tools cache cleared"
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "Policy reload failed, keeping current policy");
                }
            }
        }
    });
}

/// Wall-clock budget for producing a response on non-MCP routes.
///
/// Bounds slow-loris style handler stalls and runaway middleware chains. The
/// timer covers the handler future only; streamed response bodies continue
/// after the head is produced.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Budget for `/mcp` requests. MCP `tools/call` legitimately takes minutes for
/// Drive exports, Slides batch updates or large Gmail queries, so it cannot
/// share the 30 s budget above. Still bounded so a stuck call cannot hold a
/// concurrency slot forever.
const MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Maximum number of requests handled concurrently.
///
/// Each in-flight request can hold a body buffer up to
/// `server.max_request_bytes`, so unbounded concurrency is a memory
/// amplification vector. Requests over the cap get `503` + `Retry-After`
/// immediately instead of queueing (queueing would just move the stall).
const MAX_IN_FLIGHT_REQUESTS: usize = 512;

/// How long the server waits for a complete request body once authentication
/// has passed. A client that trickles a body holds a concurrency slot; this
/// bounds it.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-request timeout middleware. See [`REQUEST_TIMEOUT`] and
/// [`MCP_REQUEST_TIMEOUT`] for why `/mcp` gets a larger budget.
async fn request_timeout(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let budget = if path.starts_with("/mcp") {
        MCP_REQUEST_TIMEOUT
    } else {
        REQUEST_TIMEOUT
    };
    match tokio::time::timeout(budget, next.run(req)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(path = %path, "Request timed out");
            (StatusCode::REQUEST_TIMEOUT, "request timed out").into_response()
        }
    }
}

/// Bounded in-flight request limit. See [`MAX_IN_FLIGHT_REQUESTS`].
#[derive(Clone)]
struct InFlightLimit {
    permits: Arc<Semaphore>,
}

async fn limit_in_flight(State(state): State<InFlightLimit>, req: Request, next: Next) -> Response {
    let Ok(permit) = state.permits.clone().try_acquire_owned() else {
        tracing::warn!("In-flight request limit reached, rejecting request");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "1")],
            "server is at capacity",
        )
            .into_response();
    };
    let response = next.run(req).await;
    drop(permit);
    response
}

/// Bind, serve, and shut down the HTTP listener.
///
/// Transport hardening applied here:
///
/// * [`limit_in_flight`] caps concurrent requests;
/// * [`request_timeout`] caps how long a handler may take to produce a
///   response;
/// * request bodies are read with [`BODY_READ_TIMEOUT`] in
///   [`enforce_body_limit`].
///
/// HTTP/1 header-read timeout: `axum::serve` does not expose hyper's
/// `http1::Builder`, so the hyper default (30 s) applies and cannot be lowered
/// here. Shortening it would mean replacing `axum::serve` with a hand-built
/// `hyper_util` server, which needs new direct dependencies.
async fn run_server(
    app: Router,
    addr: &str,
    ct: tokio_util::sync::CancellationToken,
) -> Result<(), GwsError> {
    let app =
        app.layer(middleware::from_fn(request_timeout))
            .layer(middleware::from_fn_with_state(
                InFlightLimit {
                    permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
                },
                limit_in_flight,
            ));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to bind to {addr}: {e}")))?;

    tracing::info!(addr = addr, "HTTP server listening");

    let ct_shutdown = ct.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, shutting down HTTP server");
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down HTTP server");
            }
        }
        ct_shutdown.cancel();
    })
    .await
    .map_err(|e| GwsError::Other(anyhow::anyhow!("HTTP server error: {e}")))?;

    Ok(())
}

/// Extract the bearer token from an Authorization header.
///
/// RFC 7235 makes the auth scheme case-insensitive and allows one or more
/// spaces between the scheme and the credentials, so `Bearer`, `bearer` and
/// `BEARER` are equivalent. A missing separator (`Bearersecret`) is rejected,
/// as is an empty credential. Constant-time token comparison happens in
/// [`AuthState::accepts`], not here.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let (scheme, credentials) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = credentials.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Bearer-token authentication middleware for MCP and metrics endpoints.
///
/// Two independent facts decide the outcome:
///
/// * whether authentication is **required** — latched at construction from the
///   boot policy/override, so a reload cannot turn it off; and
/// * which token is **accepted** — read live from the policy on every request,
///   so rotation (old → new) still applies immediately.
///
/// If the requirement is latched but a reload removed the only token, there is
/// nothing to compare against. The fail-closed answer is `401` plus an error
/// log; treating "no token configured" as "no authentication needed" would let
/// anyone who can send SIGHUP (or edit the policy file) open the listener.
async fn require_bearer_token(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    if !auth.is_configured().await {
        if auth.auth_required {
            tracing::error!(
                path = %req.uri().path(),
                "Refusing request: authentication is required but no server.api_token is \
                 configured. A policy reload removed the only token; the listener stays locked \
                 out (401) until a token is restored or the process restarts."
            );
            return unauthorized(
                "authentication is required but no token is configured; \
                 restore server.api_token or restart with --api-token",
            );
        }
        return next.run(req).await;
    }

    match extract_bearer_token(req.headers()) {
        Some(token) if auth.accepts(&token).await => next.run(req).await,
        _ => unauthorized("missing or invalid Authorization: Bearer token"),
    }
}

fn unauthorized(message: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        message,
    )
        .into_response()
}

/// Origin validation middleware.
///
/// * `server.allowed_origins` non-empty: a present `Origin` must match one of
///   the configured hosts exactly, otherwise 403.
/// * `server.allowed_origins` empty: localhost origins only (the policy
///   default), everything else 403.
/// * No `Origin` header: allowed, so non-browser MCP clients keep working.
///
/// The `Host` header check lives inside rmcp and is unaffected.
async fn require_allowed_origin(
    State(policy): State<Arc<RwLock<Policy>>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let allowed = match origin.to_str() {
            Ok(origin) => policy.read().await.is_origin_allowed(origin),
            Err(_) => false,
        };
        if !allowed {
            tracing::warn!(
                origin = ?origin,
                "Rejected request with disallowed Origin header (possible cross-origin attack)"
            );
            return (StatusCode::FORBIDDEN, "Forbidden: Origin not allowed").into_response();
        }
    }
    next.run(req).await
}

/// Resolve `server.max_request_bytes` into an enforceable limit.
///
/// `0` is a configuration error, not "unlimited": [`RequestBodyLimitLayer`]
/// would reject every body, and interpreting it as unlimited would silently
/// remove the only backstop. Refusing to start is the fail-closed choice and
/// gives the operator an actionable message.
pub(crate) fn effective_body_limit(configured: usize) -> Result<usize, String> {
    if configured == 0 {
        return Err(
            "server.max_request_bytes must be greater than 0; 0 is ambiguous \
             (reject-everything vs. unlimited). Remove the field to use the \
             16 MiB default."
                .to_string(),
        );
    }
    Ok(configured)
}

/// State for [`enforce_body_limit`]: the live policy plus the boot-time cap.
#[derive(Clone)]
struct BodyLimitState {
    policy: Arc<RwLock<Policy>>,
    /// Cap fixed by [`RequestBodyLimitLayer`] at construction. It is the hard
    /// upper bound for chunked bodies: a reload may lower the effective limit
    /// but never raise it above this until restart.
    boot_limit: usize,
}

/// Enforce the current policy's `server.max_request_bytes`.
///
/// The middleware runs *after* authentication, so only an authenticated client
/// can make the server buffer a body.
///
/// Unlike the previous version it also covers bodies with no declared
/// `Content-Length` (chunked transfer encoding). Those used to surface as
/// `500 Internal Server Error` from rmcp ("Failed to read request body: length
/// limit exceeded"), contradicting the documented `413`. Buffering here lets
/// the same `413` payload be returned on both paths. MCP request bodies are
/// JSON-RPC messages that rmcp reads in full anyway, so this does not change
/// streaming behaviour; responses are untouched.
///
/// SIGHUP semantics:
///
/// * an **increase** of `max_request_bytes` has no effect until restart (the
///   boot-time cap stays in force);
/// * a **decrease** applies immediately to declared and chunked bodies alike;
/// * `0` (only reachable after a reload; startup rejects it) rejects every
///   non-empty body instead of meaning "unlimited".
async fn enforce_body_limit(
    State(state): State<BodyLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let live_limit = state.policy.read().await.max_request_bytes;
    let limit = if live_limit == 0 {
        0
    } else {
        live_limit.min(state.boot_limit)
    };

    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<usize>().ok());

    // Fast path: reject an oversized declared body without reading it.
    if declared.is_some_and(|length| length > limit) {
        tracing::warn!(
            content_length = ?declared,
            limit = limit,
            "Rejected oversized request body"
        );
        return payload_too_large(limit);
    }

    let (parts, body) = req.into_parts();
    // `to_bytes` fails when the body exceeds `limit`, which is exactly the
    // chunked case that used to escape as a 500. A body read error (client
    // disconnect) is reported the same way; it is not a server fault, so 413
    // is still better than a 500.
    let read = tokio::time::timeout(BODY_READ_TIMEOUT, axum::body::to_bytes(body, limit));
    let bytes = match read.await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            tracing::warn!(
                limit = limit,
                "Rejected request body (exceeds limit or could not be read)"
            );
            return payload_too_large(limit);
        }
        Err(_) => {
            tracing::warn!(
                limit = limit,
                timeout_secs = BODY_READ_TIMEOUT.as_secs(),
                "Timed out reading request body"
            );
            return (
                StatusCode::REQUEST_TIMEOUT,
                "timed out reading request body",
            )
                .into_response();
        }
    };

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

fn payload_too_large(limit: usize) -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("Payload too large: limit is {limit} bytes"),
    )
        .into_response()
}

/// Reject a request with `429` and a `Retry-After` derived from the window.
fn throttled(check: RateCheck, scope: &str, owner: &str, client: &str) -> Response {
    let retry_after = check.retry_after_secs().to_string();
    tracing::warn!(
        owner = %owner,
        client = %client,
        scope = scope,
        retry_after_secs = %retry_after,
        "Per-client request rate limit exceeded"
    );
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after.as_str())],
        "Too many requests",
    )
        .into_response()
}

/// Pre-authentication bucket: a small fixed per-client-IP budget in front of
/// the MCP routes.
///
/// This runs *before* the bearer check, so it must not share a bucket with
/// authenticated traffic: otherwise `401, 401, …` from anyone who knows the
/// URL consumes the real client's `server.rate_limit_rpm` (and behind an L7
/// proxy every client would share one bucket anyway).
async fn rate_limit_pre_auth(
    State(state): State<PreAuthLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(ip) = state.client_keys.client_ip(&req) else {
        return next.run(req).await;
    };
    // A request that *presents* a credential gets a separate, larger pre-auth
    // budget. Otherwise an anonymous flood from the same address (`401, 401, …`,
    // which anyone who knows the URL can generate) would consume the real
    // client's allowance before it ever reaches the bearer check. The budget is
    // still bounded per address, so token guessing is still throttled; a bogus
    // token buys at most PRE_AUTH_AUTH_MULTIPLIER times the anonymous budget.
    let (key, limit) = if req.headers().contains_key(header::AUTHORIZATION) {
        (
            format!("ip-auth:{ip}"),
            state.limit.saturating_mul(PRE_AUTH_AUTH_MULTIPLIER),
        )
    } else {
        (format!("ip:{ip}"), state.limit)
    };
    match state.limiter.check(&key, limit).await {
        RateCheck::Allowed => next.run(req).await,
        limited => throttled(limited, "pre-auth", "anonymous", &ip.to_string()),
    }
}

/// Authenticated bucket: the operator's `server.rate_limit_rpm` (falling back
/// to [`DEFAULT_HTTP_RPM`]), keyed on the authenticated credential.
///
/// Keying on the credential rather than the peer address is what makes
/// proxied deployments correct: behind an L7 proxy every client arrives from
/// the same address, so an IP-keyed bucket would either throttle all clients
/// together or (when trusted) let `X-Forwarded-For` choose the bucket.
/// `rate_limit_rpm: 0` is rejected when the policy loads; this code does not
/// add a permissive interpretation for it.
async fn rate_limit_authenticated(
    State(state): State<IpLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let limit = state
        .policy
        .read()
        .await
        .rate_limit_rpm
        .unwrap_or(DEFAULT_HTTP_RPM);

    let client_ip = state.client_keys.client_ip(&req);
    let Some(key) = credential_key(req.headers(), client_ip) else {
        // No credential and no peer address (embedded use / tests without
        // `ConnectInfo`): nothing to key on, so skip rather than fail closed.
        return next.run(req).await;
    };

    match state.limiter.check(&key, limit).await {
        RateCheck::Allowed => next.run(req).await,
        limited => throttled(limited, "authenticated", &state.owner, &key),
    }
}

/// Small fixed per-client-IP bucket in front of `/webhooks/drive`.
///
/// The route is unauthenticated by design (Google cannot present a bearer
/// token), so it needs its own cheap bound before the handler validates
/// headers and acquires tenant state.
#[derive(Clone)]
struct WebhookLimitState {
    limiter: Arc<ClientRateLimiter>,
    limit: u32,
    client_keys: ClientKeyConfig,
}

async fn rate_limit_webhook(
    State(state): State<WebhookLimitState>,
    req: Request,
    next: Next,
) -> Response {
    let Some(ip) = state.client_keys.client_ip(&req) else {
        return next.run(req).await;
    };
    let key = format!("ip:{ip}");
    match state.limiter.check(&key, state.limit).await {
        RateCheck::Allowed => next.run(req).await,
        limited => throttled(limited, "webhook", "default", &ip.to_string()),
    }
}

/// Drive push-notification endpoint.
///
/// Google echoes the watch channel's `token` in `X-Goog-Channel-Token`. We
/// require it to match the secret registered for the channel **by the tenant
/// that owns it**: unknown channels are 404 and bad tokens are 403, so forged
/// notifications cannot trigger resource notifications for a client, and one
/// tenant's secret can never authenticate another tenant's channel.
///
/// This route is unauthenticated — Drive cannot present a bearer token — so it
/// is hardened separately from `/mcp`:
///
/// * every header is length/grammar-checked **before** it is used or logged
///   (`channel_id` ≤ 128, `channel_token` ≤ 96, resource state from a fixed
///   set). Without this, hyper accepted a 520 KB `x-goog-channel-id` and each
///   request logged megabytes.
/// * the owner lookup uses a cached index instead of locking every tenant's
///   `ServerState` on every request.
/// * the body is never read (the handler takes `HeaderMap` only).
async fn handle_drive_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let channel_id = headers
        .get("x-goog-channel-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let channel_token = headers
        .get("x-goog-channel-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let resource_state = headers
        .get("x-goog-resource-state")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Validate before anything else: no logging of rejected values, no state
    // locks, no registry lookups for garbage.
    if let Err(status) = validate_webhook_headers(channel_id, channel_token, resource_state) {
        return status;
    }
    let tag = channel_tag(channel_id);

    let states = state.states_with_owners();

    // Drive sends a `sync` notification right after creating the channel —
    // potentially before the subscription has been recorded. Accept it as long
    // as the token matches the owner-scoped registry entry: channel ids are
    // unguessable and the entry records which tenant created the channel.
    // This only touches the process-wide registry (hash lookups), never a
    // tenant's `ServerState`.
    if resource_state == "sync" {
        for (owner, _) in &states {
            if crate::subscriptions::verify_channel_for(owner, channel_id, channel_token).await {
                return StatusCode::OK;
            }
        }
    }

    // In multi-user mode the channel belongs to exactly one user. The cached
    // `channel_id -> owner` index names it without sweeping every tenant, then
    // the secret is verified against that owner only.
    let Some(owner) = state.owner_of_channel(channel_id).await else {
        log_webhook_event("Drive webhook for unknown channel", &tag, false);
        return StatusCode::NOT_FOUND;
    };
    let Some((owner, st)) = states.into_iter().find(|(name, _)| name == &owner) else {
        log_webhook_event("Drive webhook owner vanished during routing", &tag, false);
        return StatusCode::NOT_FOUND;
    };

    // Re-check under this owner's lock: the index is a cache and may still
    // name a channel that was unsubscribed since the last rebuild.
    let subs = {
        let guard = st.lock().await;
        guard.subscriptions.clone()
    };
    if !crate::subscriptions::channel_owned(&subs, channel_id).await {
        log_webhook_event("Drive webhook for unknown channel", &tag, false);
        return StatusCode::NOT_FOUND;
    }

    if !crate::subscriptions::verify_channel_for(&owner, channel_id, channel_token).await {
        tracing::warn!(
            channel = %tag,
            owner = %owner,
            "Rejected Drive webhook with missing or invalid channel token"
        );
        return StatusCode::FORBIDDEN;
    }

    log_webhook_event("Drive webhook accepted", &tag, true);
    crate::subscriptions::handle_webhook(channel_id, resource_state, &subs).await;
    StatusCode::OK
}

/// Longest channel id we will accept. Ours are `gws-` + 32 hex chars (36);
/// Drive ids in the wild are well under this.
const MAX_CHANNEL_ID_LEN: usize = 128;

/// Longest channel token we will accept. Ours are 48 hex chars.
const MAX_CHANNEL_TOKEN_LEN: usize = 96;

/// Resource states Google may send. Anything else is rejected without being
/// echoed into a log line.
const ALLOWED_RESOURCE_STATES: &[&str] = &[
    "sync",
    "exists",
    "not_exists",
    "update",
    "trash",
    "remove",
    "change",
];

/// Validate the webhook headers. Errors never include the offending value.
fn validate_webhook_headers(
    channel_id: &str,
    channel_token: &str,
    resource_state: &str,
) -> Result<(), StatusCode> {
    if channel_id.is_empty() || channel_id.len() > MAX_CHANNEL_ID_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    if channel_token.len() > MAX_CHANNEL_TOKEN_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !ALLOWED_RESOURCE_STATES.contains(&resource_state) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// Short, non-reversible tag for a channel id used in logs and rate-limit keys.
///
/// Logs must never carry the full client-supplied value: it is attacker
/// controlled on the anonymous path and can be arbitrarily long.
fn channel_tag(channel_id: &str) -> String {
    let prefix: String = channel_id.chars().take(12).collect();
    format!("{prefix}\u{2026}#{}", &pseudonym(channel_id)[..8])
}

/// Sampling counter for webhook log lines. A spoofing flood must not turn into
/// a log flood: the first few events are always visible, then one in a hundred.
static WEBHOOK_LOG_SAMPLE: AtomicU64 = AtomicU64::new(0);

fn should_log_webhook_event() -> bool {
    let seen = WEBHOOK_LOG_SAMPLE.fetch_add(1, Ordering::Relaxed);
    seen < 16 || seen.is_multiple_of(100)
}

fn log_webhook_event(message: &str, tag: &str, accepted: bool) {
    if accepted {
        if should_log_webhook_event() {
            tracing::debug!(channel = %tag, "{message}");
        }
    } else if should_log_webhook_event() {
        tracing::info!(channel = %tag, "{message}");
    } else {
        tracing::trace!(channel = %tag, "{message}");
    }
}

async fn handle_health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle_readyz(State(state): State<AppState>) -> impl IntoResponse {
    let states = state.all_states();
    let mut ready = true;
    for st in &states {
        let st = st.lock().await;
        if st.tools.is_none() {
            ready = false;
        }
    }
    if states.is_empty() {
        ready = false;
    }
    if ready {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

async fn handle_livez() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn handle_metrics() -> impl IntoResponse {
    let body = crate::metrics::encode();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        h
    }

    fn policy_from_json(value: serde_json::Value) -> Policy {
        let file: crate::policy::PolicyFile = serde_json::from_value(value).unwrap();
        Policy::from_policy_file(file)
    }

    /// Spawn `app` on an ephemeral loopback port and return its base URL.
    /// Connect info is enabled so the per-IP limiter sees a peer address.
    async fn spawn(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        format!("http://{addr}")
    }

    fn test_auth_router(auth: AuthState) -> Router {
        Router::new()
            .route("/mcp", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(auth, require_bearer_token))
    }

    fn origin_router(policy: Arc<RwLock<Policy>>) -> Router {
        Router::new()
            .route("/mcp", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                policy,
                require_allowed_origin,
            ))
    }

    fn body_limit_router(policy: Arc<RwLock<Policy>>, max_bytes: usize) -> Router {
        Router::new()
            .route("/mcp", post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                BodyLimitState {
                    policy,
                    boot_limit: max_bytes,
                },
                enforce_body_limit,
            ))
            .layer(RequestBodyLimitLayer::new(max_bytes))
    }

    /// Replicates the production MCP stack (pre-auth bucket, bearer check,
    /// authenticated bucket) without rmcp, so the interaction between the two
    /// buckets and authentication is testable.
    fn layered_mcp_router(
        auth: AuthState,
        policy: Arc<RwLock<Policy>>,
        owner: &str,
        pre_auth_limit: u32,
    ) -> Router {
        let client_keys = ClientKeyConfig::new(Arc::new(Vec::new()));
        Router::new()
            .route("/mcp", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                IpLimitState {
                    limiter: ClientRateLimiter::new(64),
                    policy,
                    owner: owner.to_string(),
                    client_keys: client_keys.clone(),
                },
                rate_limit_authenticated,
            ))
            .layer(middleware::from_fn_with_state(auth, require_bearer_token))
            .layer(middleware::from_fn_with_state(
                PreAuthLimitState {
                    limiter: ClientRateLimiter::new(64),
                    limit: pre_auth_limit,
                    client_keys,
                },
                rate_limit_pre_auth,
            ))
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        assert_eq!(
            extract_bearer_token(&headers_with("Bearer secret123")),
            Some("secret123".to_string())
        );
    }

    #[test]
    fn test_extract_bearer_token_trims() {
        assert_eq!(
            extract_bearer_token(&headers_with("Bearer   secret  ")),
            Some("secret".to_string())
        );
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        assert_eq!(extract_bearer_token(&headers_with("Basic abc")), None);
    }

    #[test]
    fn test_extract_bearer_token_empty() {
        assert_eq!(extract_bearer_token(&headers_with("Bearer ")), None);
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        assert_eq!(extract_bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn test_extract_bearer_token_scheme_is_case_insensitive() {
        // RFC 7235: the auth scheme is case-insensitive.
        assert_eq!(
            extract_bearer_token(&headers_with("bearer secret")),
            Some("secret".to_string())
        );
        assert_eq!(
            extract_bearer_token(&headers_with("BEARER secret")),
            Some("secret".to_string())
        );
        assert_eq!(
            extract_bearer_token(&headers_with("BeArEr secret")),
            Some("secret".to_string())
        );
        // Missing separator between scheme and credentials.
        assert_eq!(extract_bearer_token(&headers_with("Bearersecret")), None);
        assert_eq!(extract_bearer_token(&headers_with("bearer")), None);
        // Other schemes still rejected.
        assert_eq!(extract_bearer_token(&headers_with("Basic YWJj")), None);
    }

    #[tokio::test]
    async fn bearer_scheme_casing_is_accepted_through_the_middleware() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "s3cret" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy]).await;
        let base = spawn(test_auth_router(auth)).await;
        let client = reqwest::Client::new();

        for header in ["bearer s3cret", "BEARER s3cret", "Bearer s3cret"] {
            let response = client
                .post(format!("{base}/mcp"))
                .header(header::AUTHORIZATION, header)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "header: {header}");
        }

        let no_space = client
            .post(format!("{base}/mcp"))
            .header(header::AUTHORIZATION, "Bearers3cret")
            .send()
            .await
            .unwrap();
        assert_eq!(no_space.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_normalize_token_treats_empty_as_unconfigured() {
        assert_eq!(normalize_token(Some(String::new())), None);
        assert_eq!(normalize_token(Some("   ".to_string())), None);
        assert_eq!(
            normalize_token(Some(" token ".to_string())),
            Some("token".to_string())
        );
        assert_eq!(normalize_token(None), None);
    }

    #[tokio::test]
    async fn token_required_401_without_header_and_200_with() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "s3cret" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy]).await;
        let base = spawn(test_auth_router(auth)).await;
        let client = reqwest::Client::new();

        let denied = client.post(format!("{base}/mcp")).send().await.unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let wrong = client
            .post(format!("{base}/mcp"))
            .bearer_auth("nope")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let ok = client
            .post(format!("{base}/mcp"))
            .bearer_auth("s3cret")
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cli_override_token_wins_over_policy() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "policy-token" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(Some("cli-token".to_string()), vec![policy]).await;
        assert!(auth.accepts("cli-token").await);
        assert!(!auth.accepts("policy-token").await);
    }

    #[tokio::test]
    async fn token_rotation_locks_out_the_old_token() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "old" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        assert!(auth.accepts("old").await);

        // Simulate SIGHUP replacing the policy contents.
        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "new" },
            "services": [{ "name": "drive" }]
        }));

        assert!(!auth.accepts("old").await, "old token must stop working");
        assert!(auth.accepts("new").await);
    }

    #[tokio::test]
    async fn token_rotation_takes_effect_through_the_middleware() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "old" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        let base = spawn(test_auth_router(auth)).await;
        let client = reqwest::Client::new();

        let before = client
            .post(format!("{base}/mcp"))
            .bearer_auth("old")
            .send()
            .await
            .unwrap();
        assert_eq!(before.status(), StatusCode::OK);

        // The middleware re-reads the shared policy on every request, so no
        // restart is needed for the rotation to apply.
        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "new" },
            "services": [{ "name": "drive" }]
        }));

        let old = client
            .post(format!("{base}/mcp"))
            .bearer_auth("old")
            .send()
            .await
            .unwrap();
        assert_eq!(old.status(), StatusCode::UNAUTHORIZED);

        let new = client
            .post(format!("{base}/mcp"))
            .bearer_auth("new")
            .send()
            .await
            .unwrap();
        assert_eq!(new.status(), StatusCode::OK);
    }

    /// A SIGHUP reload that removes `server.api_token` must NOT turn the
    /// listener into an unauthenticated one. The requirement is latched at
    /// construction; revocation locks everyone out until a token is restored.
    #[tokio::test]
    async fn token_revocation_locks_out_every_token() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "old" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        assert!(auth.auth_required);
        let base = spawn(test_auth_router(auth)).await;
        let client = reqwest::Client::new();

        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "" },
            "services": [{ "name": "drive" }]
        }));

        // The old token no longer matches...
        let old = client
            .post(format!("{base}/mcp"))
            .bearer_auth("old")
            .send()
            .await
            .unwrap();
        assert_eq!(old.status(), StatusCode::UNAUTHORIZED);

        // ...and with no token left to compare against, the endpoint stays
        // closed instead of falling open.
        let anonymous = client.post(format!("{base}/mcp")).send().await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let empty = client
            .post(format!("{base}/mcp"))
            .bearer_auth("")
            .send()
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::UNAUTHORIZED);
    }

    /// Same requirement for a per-user endpoint (`/mcp/<user>`), which mounts
    /// its own `AuthState` over the user's policy.
    #[tokio::test]
    async fn token_revocation_locks_out_a_per_user_endpoint() {
        let alice_policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "alice-token" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![alice_policy.clone()]).await;
        let base = spawn(
            Router::new()
                .route("/mcp/alice", post(|| async { StatusCode::OK }))
                .layer(middleware::from_fn_with_state(auth, require_bearer_token)),
        )
        .await;
        let client = reqwest::Client::new();

        *alice_policy.write().await = policy_from_json(serde_json::json!({
            "server": {},
            "services": [{ "name": "drive" }]
        }));

        for header in [None, Some("alice-token"), Some("anything")] {
            let mut request = client.post(format!("{base}/mcp/alice"));
            if let Some(header) = header {
                request = request.bearer_auth(header);
            }
            let response = request.send().await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "header {header:?} must not be accepted after revocation"
            );
        }
    }

    /// `/metrics` uses the same middleware and must stay closed too.
    #[tokio::test]
    async fn token_revocation_locks_out_metrics() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "old" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        let base = spawn(
            Router::new()
                .route(
                    "/metrics",
                    get(handle_metrics)
                        .route_layer(middleware::from_fn_with_state(auth, require_bearer_token)),
                )
                .with_state(AppState::single(Arc::new(Mutex::new(ServerState::new())))),
        )
        .await;
        let client = reqwest::Client::new();

        let before = client.get(format!("{base}/metrics")).send().await.unwrap();
        assert_eq!(before.status(), StatusCode::UNAUTHORIZED);

        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "" },
            "services": [{ "name": "drive" }]
        }));

        let after = client.get(format!("{base}/metrics")).send().await.unwrap();
        assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    }

    /// An endpoint that was unauthenticated at boot is not retroactively
    /// protected by a token appearing later, but it must not break either.
    #[tokio::test]
    async fn reload_that_empty_revokes_but_keeps_rotation_working() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "old" },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        let base = spawn(test_auth_router(auth)).await;
        let client = reqwest::Client::new();

        // Rotation old -> new keeps working through the live `accepts()`.
        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "new" },
            "services": [{ "name": "drive" }]
        }));
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .bearer_auth("new")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        // Then revocation locks it down.
        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "   " },
            "services": [{ "name": "drive" }]
        }));
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .bearer_auth("new")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    /// A token that appears only after boot does not retroactively make an
    /// endpoint require it — the requirement is latched at construction and
    /// the boot guard in `main.rs` already decided.
    #[tokio::test]
    async fn token_added_by_reload_does_not_latch_requirement() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        assert!(!auth.auth_required);
        let base = spawn(test_auth_router(auth)).await;

        *policy.write().await = policy_from_json(serde_json::json!({
            "server": { "api_token": "new" },
            "services": [{ "name": "drive" }]
        }));

        // Live token presence now requires a token, which is fine: the
        // endpoint is never weaker than at boot.
        let response = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn origin_allowed_matches_configured_host() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "allowed_origins": ["internal.corp.com"] },
            "services": [{ "name": "drive" }]
        }))));
        let base = spawn(origin_router(policy)).await;
        let client = reqwest::Client::new();

        let allowed = client
            .get(format!("{base}/mcp"))
            .header(header::ORIGIN, "https://internal.corp.com")
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        let disallowed = client
            .get(format!("{base}/mcp"))
            .header(header::ORIGIN, "https://evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(disallowed.status(), StatusCode::FORBIDDEN);

        // Non-browser clients send no Origin at all.
        let absent = client.get(format!("{base}/mcp")).send().await.unwrap();
        assert_eq!(absent.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn origin_default_is_localhost_only() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "services": [{ "name": "drive" }]
        }))));
        let base = spawn(origin_router(policy)).await;
        let client = reqwest::Client::new();

        let localhost = client
            .get(format!("{base}/mcp"))
            .header(header::ORIGIN, "http://localhost:3000")
            .send()
            .await
            .unwrap();
        assert_eq!(localhost.status(), StatusCode::OK);

        let evil = client
            .get(format!("{base}/mcp"))
            .header(header::ORIGIN, "https://evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(evil.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn oversized_body_is_rejected_with_413() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "max_request_bytes": 64 },
            "services": [{ "name": "drive" }]
        }))));
        let base = spawn(body_limit_router(policy, 64)).await;
        let client = reqwest::Client::new();

        let too_big = client
            .post(format!("{base}/mcp"))
            .body("x".repeat(1024))
            .send()
            .await
            .unwrap();
        assert_eq!(too_big.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let small = client
            .post(format!("{base}/mcp"))
            .body("ok")
            .send()
            .await
            .unwrap();
        assert_eq!(small.status(), StatusCode::OK);
    }

    /// Regression: unauthenticated requests used to consume the authenticated
    /// client's budget (`401, 401, 429`, then a valid token got `429`) because
    /// one IP-keyed bucket sat *outside* the bearer check.
    #[tokio::test]
    async fn unauthenticated_requests_do_not_consume_the_authenticated_budget() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "s3cret", "rate_limit_rpm": 2 },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        // Pre-auth budget is the fixed one, not the operator's 2/min.
        let base = spawn(layered_mcp_router(auth, policy, "test-user", PRE_AUTH_RPM)).await;
        let client = reqwest::Client::new();

        for i in 0..3 {
            let response = client.post(format!("{base}/mcp")).send().await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "unauthenticated request {i} must be 401, not throttled"
            );
        }

        // The real client still has its full 2/min budget.
        for i in 0..2 {
            let response = client
                .post(format!("{base}/mcp"))
                .bearer_auth("s3cret")
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "authenticated request {i} must not be throttled by unauthenticated traffic"
            );
        }

        let limited = client
            .post(format!("{base}/mcp"))
            .bearer_auth("s3cret")
            .send()
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = limited
            .headers()
            .get(header::RETRY_AFTER)
            .expect("429 must carry Retry-After")
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!((1..=60).contains(&retry_after), "{retry_after}");
    }

    /// An unauthenticated flood is still throttled — by the pre-auth bucket,
    /// before any token comparison or body read.
    #[tokio::test]
    async fn unauthenticated_flood_is_throttled() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "s3cret", "rate_limit_rpm": 1000 },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        let base = spawn(layered_mcp_router(auth, policy, "test-user", 2)).await;
        let client = reqwest::Client::new();

        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let throttled = client.post(format!("{base}/mcp")).send().await.unwrap();
        assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(throttled.headers().contains_key(header::RETRY_AFTER));

        // The valid token is unaffected by the anonymous flood.
        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .bearer_auth("s3cret")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    /// Behind an L7 proxy every client has the same peer address. The
    /// authenticated bucket is keyed on the credential, so one client's
    /// traffic can never land in another client's bucket.
    #[tokio::test]
    async fn authenticated_budget_is_keyed_on_the_credential() {
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "api_token": "s3cret", "rate_limit_rpm": 1 },
            "services": [{ "name": "drive" }]
        }))));
        let auth = AuthState::new(None, vec![policy.clone()]).await;
        let base = spawn(layered_mcp_router(auth, policy, "test-user", PRE_AUTH_RPM)).await;
        let client = reqwest::Client::new();

        assert_eq!(
            client
                .post(format!("{base}/mcp"))
                .bearer_auth("s3cret")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );

        // A different source address still shares the credential's bucket.
        let other_ip = reqwest::Client::builder()
            .local_address(std::net::IpAddr::from([127, 0, 0, 2]))
            .build()
            .unwrap();
        assert_eq!(
            other_ip
                .post(format!("{base}/mcp"))
                .bearer_auth("s3cret")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    /// Two tenants on the same IP have independent budgets. Before the fix one
    /// shared limiter keyed only by IP let each request consume every tenant's
    /// budget.
    #[tokio::test]
    async fn rate_limit_budget_is_scoped_to_owner() {
        fn user_route(owner: &str) -> Router {
            let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
                "server": { "rate_limit_rpm": 1 },
                "services": [{ "name": "drive" }]
            }))));
            let client_keys = ClientKeyConfig::new(Arc::new(Vec::new()));
            Router::new().route(
                "/mcp",
                post(|| async { "ok" }).layer(middleware::from_fn_with_state(
                    IpLimitState {
                        // Each owner gets its own limiter instance, so the
                        // effective map key is (owner, credential).
                        limiter: ClientRateLimiter::new(16),
                        policy,
                        owner: owner.to_string(),
                        client_keys,
                    },
                    rate_limit_authenticated,
                )),
            )
        }

        // Same router shape mounted twice; both tenants share one process.
        let alice = user_route("alice");
        let bob = user_route("bob");
        let app = Router::new().nest("/a", alice).nest("/b", bob);
        let base = spawn(app).await;

        let client = reqwest::Client::new();

        // Alice exhausts her budget.
        let alice_first = client.post(format!("{base}/a/mcp")).send().await.unwrap();
        assert_eq!(alice_first.status(), StatusCode::OK);
        let alice_again = client.post(format!("{base}/a/mcp")).send().await.unwrap();
        assert_eq!(alice_again.status(), StatusCode::TOO_MANY_REQUESTS);

        // Bob keeps his own budget on the same IP.
        let bob_first = client.post(format!("{base}/b/mcp")).send().await.unwrap();
        assert_eq!(bob_first.status(), StatusCode::OK);
    }

    #[test]
    fn x_forwarded_for_is_honoured_only_for_a_trusted_peer() {
        let policy = policy_from_json(serde_json::json!({
            "server": { "trusted_proxies": ["10.0.0.0/8", "127.0.0.1"] },
            "services": [{ "name": "drive" }]
        }));
        let config = ClientKeyConfig::new(configured_trusted_proxies(std::slice::from_ref(
            &policy.trusted_proxies,
        )));
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));

        // Trusted peer: the header names the client.
        let proxy_peer: SocketAddr = "127.0.0.1:5555".parse().unwrap();
        assert_eq!(
            config.resolve(proxy_peer, &headers),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );

        // Untrusted peer: the header is ignored entirely — otherwise any
        // direct client could pick its own bucket.
        let direct_peer: SocketAddr = "198.51.100.7:5555".parse().unwrap();
        assert_eq!(
            config.resolve(direct_peer, &headers),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );

        // No trusted proxies configured: never trust the header.
        let untrusting = ClientKeyConfig::new(configured_trusted_proxies(&[Vec::<String>::new()]));
        assert_eq!(
            untrusting.resolve(proxy_peer, &headers),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn x_forwarded_for_chain_skips_trusted_hops() {
        let policy = policy_from_json(serde_json::json!({
            "server": { "trusted_proxies": ["10.0.0.0/8"] },
            "services": [{ "name": "drive" }]
        }));
        let config = ClientKeyConfig::new(configured_trusted_proxies(std::slice::from_ref(
            &policy.trusted_proxies,
        )));
        let mut headers = HeaderMap::new();
        // Client-supplied prefix is untrusted: walk from the right and stop at
        // the first address that is not one of our proxies.
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.2.3.4, 203.0.113.9, 10.0.0.2"),
        );
        let peer: SocketAddr = "10.0.0.1:443".parse().unwrap();
        assert_eq!(
            config.resolve(peer, &headers),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn trusted_proxy_parsing_rejects_garbage() {
        assert_eq!(parse_trusted_proxies("").len(), 0);
        assert_eq!(parse_trusted_proxies("not-an-ip").len(), 0);
        assert_eq!(parse_trusted_proxies("10.0.0.0/33").len(), 0);
        let parsed = parse_trusted_proxies("10.0.0.0/8,192.168.1.5, ::1/128");
        assert_eq!(parsed.len(), 3);
        assert!(parsed[1].contains("192.168.1.5".parse::<IpAddr>().unwrap()));
        assert!(!parsed[1].contains("192.168.1.6".parse::<IpAddr>().unwrap()));
    }

    /// The trust set comes from `server.trusted_proxies`, not from an
    /// environment variable: an env var must never widen what the policy says.
    #[test]
    fn trusted_proxies_come_from_the_policy_field() {
        let policy = policy_from_json(serde_json::json!({
            "server": { "trusted_proxies": ["10.0.0.0/8", "127.0.0.1"] },
            "services": [{ "name": "drive" }]
        }));
        let proxies = configured_trusted_proxies(std::slice::from_ref(&policy.trusted_proxies));
        assert_eq!(proxies.len(), 2);
        assert!(proxies[0].contains("10.1.2.3".parse::<IpAddr>().unwrap()));
        assert!(!proxies[0].contains("192.168.1.1".parse::<IpAddr>().unwrap()));

        // The default (absent field) is empty: X-Forwarded-For is ignored.
        let default = policy_from_json(serde_json::json!({
            "services": [{ "name": "drive" }]
        }));
        assert!(
            configured_trusted_proxies(std::slice::from_ref(&default.trusted_proxies)).is_empty()
        );

        // Malformed entries are dropped, never interpreted as "trust all".
        assert!(
            configured_trusted_proxies(&[vec!["not-a-cidr".to_string()]]).is_empty(),
            "an unparsable entry must not grant trust"
        );
    }

    /// Multi-user mode has one listener-wide trust set. Differing per-tenant
    /// lists must not be unioned: that would let one tenant widen another's
    /// trust. The intersection is used instead.
    #[test]
    fn multi_user_trusted_proxies_use_the_intersection() {
        let alice = vec!["10.0.0.0/8".to_string(), "192.168.0.0/16".to_string()];
        let bob = vec!["10.0.0.0/8".to_string()];
        let proxies = configured_trusted_proxies(&[alice, bob]);
        assert_eq!(proxies.len(), 1);
        assert!(proxies[0].contains("10.9.9.9".parse::<IpAddr>().unwrap()));
        assert!(!proxies[0].contains("192.168.1.1".parse::<IpAddr>().unwrap()));

        // One tenant with no trusted proxies disables header trust for all.
        assert!(
            configured_trusted_proxies(&[vec!["10.0.0.0/8".to_string()], Vec::<String>::new()])
                .is_empty()
        );
    }

    #[test]
    fn test_effective_body_limit_rejects_zero() {
        // 0 is ambiguous (reject-everything vs. unlimited) and must never be
        // interpreted as "unlimited".
        assert!(effective_body_limit(0).is_err());
        assert_eq!(effective_body_limit(64).unwrap(), 64);
        assert_eq!(
            effective_body_limit(16 * 1024 * 1024).unwrap(),
            16 * 1024 * 1024
        );
    }

    #[tokio::test]
    async fn zero_max_request_bytes_is_fail_closed_at_request_time() {
        // A policy reloaded to 0 must reject declared bodies, not allow them.
        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "max_request_bytes": 0 },
            "services": [{ "name": "drive" }]
        }))));
        let app = Router::new().route("/mcp", post(|| async { "ok" })).layer(
            middleware::from_fn_with_state(
                BodyLimitState {
                    policy,
                    boot_limit: 1024,
                },
                enforce_body_limit,
            ),
        );
        let base = spawn(app).await;

        let response = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// A body with no `Content-Length` (chunked transfer encoding) is capped
    /// the same way as a declared one. It used to return `500 Internal Server
    /// Error` from rmcp ("Failed to read request body: length limit exceeded"),
    /// contradicting the documented `413`.
    #[tokio::test]
    async fn chunked_body_above_limit_is_rejected_with_413() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let policy = Arc::new(RwLock::new(policy_from_json(serde_json::json!({
            "server": { "max_request_bytes": 64 },
            "services": [{ "name": "drive" }]
        }))));
        let base = spawn(body_limit_router(policy, 64)).await;
        let addr = base.strip_prefix("http://").unwrap().to_string();

        // Sent by hand: reqwest always declares a Content-Length, and its
        // streaming body support needs the optional `stream` feature which
        // this crate does not enable. Raw HTTP/1.1 is also the only way to
        // reproduce the exact chunked framing hyper saw.
        async fn post_chunked(addr: &str, payload: &[u8]) -> String {
            let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            let head = format!(
                "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
                 Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket
                .write_all(format!("{:x}\r\n", payload.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(payload).await.unwrap();
            socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).await.unwrap();
            response
        }

        let oversized = post_chunked(&addr, &[b'x'; 512]).await;
        assert!(
            oversized.starts_with("HTTP/1.1 413"),
            "expected 413, got: {oversized}"
        );
        assert!(
            oversized.contains("limit is 64 bytes"),
            "unexpected body: {oversized}"
        );

        // A chunked body under the limit still works.
        let ok = post_chunked(&addr, b"ok").await;
        assert!(ok.starts_with("HTTP/1.1 200"), "expected 200, got: {ok}");
    }

    #[test]
    fn test_derive_allowed_hosts_includes_loopback_bind_and_env() {
        let hosts = derive_allowed_hosts(
            "0.0.0.0:3000",
            &["https://internal.corp.com:8443".to_string()],
            Some("mcp-gws.example.svc.cluster.local, mcp-gws.example"),
        );
        for expected in [
            "localhost",
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "internal.corp.com",
            "mcp-gws.example.svc.cluster.local",
            "mcp-gws.example",
        ] {
            assert!(
                hosts.iter().any(|host| host == expected),
                "missing {expected} in {hosts:?}"
            );
        }
    }

    #[test]
    fn test_derive_allowed_hosts_dedupes_and_handles_ipv6_bind() {
        let hosts = derive_allowed_hosts("[::1]:3000", &[], Some("localhost,LOCALHOST,"));
        assert_eq!(
            hosts
                .iter()
                .filter(|host| host.eq_ignore_ascii_case("localhost"))
                .count(),
            1
        );
        assert!(hosts.iter().any(|host| host == "::1"));
        assert!(hosts.iter().all(|host| !host.is_empty()));
    }

    #[test]
    fn test_derive_allowed_hosts_bare_origin_and_port() {
        let hosts = derive_allowed_hosts(
            "127.0.0.1:3000",
            &["internal.corp.com:8443".to_string()],
            None,
        );
        assert!(hosts.iter().any(|host| host == "internal.corp.com"));
    }

    #[tokio::test]
    async fn drive_webhook_unknown_channel_is_404() {
        let app = Router::new()
            .route("/webhooks/drive", post(handle_drive_webhook))
            .with_state(AppState::single(Arc::new(Mutex::new(ServerState::new()))));
        let base = spawn(app).await;

        let response = reqwest::Client::new()
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", "gws-unknown")
            .header("x-goog-channel-token", "whatever")
            .header("x-goog-resource-state", "exists")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn drive_webhook_missing_channel_id_is_400() {
        let app = Router::new()
            .route("/webhooks/drive", post(handle_drive_webhook))
            .with_state(AppState::single(Arc::new(Mutex::new(ServerState::new()))));
        let base = spawn(app).await;

        let response = reqwest::Client::new()
            .post(format!("{base}/webhooks/drive"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Collects log output so tests can assert a client-supplied value never
    /// reaches a log line.
    #[derive(Clone, Default)]
    struct SharedLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedLog {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedLog {
        type Writer = SharedLog;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn webhook_router() -> Router {
        Router::new()
            .route("/webhooks/drive", post(handle_drive_webhook))
            .with_state(AppState::single(Arc::new(Mutex::new(ServerState::new()))))
    }

    /// An attacker-controlled 200-byte channel id must be rejected before it is
    /// used, and must never reach a log line. The old handler logged the full
    /// value at WARN on the 403 and 404 paths.
    #[tokio::test]
    async fn drive_webhook_oversized_channel_id_is_400_and_not_logged() {
        let base = spawn(webhook_router()).await;

        let log = SharedLog::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let oversized = "z".repeat(200);
        let response = reqwest::Client::new()
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &oversized)
            .header("x-goog-channel-token", "token")
            .header("x-goog-resource-state", "exists")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let logs = String::from_utf8(
            log.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .unwrap();
        assert!(
            !logs.contains(&oversized),
            "full channel id reached a log line: {logs}"
        );
    }

    /// Validation is not over-broad: a normal-length unknown channel still
    /// takes the 404 path, and its id is only logged as a short tag.
    #[tokio::test]
    async fn drive_webhook_unknown_channel_is_logged_as_a_tag_only() {
        let base = spawn(webhook_router()).await;

        let log = SharedLog::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let channel = crate::subscriptions::new_channel_id();
        let response = reqwest::Client::new()
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel)
            .header("x-goog-channel-token", "token")
            .header("x-goog-resource-state", "exists")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let logs = String::from_utf8(
            log.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .unwrap();
        assert!(!logs.contains(&channel), "full channel id logged: {logs}");
        assert!(logs.contains(&channel[..12]), "short tag missing: {logs}");
    }

    #[tokio::test]
    async fn drive_webhook_rejects_invalid_resource_state_and_long_token() {
        let base = spawn(webhook_router()).await;
        let client = reqwest::Client::new();
        let channel = crate::subscriptions::new_channel_id();

        let bad_state = client
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel)
            .header("x-goog-channel-token", "token")
            .header("x-goog-resource-state", "<script>alert(1)</script>")
            .send()
            .await
            .unwrap();
        assert_eq!(bad_state.status(), StatusCode::BAD_REQUEST);

        let long_token = client
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel)
            .header("x-goog-channel-token", "t".repeat(200))
            .header("x-goog-resource-state", "exists")
            .send()
            .await
            .unwrap();
        assert_eq!(long_token.status(), StatusCode::BAD_REQUEST);

        let missing_state = client
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel)
            .header("x-goog-channel-token", "token")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_state.status(), StatusCode::BAD_REQUEST);
    }

    /// A valid channel id with the registered secret is still accepted (the
    /// `sync` path), so the new validation does not break notifications.
    #[tokio::test]
    async fn drive_webhook_valid_channel_still_notifies() {
        use crate::subscriptions::{register_channel_secret, unregister_channel_secret};

        let channel = crate::subscriptions::new_channel_id();
        let token = crate::subscriptions::new_channel_token();
        register_channel_secret(&channel, &token).await;
        let base = spawn(webhook_router()).await;

        let response = reqwest::Client::new()
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel)
            .header("x-goog-channel-token", &token)
            .header("x-goog-resource-state", "sync")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        unregister_channel_secret(&channel).await;
    }

    /// A tenant's bearer token must not authenticate another tenant's Drive
    /// channel, even though both live in the same process. Uses the `sync`
    /// path, which verifies the registry before any subscription exists.
    #[tokio::test]
    async fn drive_webhook_token_is_verified_against_the_owning_tenant() {
        use crate::subscriptions::{register_channel_secret_for, unregister_channel_secret_for};

        fn user(name: &str) -> UserInstance {
            let policy = policy_from_json(serde_json::json!({
                "services": [{ "name": "drive" }]
            }));
            UserInstance {
                name: name.to_string(),
                policy_path: PathBuf::new(),
                policy: Arc::new(RwLock::new(policy)),
                state: Arc::new(Mutex::new(ServerState::new())),
            }
        }

        let channel_of_bob = crate::subscriptions::new_channel_id();
        let token_of_bob = crate::subscriptions::new_channel_token();
        let token_of_alice = crate::subscriptions::new_channel_token();
        register_channel_secret_for("bob", &channel_of_bob, &token_of_bob).await;

        let app = Router::new()
            .route("/webhooks/drive", post(handle_drive_webhook))
            .with_state(AppState::multi(vec![user("alice"), user("bob")]));
        let base = spawn(app).await;
        let client = reqwest::Client::new();

        // Bob's own token for Bob's channel is accepted.
        let bob_ok = client
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel_of_bob)
            .header("x-goog-channel-token", &token_of_bob)
            .header("x-goog-resource-state", "sync")
            .send()
            .await
            .unwrap();
        assert_eq!(bob_ok.status(), StatusCode::OK);

        // Alice's token must not authenticate Bob's channel: the registry key
        // includes the owner, so it is indistinguishable from an unknown
        // channel.
        let cross_tenant = client
            .post(format!("{base}/webhooks/drive"))
            .header("x-goog-channel-id", &channel_of_bob)
            .header("x-goog-channel-token", &token_of_alice)
            .header("x-goog-resource-state", "sync")
            .send()
            .await
            .unwrap();
        assert_eq!(cross_tenant.status(), StatusCode::NOT_FOUND);

        unregister_channel_secret_for("bob", &channel_of_bob).await;
    }

    #[test]
    fn test_app_state_multi_all_states() {
        let dir = std::env::temp_dir().join("mcp-gws-test-http-multi");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("alice")).unwrap();
        std::fs::write(
            dir.join("alice/gws-policy.json"),
            r#"{"services": [{"name": "drive"}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("bob")).unwrap();
        std::fs::write(
            dir.join("bob/gws-policy.json"),
            r#"{"services": [{"name": "gmail"}]}"#,
        )
        .unwrap();

        let users = crate::users::load_users(&dir, &[], false, None).unwrap();
        let app_state = AppState::multi(users);
        assert!(app_state.single.is_none());
        assert_eq!(app_state.all_states().len(), 2);
        // Owner identities come from the user names, so webhook secrets and
        // rate-limit budgets stay per tenant.
        let owners: Vec<String> = app_state
            .states_with_owners()
            .into_iter()
            .map(|(owner, _)| owner)
            .collect();
        assert_eq!(owners, vec!["alice".to_string(), "bob".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_app_state_single_all_states() {
        let state = Arc::new(Mutex::new(ServerState::new()));
        let app_state = AppState::single(state);
        assert_eq!(app_state.all_states().len(), 1);
        let owners: Vec<String> = app_state
            .states_with_owners()
            .into_iter()
            .map(|(owner, _)| owner)
            .collect();
        assert_eq!(owners, vec![DEFAULT_CHANNEL_OWNER.to_string()]);
    }
}
