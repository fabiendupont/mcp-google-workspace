use std::sync::Arc;

use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::task_manager::TaskManager;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::policy::Policy;
use crate::shared::ServerState;

/// Gate for Drive push-notification channels (`resources/subscribe` →
/// `files.watch`, `resources/unsubscribe` → `channels.stop`).
///
/// Both operations POST to Drive, so they must pass the same policy checks as
/// any other Drive write — the URI's service alone is not enough.
pub(crate) fn check_drive_watch_allowed(
    policy: &Policy,
    service: &str,
    resource: &str,
) -> Result<(), String> {
    if service != "drive" {
        return Err(format!(
            "resources/subscribe only supports Drive push notifications (gws://drive/...); \
             got service '{service}'. Fix: subscribe to a gws://drive/<resource>/<id> URI"
        ));
    }
    if !policy.is_service_allowed("drive") {
        return Err("Service 'drive' is not enabled by policy. \
             Fix: add {\"name\": \"drive\"} to the \"services\" array in your policy file"
            .to_string());
    }
    if policy.is_read_only("drive") {
        return Err(
            "Service 'drive' is read-only; subscribing to changes requires the Drive write \
             files.watch, which is not allowed. \
             Fix: set \"read_only\": false on the \"drive\" service in your policy file"
                .to_string(),
        );
    }
    if !crate::resources::is_resource_allowed(policy.allowed_resources("drive"), resource) {
        return Err(format!(
            "Resource '{resource}' is not in allowed_resources for service 'drive'. \
             Fix: add \"{resource}\" to the \"allowed_resources\" array in your policy file"
        ));
    }
    Ok(())
}

/// Run `policy.check_method` against the real discovery method for a Drive
/// watch/stop call.
async fn check_drive_method_allowed(
    state: &Arc<Mutex<ServerState>>,
    policy: &Policy,
    resource: &str,
    method_name: &str,
) -> Result<(), String> {
    let mut st = state.lock().await;
    let doc = st
        .get_doc("drive")
        .await
        .map_err(|e| format!("Failed to load drive discovery doc: {e}"))?;
    let drive_resource = crate::tools::find_resource(&doc.resources, resource)
        .ok_or_else(|| format!("Resource '{resource}' not found in drive"))?;
    let method = drive_resource.methods.get(method_name).ok_or_else(|| {
        format!("Method 'drive.{resource}.{method_name}' not found in discovery document")
    })?;
    policy
        .check_method("drive", resource, method_name, method)
        .map_err(|e| e.to_string())
}

pub struct GwsHandler {
    pub(crate) state: Arc<Mutex<ServerState>>,
    pub(crate) policy: Arc<RwLock<Policy>>,
    pub(crate) task_manager: TaskManager,
    /// Identity that owns this handler's webhook channel secrets.
    ///
    /// Single-user (stdio) mode uses
    /// [`crate::subscriptions::DEFAULT_CHANNEL_OWNER`]; multi-user HTTP mode
    /// passes the user name so one tenant cannot register, verify, or evict
    /// another tenant's channel secrets.
    pub(crate) owner: String,
}

impl GwsHandler {
    pub fn new(
        policy: Policy,
        prompts: Vec<crate::prompts::Prompt>,
        audit: Option<Arc<crate::audit::AuditLogger>>,
        eager_tools: bool,
    ) -> Self {
        let mut state = ServerState::new();
        state.prompts = prompts;
        state.audit = audit;
        state.eager_tools = eager_tools;
        Self {
            state: Arc::new(Mutex::new(state)),
            policy: Arc::new(RwLock::new(policy)),
            task_manager: TaskManager::new(),
            owner: crate::subscriptions::DEFAULT_CHANNEL_OWNER.to_string(),
        }
    }

    /// Single-user/default-owner handler.
    pub fn from_shared(state: Arc<Mutex<ServerState>>, policy: Arc<RwLock<Policy>>) -> Self {
        Self::from_shared_for(state, policy, crate::subscriptions::DEFAULT_CHANNEL_OWNER)
    }

