use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::ResourceUpdatedNotificationParam;
use rmcp::service::{Peer, RoleServer};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use google_workspace::error::GwsError;

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

pub async fn watch_resource(
    service: &str,
    resource_id: &str,
    webhook_url: &str,
    token_cache: &mut Option<crate::auth::TokenCache>,
    policy: &crate::policy::Policy,
) -> Result<(String, String, u64), GwsError> {
    let channel_id = format!(
        "gws-{:016x}",
        crate::execute::simple_hash(
            format!(
                "{service}/{resource_id}/{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            )
            .as_bytes()
        )
    );

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
    let response = client
        .post(&watch_url)
        .bearer_auth(&token)
        .json(&json!({
            "id": channel_id,
            "type": "web_hook",
            "address": format!("{webhook_url}/webhooks/drive"),
        }))
        .send()
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Watch request failed: {e}")))?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or(json!({}));

    if !status.is_success() {
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

pub async fn stop_watch(
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
}
