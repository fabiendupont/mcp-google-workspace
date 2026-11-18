use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rmcp::model::ResourceUpdatedNotificationParam;
use rmcp::service::{Peer, RoleServer};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use google_workspace::error::GwsError;

/// Owner key used in single-user mode and by the legacy un-keyed helpers.
///
/// Multi-user mode uses the user's name (see [`crate::users::UserInstance`])
/// so tenants can never see, verify, or evict each other's channel secrets.
pub const DEFAULT_CHANNEL_OWNER: &str = "default";

/// Maximum number of channel secrets retained per owner.
///
/// The cap is enforced per owner: a tenant creating watches can only evict its
/// own oldest entry, never another tenant's live channel. Leaving this purely
/// global let one tenant push another tenant's secret out of the registry, so
/// that tenant's genuine Drive notifications were rejected with 403 for up to
/// [`CHANNEL_SECRET_TTL`].
const MAX_CHANNEL_SECRETS_PER_OWNER: usize = 256;

/// Maximum number of channel secrets retained in the process-wide registry.
/// Channels are removed when unsubscribed; this cap only remains as a backstop
/// against leaked watches that were never stopped.
const MAX_CHANNEL_SECRETS: usize = 4096;

/// Maximum lifetime of a registry entry. Drive push channels expire within a
/// day, so anything older is stale.
const CHANNEL_SECRET_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Per-channel webhook shared secret. Drive echoes it back in the
/// `X-Goog-Channel-Token` header, which lets us reject forged notifications.
struct ChannelSecret {
    token: String,
    created: Instant,
}

/// Registry key: `(owner, channel_id)`. The owner is the single-user sentinel
/// or a multi-user instance name.
type ChannelSecretKey = (String, String);

type ChannelSecretMap = HashMap<ChannelSecretKey, ChannelSecret>;

fn channel_secrets() -> &'static Mutex<ChannelSecretMap> {
    static REGISTRY: OnceLock<Mutex<ChannelSecretMap>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channel_secret_key(owner: &str, channel_id: &str) -> ChannelSecretKey {
    (owner.to_string(), channel_id.to_string())
}

pub struct Subscription {
    pub uri: String,
    pub channel_id: String,
    pub resource_id: String,
    pub expiration: Instant,
    pub peer: Peer<RoleServer>,
    pub service: String,
    /// Polling subscriptions watch the resource by re-reading it
    /// periodically instead of relying on Google push channels.
    pub polling: bool,
}

pub type SubscriptionMap = HashMap<String, Subscription>;

/// Generate an unpredictable hex string.
///
/// `RandomState` is seeded from OS entropy (and mixed with a per-process
/// counter), so the output cannot be predicted from the timestamp the way the
/// old FNV-of-milliseconds channel IDs could. No extra dependency needed.
fn random_hex(bytes: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut out = String::with_capacity(bytes * 2);
    while out.len() < bytes * 2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.truncate(bytes * 2);
    out
}

/// Unpredictable Drive channel ID.
pub fn new_channel_id() -> String {
    format!("gws-{}", random_hex(16))
}

/// Unpredictable webhook token for a Drive channel.
pub fn new_channel_token() -> String {
    random_hex(24)
}