    /// Handler bound to an explicit owner (multi-user mode passes the user
    /// name). The owner scopes webhook channel secrets.
    pub fn from_shared_for(
        state: Arc<Mutex<ServerState>>,
        policy: Arc<RwLock<Policy>>,
        owner: &str,
    ) -> Self {
        Self {
            state,
            policy,
            task_manager: TaskManager::new(),
            owner: owner.to_string(),
        }
    }
}

impl ServerHandler for GwsHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_tasks()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_completions()
                .build(),
        )
        .with_server_info(Implementation::new(
            "mcp-google-workspace",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(crate::server::server_instructions().to_string())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            let policy = self.policy.read().await;
            let mut st = self.state.lock().await;
            if st.tools.is_none() {
                let activated = st.activated_services.clone();
                let eager = st.eager_tools;
                let tools =
                    crate::tools::build_tools_list(&policy, &mut st.docs, Some(&activated), eager)
                        .await
                        .map_err(|e| {
                            McpError::internal_error(format!("Failed to build tools: {e}"), None)
                        })?;
                st.tools = Some(tools);
            }
            let tools = st.tools.as_ref().unwrap().clone();
            Ok(ListToolsResult {
                result_type: Some(ResultType::COMPLETE),
                meta: None,
                next_cursor: None,
                ttl_ms: None,
                cache_scope: None,
                tools,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, McpError>> + Send + '_ {
        let peer = context.peer.clone();
        let progress_token = context.meta.get_progress_token();
        let request_meta = crate::meta::RequestMeta::from_rmcp_meta(&context.meta);
        async move {
            let tool_name = request.name.to_string();
            let arguments = match request.arguments {
                Some(map) => Value::Object(map),
                None => json!({}),
            };

            let params = json!({
                "name": tool_name,
                "arguments": arguments,
            });
            let policy = self.policy.read().await;

            let start = std::time::Instant::now();
            let result = crate::server::handle_tool_call_concurrent(
                &params,
                &request_meta,
                &policy,
                &self.state,
                Some(&peer),
                progress_token.as_ref(),
            )
            .await;

            let is_err = result.is_err();
            crate::metrics::record_request("tools/call", is_err, start.elapsed().as_secs_f64());
            let tools_invalidated = {
                let st = self.state.lock().await;
                crate::metrics::set_active_tasks(st.tasks.len() as i64);
                st.tools.is_none() && !st.eager_tools
            };
            if tools_invalidated {
                let _ = peer
                    .send_notification(
                        rmcp::model::ServerNotification::ToolListChangedNotification(
                            rmcp::model::NotificationNoParam::default(),
                        ),
                    )
                    .await;
                tracing::info!("Sent tools/list_changed notification");
            }

            match result {
                Ok(ref value) => {
                    let is_tool_error = value
                        .get("isError")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if is_tool_error {
                        let err_text = value
                            .get("content")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first())
                            .and_then(|e| e.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown error");
                        tracing::warn!(tool = %tool_name, error = %err_text, duration_ms = start.elapsed().as_millis() as u64, "Tool error");
                    } else {
                        tracing::info!(tool = %tool_name, duration_ms = start.elapsed().as_millis() as u64, "Tool ok");
                    }
                    Ok(value_to_call_tool_result(result.unwrap()).into())
                }
                Err(e) => {
                    tracing::warn!(tool = %tool_name, error = %e, duration_ms = start.elapsed().as_millis() as u64, "Tool failed");
                    Ok(CallToolResult::error(vec![ContentBlock::text(e.to_string())]).into())
                }
            }
        }
    }

    fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CompleteResult, McpError>> + Send + '_ {
        async move {
            let policy = self.policy.read().await;
            let st = self.state.lock().await;
            let completion = crate::completions::complete_request(
                &request.r#ref,
                &request.argument,
                &policy,
                &st.docs,
                &st.prompts,
            );
            Ok(CompleteResult::new(completion))
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        async move { Ok(ListResourcesResult::with_all_items(vec![])) }
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_
    {
        async move {
            let policy = self.policy.read().await;
            let mut st = self.state.lock().await;
            // The discovery documents are loaded lazily. If `resources/templates/list`
            // is the very first request, `st.docs` is empty and the client would see
            // zero templates; load the allowed services first so the advertised
            // surface is available from the first call.
            for service in policy.allowed_services() {
                if let Err(e) = st.get_doc(service).await {
                    tracing::warn!(
                        service,
                        error = %e,
                        "could not load discovery document for resource templates"
                    );
                }
            }
            let templates = crate::resources::build_resource_templates(&policy, &st.docs);
            Ok(ListResourceTemplatesResult::with_all_items(templates))
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResponse, McpError>> + Send + '_ {
        async move {
            let parsed = crate::resources::parse_gws_uri(&request.uri).ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "Invalid resource URI: {}. Expected gws://service/resource/id",
                        request.uri
                    ),
                    None,
                )
            })?;

            let policy = self.policy.read().await;
            if !policy.is_service_allowed(&parsed.service) {
                return Err(McpError::invalid_params(
                    format!("Service '{}' not enabled by policy", parsed.service),
                    None,
                ));
            }

            let allowed = policy.allowed_resources(&parsed.service);
            if !crate::resources::is_resource_allowed(allowed, &parsed.resource) {
                return Err(McpError::invalid_params(
                    format!(
                        "Resource '{}' is not in allowed_resources for service '{}'. \
                         Fix: add \"{}\" to the \"allowed_resources\" array in your policy file",
                        parsed.resource, parsed.service, parsed.resource
                    ),
                    None,
                ));
            }

            let meta = crate::meta::RequestMeta::default();
            let mut st = self.state.lock().await;
            let doc = st.get_doc(&parsed.service).await.map_err(|e| {
                McpError::internal_error(format!("Failed to load discovery doc: {e}"), None)
            })?;

            let resource = crate::tools::find_resource(&doc.resources, &parsed.resource)
                .ok_or_else(|| {
                    McpError::invalid_params(
                        format!(
                            "Resource '{}' not found in {}",
                            parsed.resource, parsed.service
                        ),
                        None,
                    )
                })?;

            let id_param = crate::resources::id_param_name(resource, "get").ok_or_else(|| {
                McpError::invalid_params(
                    format!(
                        "Resource '{}' has no get method with path parameter",
                        parsed.resource
                    ),
                    None,
                )
            })?;

            let method = resource.methods.get("get").ok_or_else(|| {
                McpError::invalid_params(
                    format!("Resource '{}' has no get method", parsed.resource),
                    None,
                )
            })?;

            let args = json!({
                "params": crate::resources::build_get_params(method, &id_param, &parsed.id)
            });
            let result = crate::execute::execute_tool(
                &doc,
                method,
                &parsed.resource,
                "get",
                &args,
                &parsed.service,
                &policy,
                &meta,
                None,
                None,
                false,
                &mut st.token_cache,
            )
            .await
            .map_err(|e| McpError::internal_error(format!("API call failed: {e}"), None))?;

            if parsed.service == "gmail" {
                let allowed_labels = policy.allowed_labels("gmail");
                if !allowed_labels.is_empty() {
                    let label_ids = crate::gmail_helpers::extract_label_ids(&result);
                    let label_doc = st.get_doc("gmail").await.map_err(|e| {
                        McpError::internal_error(format!("Failed to load gmail doc: {e}"), None)
                    })?;
                    let labels_resource =
                        crate::tools::find_resource(&label_doc.resources, "users.labels")
                            .ok_or_else(|| {
                                McpError::internal_error("users.labels not found", None)
                            })?;
                    let list_method = labels_resource
                        .methods
                        .get("list")
                        .ok_or_else(|| McpError::internal_error("labels.list not found", None))?;
                    // Fail closed: a configured label policy that cannot be
                    // resolved must not silently disable label enforcement.
                    let labels_result = match crate::execute::execute_tool(
                        &label_doc,
                        list_method,
                        "users.labels",
                        "list",
                        &json!({ "params": { "userId": "me" } }),
                        "gmail",
                        &policy,
                        &meta,
                        None,
                        None,
                        false,
                        &mut st.token_cache,
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to list Gmail labels");
                            return Err(McpError::invalid_params(
                                format!(
                                    "Gmail label policy configured but none of the allowed labels resolved: {}",
                                    allowed_labels.join(", ")
                                ),
                                None,
                            ));
                        }
                    };
                    let mut allowed_ids = Vec::new();
                    for name in allowed_labels {
                        if let Some(id) =
                            crate::gmail_helpers::resolve_label_id(name, &labels_result)
                        {
                            allowed_ids.push(id);
                        }
                    }
                    if allowed_ids.is_empty() {
                        return Err(McpError::invalid_params(
                            format!(
                                "Gmail label policy configured but none of the allowed labels resolved: {}",
                                allowed_labels.join(", ")
                            ),
                            None,
                        ));
                    }
                    if let Err(msg) =
                        crate::gmail_helpers::check_label_policy(&label_ids, &allowed_ids)
                    {
                        return Err(McpError::invalid_params(msg, None));
                    }
                }
            }

            let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
            let contents = rmcp::model::ResourceContents::text(&text, &request.uri)
                .with_mime_type("application/json");
            Ok(ReadResourceResult::new(vec![contents]).into())
        }
    }

    fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        let peer = context.peer.clone();
        async move {
            let parsed = crate::resources::parse_gws_uri(&request.uri).ok_or_else(|| {
                McpError::invalid_params(format!("Invalid resource URI: {}", request.uri), None)
            })?;

            let policy = self.policy.read().await;
            // `files.watch` is a Drive write, whatever the URI says.
            check_drive_watch_allowed(&policy, &parsed.service, &parsed.resource)
                .map_err(|msg| McpError::invalid_params(msg, None))?;
            check_drive_method_allowed(&self.state, &policy, &parsed.resource, "watch")
                .await
                .map_err(|msg| McpError::invalid_params(msg, None))?;

            let webhook_url = {
                let st = self.state.lock().await;
                st.webhook_url.clone()
            };

            match webhook_url {
                Some(webhook_url) => {
                    let (channel_id, resource_id, expiration_ms) =
                        crate::subscriptions::watch_resource(
                            &self.owner,
                            &parsed.service,
                            &parsed.id,
                            &webhook_url,
                            &mut self.state.lock().await.token_cache,
                            &policy,
                        )
                        .await
                        .map_err(|e| {
                            McpError::internal_error(format!("Failed to watch resource: {e}"), None)
                        })?;

                    let expiration = std::time::Instant::now()
                        + std::time::Duration::from_millis(
                            expiration_ms.saturating_sub(
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64,
                            ),
                        );

                    let subscription = crate::subscriptions::Subscription {
                        uri: request.uri.clone(),
                        channel_id,
                        resource_id,
                        expiration,
                        peer,
                        service: parsed.service,
                        polling: false,
                    };

                    let st = self.state.lock().await;
                    st.subscriptions
                        .lock()
                        .await
                        .insert(request.uri, subscription);
                    tracing::info!("Subscription created for resource (webhook mode)");
                    Ok(())
                }
                None => {
                    // Polling fallback: no webhook URL (stdio mode or HTTP without
                    // --external-url). Re-read the resource periodically and emit
                    // resource-updated notifications on change.
                    let interval =
                        std::time::Duration::from_secs(policy.subscription_poll_secs.max(10));
                    let cfg = crate::subscriptions::PollConfig {
                        service: parsed.service.clone(),
                        resource: parsed.resource,
                        id: parsed.id,
                        interval,
                    };
                    let subscription = crate::subscriptions::Subscription {
                        uri: request.uri.clone(),
                        channel_id: String::new(),
                        resource_id: String::new(),
                        expiration: std::time::Instant::now()
                            + std::time::Duration::from_secs(10 * 365 * 24 * 3600),
                        peer,
                        service: parsed.service,
                        polling: true,
                    };

                    let uri = request.uri;
                    {
                        let st = self.state.lock().await;
                        st.subscriptions
                            .lock()
                            .await
                            .insert(uri.clone(), subscription);
                    }
                    crate::subscriptions::spawn_poll_task(
                        uri,
                        cfg,
                        self.state.clone(),
                        self.policy.clone(),
                    );
                    tracing::info!(
                        interval_secs = interval.as_secs(),
                        "Subscription created for resource (polling mode)"
                    );
                    Ok(())
                }
            }
        }
    }

    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        async move {
            let parsed = crate::resources::parse_gws_uri(&request.uri).ok_or_else(|| {
                McpError::invalid_params(format!("Invalid resource URI: {}", request.uri), None)
            })?;

            let (channel_id, resource_id, polling, service) = {
                let st = self.state.lock().await;
                let sub = {
                    let mut subs = st.subscriptions.lock().await;
                    subs.remove(&request.uri).ok_or_else(|| {
                        McpError::invalid_params(
                            format!("No subscription for URI: {}", request.uri),
                            None,
                        )
                    })?
                };
                (sub.channel_id, sub.resource_id, sub.polling, sub.service)
            };

            if polling {
                // The poll task detects removal and exits on its own; no API call.
                tracing::info!(uri = %request.uri, "Polling subscription removed");
                return Ok(());
            }

            let policy = self.policy.read().await;
            // `channels.stop` POSTs to Drive just like `files.watch`.
            if let Err(msg) = check_drive_watch_allowed(&policy, &service, &parsed.resource) {
                // The subscription is already gone locally: the watch is being
                // abandoned, so do not leave its secret behind until TTL.
                crate::subscriptions::unregister_channel_secret_for(&self.owner, &channel_id).await;
                return Err(McpError::invalid_params(msg, None));
            }
            if let Err(msg) =
                check_drive_method_allowed(&self.state, &policy, "channels", "stop").await
            {
                crate::subscriptions::unregister_channel_secret_for(&self.owner, &channel_id).await;
                return Err(McpError::invalid_params(msg, None));
            }

            // `stop_watch` drops the secret on every exit path, including
            // errors; additionally drop it here so a failure can never leave
            // the local secret behind, and surface the failure in the logs.
            let stop_result = {
                let mut st = self.state.lock().await;
                crate::subscriptions::stop_watch(
                    &self.owner,
                    &channel_id,
                    &resource_id,
                    &mut st.token_cache,
                    &policy,
                )
                .await
            };
            if let Err(e) = stop_result {
                tracing::warn!(
                    uri = %request.uri,
                    channel_id = %channel_id,
                    owner = %self.owner,
                    error = %e,
                    "Failed to stop watch channel; abandoned subscription, dropped local secret"
                );
                crate::subscriptions::unregister_channel_secret_for(&self.owner, &channel_id).await;
            }

            tracing::info!(uri = %request.uri, "Subscription removed");
            Ok(())
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        async move {
            let st = self.state.lock().await;
            let prompts = st
                .prompts
                .iter()
                .map(|p| {
                    let args: Option<Vec<rmcp::model::PromptArgument>> = if p.arguments.is_empty() {
                        None
                    } else {
                        Some(
                            p.arguments
                                .iter()
                                .map(|a| {
                                    rmcp::model::PromptArgument::new(&a.name)
                                        .with_description(&a.description)
                                        .with_required(a.required)
                                })
                                .collect(),
                        )
                    };
                    let mut prompt = rmcp::model::Prompt::new(&p.name, Some(&p.description), args);
                    if !p.title.is_empty() {
                        prompt = prompt.with_title(&p.title);
                    }
                    prompt
                })
                .collect();
            Ok(ListPromptsResult {
                result_type: Some(ResultType::COMPLETE),
                meta: None,
                next_cursor: None,
                ttl_ms: None,
                cache_scope: None,
                prompts,
            })
        }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResponse, McpError>> + Send + '_ {
        async move {
            let st = self.state.lock().await;
            let args_value = match &request.arguments {
                Some(map) => Value::Object(map.clone()),
                None => json!({}),
            };
            let result = crate::prompts::get_prompt(&st.prompts, &request.name, &args_value)
                .map_err(|msg| McpError::invalid_params(msg, None))?;

            let description = result
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let body_text = result
                .pointer("/messages/0/content/text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let messages = vec![PromptMessage::new_text(Role::User, body_text)];

            let mut r = GetPromptResult::new(messages);
            if let Some(desc) = description {
                r = r.with_description(desc);
            }
            Ok(r.into())
        }
    }

    fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskResult, McpError>> + Send + '_ {
        async move {
            let detailed = self.task_manager.get_task(&request.task_id)?;
            Ok(GetTaskResult::new(detailed))
        }
    }

    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        async move { self.task_manager.cancel_task(&request.task_id) }
    }
}

