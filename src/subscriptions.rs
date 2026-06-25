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
    let token = crate::auth::get_token(
        scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
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
    let token = crate::auth::get_token(
        scopes,
        policy.credentials_file.as_deref(),
        Some(token_cache),
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
            .notify_resource_updated(ResourceUpdatedNotificationParam {
                uri: sub.uri.clone(),
            })
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
                    s.expiration.saturating_duration_since(now) < Duration::from_secs(600)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_map() {
        let map: SubscriptionMap = HashMap::new();
        assert!(map.is_empty());
    }
}