/// Constant-time string comparison. Only the length is allowed to leak.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Remember the secret associated with a channel so webhook callbacks can be
/// authenticated. Called by [`watch_resource`] once Google accepts the watch.
///
/// Registration is scoped to `owner` (the single-user sentinel or the
/// multi-user instance name). The per-owner cap evicts only that owner's
/// oldest entry; the global cap is a last-resort backstop.
pub async fn register_channel_secret_for(owner: &str, channel_id: &str, token: &str) {
    let now = Instant::now();
    let mut secrets = channel_secrets().lock().await;

    // Drop stale entries before applying the size caps.
    secrets.retain(|_, secret| now.duration_since(secret.created) < CHANNEL_SECRET_TTL);

    let key = channel_secret_key(owner, channel_id);

    // Per-owner cap: evict only the oldest entry belonging to *this* owner.
    // Cross-tenant eviction was the original vulnerability.
    if !secrets.contains_key(&key) {
        let oldest_for_owner = secrets
            .iter()
            .filter(|((entry_owner, _), _)| entry_owner == owner)
            .map(|(entry_key, secret)| (entry_key.clone(), secret.created))
            .min_by_key(|(_, created)| *created);
        let owned_count = secrets.keys().filter(|(o, _)| o == owner).count();
        if owned_count >= MAX_CHANNEL_SECRETS_PER_OWNER
            && let Some((oldest, _)) = oldest_for_owner
        {
            secrets.remove(&oldest);
        }
    }

    // Global backstop for leaked watches that were never stopped.
    if secrets.len() >= MAX_CHANNEL_SECRETS
        && let Some(oldest) = secrets
            .iter()
            .min_by_key(|(_, secret)| secret.created)
            .map(|(key, _)| key.clone())
    {
        secrets.remove(&oldest);
    }

    secrets.insert(
        key,
        ChannelSecret {
            token: token.to_string(),
            created: now,
        },
    );
}

/// Forget the secret for `(owner, channel_id)`. Idempotent.
pub async fn unregister_channel_secret_for(owner: &str, channel_id: &str) {
    channel_secrets()
        .lock()
        .await
        .remove(&channel_secret_key(owner, channel_id));
}

/// Verify a Drive webhook against the secret stored for `(owner, channel_id)`.
///
/// Returns `false` for unknown channels and for token mismatches, so callers
/// must distinguish "unknown channel" from "bad token" separately (see
/// [`channel_owned`]) if they want different status codes.
pub async fn verify_channel_for(owner: &str, channel_id: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let secrets = channel_secrets().lock().await;
    secrets
        .get(&channel_secret_key(owner, channel_id))
        .is_some_and(|secret| constant_time_eq(token, &secret.token))
}

/// Legacy un-keyed registration: single-user/default owner.
pub async fn register_channel_secret(channel_id: &str, token: &str) {
    register_channel_secret_for(DEFAULT_CHANNEL_OWNER, channel_id, token).await;
}

/// Legacy un-keyed unregistration: single-user/default owner.
pub async fn unregister_channel_secret(channel_id: &str) {
    unregister_channel_secret_for(DEFAULT_CHANNEL_OWNER, channel_id).await;
}

/// Verify a webhook against the default owner's secret.
///
/// Multi-user callers must use [`verify_channel_for`] with the owner of the
/// matched channel; this wrapper only exists for single-user mode and tests.
pub async fn verify_channel_token(channel_id: &str, token: &str) -> bool {
    verify_channel_for(DEFAULT_CHANNEL_OWNER, channel_id, token).await
}

/// Whether a state's subscription map holds a subscription for `channel_id`.
pub async fn channel_owned(subscriptions: &Arc<Mutex<SubscriptionMap>>, channel_id: &str) -> bool {
    subscriptions
        .lock()
        .await
        .values()
        .any(|sub| sub.channel_id == channel_id)
}

/// Verify a webhook token against the matching state's subscription map.
///
/// Both the channel (must be an active subscription in this state) and the
/// token (must match the default owner's stored secret) are checked. Multi-user
/// callers should combine [`channel_owned`] with [`verify_channel_for`] so the
/// secret is checked against the owner of the matched channel.
pub async fn verify_channel(
    subscriptions: &Arc<Mutex<SubscriptionMap>>,
    channel_id: &str,
    token: &str,
) -> bool {
    if !channel_owned(subscriptions, channel_id).await {
        return false;
    }
    verify_channel_token(channel_id, token).await
}