fn value_to_call_tool_result(value: Value) -> CallToolResult {
    let is_error = value
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let content: Vec<ContentBlock> = if let Some(arr) =
        value.get("content").and_then(|v| v.as_array())
    {
        arr.iter()
            .map(|item| {
                let content_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                match content_type {
                    "text" => {
                        let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        ContentBlock::text(text)
                    }
                    "image" => {
                        let data = item.get("data").and_then(|v| v.as_str()).unwrap_or("");
                        let mime = item
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/png");
                        ContentBlock::image(data, mime)
                    }
                    _ => {
                        let text = serde_json::to_string(item).unwrap_or_else(|_| "{}".to_string());
                        ContentBlock::text(text)
                    }
                }
            })
            .collect()
    } else {
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
        vec![ContentBlock::text(text)]
    };

    let structured_content = value.get("structuredContent").cloned();

    if is_error {
        let mut result = CallToolResult::error(content);
        result.structured_content = structured_content;
        result
    } else if let Some(sc) = structured_content {
        let mut result = CallToolResult::structured(sc);
        result.content = content;
        result
    } else {
        CallToolResult::success(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_from(value: Value) -> Policy {
        let file: crate::policy::PolicyFile = serde_json::from_value(value).unwrap();
        Policy::from_policy_file(file)
    }

    #[test]
    fn test_subscribe_rejects_read_only_drive() {
        let policy = policy_from(json!({
            "services": [{"name": "drive", "read_only": true}]
        }));
        let err = check_drive_watch_allowed(&policy, "drive", "files").unwrap_err();
        assert!(err.contains("read-only"), "{err}");
        assert!(err.contains("files.watch"), "{err}");
    }

    #[test]
    fn test_subscribe_rejects_global_read_only() {
        let policy = policy_from(json!({
            "server": {"read_only": true},
            "services": [{"name": "drive"}]
        }));
        let err = check_drive_watch_allowed(&policy, "drive", "files").unwrap_err();
        assert!(err.contains("read-only"), "{err}");
    }

    #[test]
    fn test_subscribe_rejects_sheets_only_policy() {
        let policy = policy_from(json!({
            "services": [{"name": "sheets"}]
        }));
        let err = check_drive_watch_allowed(&policy, "drive", "files").unwrap_err();
        assert!(err.contains("'drive' is not enabled by policy"), "{err}");
    }

    #[test]
    fn test_subscribe_rejects_non_drive_service() {
        let policy = policy_from(json!({
            "services": [{"name": "drive"}, {"name": "gmail"}]
        }));
        let err = check_drive_watch_allowed(&policy, "gmail", "messages").unwrap_err();
        assert!(
            err.contains("only supports Drive push notifications"),
            "{err}"
        );
        assert!(err.contains("gmail"), "{err}");
    }

    #[test]
    fn test_subscribe_rejects_resource_outside_allowlist() {
        let policy = policy_from(json!({
            "services": [{"name": "drive", "allowed_resources": ["files"]}]
        }));
        let err = check_drive_watch_allowed(&policy, "drive", "comments").unwrap_err();
        assert!(err.contains("allowed_resources"), "{err}");
        assert!(check_drive_watch_allowed(&policy, "drive", "files").is_ok());
    }

    #[test]
    fn test_subscribe_allows_drive_write_policy() {
        let policy = policy_from(json!({
            "services": [{"name": "drive"}]
        }));
        assert!(check_drive_watch_allowed(&policy, "drive", "files").is_ok());
    }
}