pub async fn watch_resource(
    owner: &str,
    _service: &str,
    resource_id: &str,
    webhook_url: &str,
    token_cache: &mut Option<crate::auth::TokenCache>,
    policy: &crate::policy::Policy,
) -> Result<(String, String, u64), GwsError> {
    let channel_id = new_channel_id();
    let channel_token = new_channel_token();

    let scopes = &["https://www.googleapis.com/auth/drive.readonly"];
    let token = crate::auth::get_token_as(
        scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
        policy.delegate_subject.as_deref(),
    )
    .await
    .map_err(|e| GwsError::Auth(format!("Authentication failed: {e}")))?;

    let watch_url = format!(
        "https://www.googleapis.com/drive/v3/files/{}/watch",
        resource_id
    );

    let client = google_workspace::client::shared_client()?;

    // Register the secret *before* the watch exists: Drive sends its first
    // (sync) notification immediately after creating the channel, possibly
    // before this function returns. It is removed again if the watch fails.
    register_channel_secret_for(owner, &channel_id, &channel_token).await;

    let response = match client
        .post(&watch_url)
        .bearer_auth(&token)
        .json(&json!({
            "id": channel_id,
            "type": "web_hook",
            "address": format!("{webhook_url}/webhooks/drive"),
            "token": channel_token,
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            unregister_channel_secret_for(owner, &channel_id).await;
            return Err(GwsError::Other(anyhow::anyhow!(
                "Watch request failed: {e}"
            )));
        }
    };

    let status = response.status();
    let body: Value = response.json().await.unwrap_or(json!({}));

    if !status.is_success() {
        unregister_channel_secret_for(owner, &channel_id).await;
        let msg = body["error"]["message"].as_str().unwrap_or("Unknown error");
        return Err(GwsError::Validation(format!(
            "Drive watch failed ({status}): {msg}"
        )));
    }

    let api_resource_id = body["resourceId"]
        .as_str()
        .unwrap_or(resource_id)
        .to_string();
    let expiration_ms = body["expiration"].as_u64().unwrap_or(0);

    Ok((channel_id, api_resource_id, expiration_ms))
}

/// Stop a Drive watch channel and forget its webhook secret.
///
/// The secret is dropped on **every** exit path (success, transport error,
/// non-2xx response, authentication failure) because the caller is abandoning
/// the local subscription either way. Returning early without unregistering
/// leaked the secret until its TTL expired.
pub async fn stop_watch(
    owner: &str,
    channel_id: &str,
    resource_id: &str,
    token_cache: &mut Option<crate::auth::TokenCache>,
    policy: &crate::policy::Policy,
) -> Result<(), GwsError> {
    let result = stop_watch_inner(channel_id, resource_id, token_cache, policy).await;
    unregister_channel_secret_for(owner, channel_id).await;
    result
}

async fn stop_watch_inner(
    channel_id: &str,
    resource_id: &str,
    token_cache: &mut Option<crate::auth::TokenCache>,
    policy: &crate::policy::Policy,
) -> Result<(), GwsError> {
    let scopes = &["https://www.googleapis.com/auth/drive.readonly"];
    let token = crate::auth::get_token_as(
        scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
        policy.delegate_subject.as_deref(),
    )
    .await
    .map_err(|e| GwsError::Auth(format!("Authentication failed: {e}")))?;

    let client = google_workspace::client::shared_client()?;
    let response = client
        .post("https://www.googleapis.com/drive/v3/channels/stop")
        .bearer_auth(&token)
        .json(&json!({
            "id": channel_id,
            "resourceId": resource_id,
        }))
        .send()
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Stop watch failed: {e}")))?;

    if !response.status().is_success() {
        tracing::warn!(
            channel_id = channel_id,
            status = %response.status(),
            "Failed to stop watch channel (may have already expired)"
        );
    }

    Ok(())
}

pub async fn handle_webhook(
    channel_id: &str,
    resource_state: &str,
    subscriptions: &Arc<Mutex<SubscriptionMap>>,
) {
    if resource_state == "sync" {
        tracing::debug!(channel_id = channel_id, "Watch channel sync confirmation");
        return;
    }

    let subs = subscriptions.lock().await;
    let sub = subs.values().find(|s| s.channel_id == channel_id);

    if let Some(sub) = sub {
        tracing::info!(
            uri = %sub.uri,
            channel_id = channel_id,
            state = resource_state,
            "Resource changed, notifying client"
        );
        let _ = sub
            .peer
            .notify_resource_updated(ResourceUpdatedNotificationParam::new(sub.uri.clone()))
            .await;
    } else {
        tracing::warn!(
            channel_id = channel_id,
            "Webhook for unknown channel (may have been unsubscribed)"
        );
    }
}

pub fn spawn_renewal_task(
    subscriptions: Arc<Mutex<SubscriptionMap>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            let subs = subscriptions.lock().await;
            let now = Instant::now();
            let expiring: Vec<String> = subs
                .iter()
                .filter(|(_, s)| {
                    !s.polling
                        && s.expiration.saturating_duration_since(now) < Duration::from_secs(600)
                })
                .map(|(uri, _)| uri.clone())
                .collect();
            drop(subs);

            for uri in expiring {
                tracing::info!(uri = %uri, "Subscription expiring soon, client should re-subscribe");
            }
        }
    })
}

/// Configuration for a polling subscription.
#[derive(Debug, Clone)]
pub struct PollConfig {
    pub service: String,
    pub resource: String,
    pub id: String,
    pub interval: Duration,
}

/// Stable hash of a resource snapshot, used for change detection.
pub fn content_hash(value: &Value) -> u64 {
    crate::execute::simple_hash(value.to_string().as_bytes())
}

/// Read the current state of a resource for change detection.
async fn poll_read(
    state: &Arc<Mutex<crate::shared::ServerState>>,
    policy: &crate::policy::Policy,
    cfg: &PollConfig,
) -> Result<Value, GwsError> {
    let (mut tc, doc) = {
        let mut st = state.lock().await;
        let tc = st.token_cache.take();
        let doc = st.get_doc(&cfg.service).await?;
        (tc, doc)
    };

    let result = poll_read_inner(&doc, &mut tc, policy, cfg).await;

    // Restore the token cache regardless of the outcome
    let mut st = state.lock().await;
    st.token_cache = tc;
    result
}

async fn poll_read_inner(
    doc: &Arc<google_workspace::discovery::RestDescription>,
    tc: &mut Option<crate::auth::TokenCache>,
    policy: &crate::policy::Policy,
    cfg: &PollConfig,
) -> Result<Value, GwsError> {
    let resource = crate::tools::find_resource(&doc.resources, &cfg.resource).ok_or_else(|| {
        GwsError::Validation(format!(
            "Resource '{}' not found in {}",
            cfg.resource, cfg.service
        ))
    })?;
    let method = resource.methods.get("get").ok_or_else(|| {
        GwsError::Validation(format!("Resource '{}' has no get method", cfg.resource))
    })?;
    let id_param = crate::resources::id_param_name(resource, "get").ok_or_else(|| {
        GwsError::Validation(format!(
            "Resource '{}' has no get method with path parameter",
            cfg.resource
        ))
    })?;

    let args = json!({ "params": { &id_param: &cfg.id } });
    crate::execute::execute_tool(
        doc,
        method,
        &cfg.resource,
        "get",
        &args,
        &cfg.service,
        policy,
        &crate::meta::RequestMeta::default(),
        None,
        None,
        false,
        tc,
    )
    .await
}

/// Spawn a background task that polls a resource and emits
/// `notifications/resources/updated` whenever its content changes.
/// The task exits automatically once the subscription is removed.
pub fn spawn_poll_task(
    uri: String,
    cfg: PollConfig,
    state: Arc<Mutex<crate::shared::ServerState>>,
    policy: Arc<tokio::sync::RwLock<crate::policy::Policy>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            uri = %uri,
            service = %cfg.service,
            resource = %cfg.resource,
            interval_secs = cfg.interval.as_secs(),
            "Polling subscription started"
        );
        let mut last_hash: Option<u64> = None;
        let mut ticker = tokio::time::interval(cfg.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            let still_subscribed = {
                let st = state.lock().await;
                st.subscriptions.lock().await.contains_key(&uri)
            };
            if !still_subscribed {
                tracing::info!(uri = %uri, "Polling subscription removed, stopping");
                break;
            }

            let policy = policy.read().await;
            let result = poll_read(&state, &policy, &cfg).await;
            drop(policy);

            match result {
                Ok(value) => {
                    let hash = content_hash(&value);
                    if let Some(prev) = last_hash
                        && prev != hash
                    {
                        tracing::info!(uri = %uri, "Resource changed, notifying client");
                        let peer = {
                            let st = state.lock().await;
                            st.subscriptions
                                .lock()
                                .await
                                .get(&uri)
                                .map(|s| s.peer.clone())
                        };
                        if let Some(peer) = peer {
                            let _ = peer
                                .notify_resource_updated(ResourceUpdatedNotificationParam::new(
                                    uri.clone(),
                                ))
                                .await;
                        } else {
                            // Unsubscribed while notifying
                            break;
                        }
                    }
                    last_hash = Some(hash);
                }
                Err(e) => {
                    tracing::warn!(uri = %uri, error = %e, "Polling read failed");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_map() {
        let map: SubscriptionMap = HashMap::new();
        assert!(map.is_empty());
    }

    #[test]
    fn test_content_hash_stable() {
        let a = json!({ "id": "x", "title": "t" });
        assert_eq!(content_hash(&a), content_hash(&a));
    }

    #[test]
    fn test_content_hash_detects_change() {
        let a = json!({ "id": "x", "title": "t" });
        let b = json!({ "id": "x", "title": "changed" });
        assert_ne!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn test_content_hash_key_order_insensitive() {
        // serde_json::Value uses ordered maps from JSON object syntax,
        // so logically-equal objects constructed differently hash the same
        let mut a = serde_json::Map::new();
        a.insert("x".to_string(), json!(1));
        a.insert("y".to_string(), json!(2));
        let mut b = serde_json::Map::new();
        b.insert("y".to_string(), json!(2));
        b.insert("x".to_string(), json!(1));
        assert_eq!(
            content_hash(&Value::Object(a)),
            content_hash(&Value::Object(b))
        );
    }

    #[test]
    fn test_channel_ids_are_unique_and_random_hex() {
        let a = new_channel_id();
        let b = new_channel_id();
        assert_ne!(a, b, "channel IDs must not collide or be time-derived");
        assert!(a.starts_with("gws-"));
        // 16 OS-seeded bytes rendered as hex, plus the prefix. The
        // authentication secret is a separate 48-hex-character value; the id
        // only needs to be unguessable enough not to collide across tenants.
        assert_eq!(a.len(), 4 + 32);
        assert!(a[4..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(new_channel_token(), new_channel_token());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret "));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }

    #[tokio::test]
    async fn test_verify_channel_token_correct_token_accepted() {
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret(&channel_id, &token).await;
        assert!(verify_channel_token(&channel_id, &token).await);
        unregister_channel_secret(&channel_id).await;
    }

    #[tokio::test]
    async fn test_verify_channel_token_wrong_token_rejected() {
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret(&channel_id, &token).await;
        assert!(!verify_channel_token(&channel_id, "not-the-token").await);
        assert!(!verify_channel_token(&channel_id, "").await);
        unregister_channel_secret(&channel_id).await;
    }

    #[tokio::test]
    async fn test_verify_channel_token_unknown_channel_rejected() {
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret(&channel_id, &token).await;
        unregister_channel_secret(&channel_id).await;
        // The secret is gone once the channel is unsubscribed.
        assert!(!verify_channel_token(&channel_id, &token).await);
        assert!(!verify_channel_token("gws-does-not-exist", &token).await);
    }

    #[tokio::test]
    async fn test_verify_channel_requires_active_subscription() {
        // An empty map owns no channel, so verification always fails even for
        // a channel whose secret is still registered.
        let subs: Arc<Mutex<SubscriptionMap>> = Arc::new(Mutex::new(HashMap::new()));
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret(&channel_id, &token).await;
        assert!(!verify_channel(&subs, &channel_id, &token).await);
        assert!(!channel_owned(&subs, &channel_id).await);
        unregister_channel_secret(&channel_id).await;
    }

    #[tokio::test]
    async fn test_owner_scoped_token_accepted() {
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret_for("tenant-a", &channel_id, &token).await;
        assert!(verify_channel_for("tenant-a", &channel_id, &token).await);
        // Empty/absent tokens never verify.
        assert!(!verify_channel_for("tenant-a", &channel_id, "").await);
        unregister_channel_secret_for("tenant-a", &channel_id).await;
    }

    #[tokio::test]
    async fn test_wrong_owner_token_rejected() {
        let channel_id = new_channel_id();
        let token_a = new_channel_token();
        let token_b = new_channel_token();
        // Same channel id registered independently by two tenants.
        register_channel_secret_for("tenant-a", &channel_id, &token_a).await;
        register_channel_secret_for("tenant-b", &channel_id, &token_b).await;

        assert!(verify_channel_for("tenant-a", &channel_id, &token_a).await);
        assert!(!verify_channel_for("tenant-a", &channel_id, &token_b).await);
        assert!(verify_channel_for("tenant-b", &channel_id, &token_b).await);
        assert!(!verify_channel_for("tenant-b", &channel_id, &token_a).await);

        unregister_channel_secret_for("tenant-a", &channel_id).await;
        unregister_channel_secret_for("tenant-b", &channel_id).await;
    }

    #[tokio::test]
    async fn test_cross_owner_channel_id_rejected() {
        // A channel registered by one tenant must never be verifiable by
        // another tenant, even with the correct token.
        let channel_id = new_channel_id();
        let token = new_channel_token();
        register_channel_secret_for("tenant-a", &channel_id, &token).await;

        assert!(verify_channel_for("tenant-a", &channel_id, &token).await);
        assert!(!verify_channel_for("tenant-b", &channel_id, &token).await);
        assert!(!verify_channel_for(DEFAULT_CHANNEL_OWNER, &channel_id, &token).await);

        unregister_channel_secret_for("tenant-a", &channel_id).await;
        assert!(!verify_channel_for("tenant-a", &channel_id, &token).await);
    }

    #[tokio::test]
    async fn test_unregister_is_owner_scoped() {
        let channel_id = new_channel_id();
        let token_a = new_channel_token();
        let token_b = new_channel_token();
        register_channel_secret_for("tenant-a", &channel_id, &token_a).await;
        register_channel_secret_for("tenant-b", &channel_id, &token_b).await;

        unregister_channel_secret_for("tenant-a", &channel_id).await;
        assert!(!verify_channel_for("tenant-a", &channel_id, &token_a).await);
        assert!(verify_channel_for("tenant-b", &channel_id, &token_b).await);

        unregister_channel_secret_for("tenant-b", &channel_id).await;
    }

    #[tokio::test]
    async fn test_per_owner_eviction_does_not_touch_another_owner() {
        let owner = "eviction-owner-a";
        let other = "eviction-owner-b";
        let now = Instant::now();

        // Another tenant's live channel, registered before the flood below so
        // it is globally the oldest entry.
        register_channel_secret_for(other, "gws-other-tenant", "other-token").await;

        {
            let mut secrets = channel_secrets().lock().await;
            for i in 0..MAX_CHANNEL_SECRETS_PER_OWNER {
                secrets.insert(
                    channel_secret_key(owner, &format!("gws-cap-{i}")),
                    ChannelSecret {
                        token: format!("token-{i}"),
                        created: now + Duration::from_millis(i as u64),
                    },
                );
            }
        }

        // One more registration evicts only the oldest entry of *this* owner.
        register_channel_secret_for(owner, "gws-cap-new", "new-token").await;

        assert!(
            !verify_channel_for(owner, "gws-cap-0", "token-0").await,
            "the owner's oldest entry must be evicted"
        );
        assert!(verify_channel_for(owner, "gws-cap-1", "token-1").await);
        assert!(verify_channel_for(owner, "gws-cap-new", "new-token").await);
        assert!(
            verify_channel_for(other, "gws-other-tenant", "other-token").await,
            "another owner's secret must survive the eviction"
        );

        unregister_channel_secret_for(other, "gws-other-tenant").await;
        {
            let mut secrets = channel_secrets().lock().await;
            secrets.retain(|(entry_owner, _), _| entry_owner != owner);
        }
    }
}
