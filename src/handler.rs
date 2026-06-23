use std::sync::Arc;

use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::task_manager::{OperationProcessor, OperationMessage, OperationDescriptor, ToolCallTaskResult};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::policy::Policy;
use crate::server::ServerState;

pub struct GwsHandler {
    pub(crate) state: Arc<Mutex<ServerState>>,
    pub(crate) policy: Arc<RwLock<Policy>>,
    pub(crate) processor: Arc<Mutex<OperationProcessor>>,
}

impl GwsHandler {
    pub fn new(
        policy: Policy,
        prompts: Vec<crate::prompts::Prompt>,
        audit: Option<Arc<crate::audit::AuditLogger>>,
    ) -> Self {
        let mut state = ServerState::new();
        state.prompts = prompts;
        state.audit = audit;
        Self {
            state: Arc::new(Mutex::new(state)),
            policy: Arc::new(RwLock::new(policy)),
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
        }
    }

    pub fn from_shared(
        state: Arc<Mutex<ServerState>>,
        policy: Arc<RwLock<Policy>>,
    ) -> Self {
        Self {
            state,
            policy,
            processor: Arc::new(Mutex::new(OperationProcessor::new())),
        }
    }
}

impl ServerHandler for GwsHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
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
                let tools =
                    crate::tools::build_tools_list(&policy, &mut st.docs).await.map_err(
                        |e| McpError::internal_error(format!("Failed to build tools: {e}"), None),
                    )?;
                st.tools = Some(tools);
            }
            let tools = st.tools.as_ref().unwrap().clone();
            Ok(ListToolsResult {
                meta: None,
                next_cursor: None,
                tools,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
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
            let task_count = self.state.lock().await.tasks.len();
            crate::metrics::set_active_tasks(task_count as i64);

            match result {
                Ok(value) => Ok(value_to_call_tool_result(value)),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
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
        async move {
            Ok(ListResourcesResult::with_all_items(vec![]))
        }
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        async move {
            let policy = self.policy.read().await;
            let st = self.state.lock().await;
            let templates = crate::resources::build_resource_templates(&policy, &st.docs);
            Ok(ListResourceTemplatesResult::with_all_items(templates))
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        async move {
            let parsed = crate::resources::parse_gws_uri(&request.uri).ok_or_else(|| {
                McpError::invalid_params(
                    format!("Invalid resource URI: {}. Expected gws://service/resource/id", request.uri),
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

            let meta = crate::meta::RequestMeta::default();
            let mut st = self.state.lock().await;
            let doc = st.get_doc(&parsed.service).await.map_err(|e| {
                McpError::internal_error(format!("Failed to load discovery doc: {e}"), None)
            })?;

            let resource = crate::tools::find_resource(&doc.resources, &parsed.resource)
                .ok_or_else(|| {
                    McpError::invalid_params(
                        format!("Resource '{}' not found in {}", parsed.resource, parsed.service),
                        None,
                    )
                })?;

            let id_param = crate::resources::id_param_name(resource, "get").ok_or_else(|| {
                McpError::invalid_params(
                    format!("Resource '{}' has no get method with path parameter", parsed.resource),
                    None,
                )
            })?;

            let method = resource.methods.get("get").ok_or_else(|| {
                McpError::invalid_params(
                    format!("Resource '{}' has no get method", parsed.resource),
                    None,
                )
            })?;

            let args = json!({ "params": { &id_param: &parsed.id } });
            let result = crate::execute::execute_tool(
                &doc, method, &parsed.resource, "get", &args,
                &parsed.service, &policy, &meta, None, None, false,
                &mut st.token_cache,
            ).await.map_err(|e| {
                McpError::internal_error(format!("API call failed: {e}"), None)
            })?;

            let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
            let contents = rmcp::model::ResourceContents::text(&text, &request.uri)
                .with_mime_type("application/json");
            Ok(ReadResourceResult::new(vec![contents]))
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
                McpError::invalid_params(
                    format!("Invalid resource URI: {}", request.uri),
                    None,
                )
            })?;

            let mut st = self.state.lock().await;
            let webhook_url = st.webhook_url.as_ref().ok_or_else(|| {
                McpError::internal_error(
                    "Subscriptions require HTTP mode with --external-url. \
                     In stdio mode, Google cannot deliver webhook callbacks.",
                    None,
                )
            })?.clone();

            let policy = self.policy.read().await;
            if !policy.is_service_allowed(&parsed.service) {
                return Err(McpError::invalid_params(
                    format!("Service '{}' not enabled", parsed.service),
                    None,
                ));
            }

            let (channel_id, resource_id, expiration_ms) =
                crate::subscriptions::watch_resource(
                    &parsed.service,
                    &parsed.id,
                    &webhook_url,
                    &mut st.token_cache,
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
            };

            st.subscriptions.lock().await.insert(request.uri, subscription);
            tracing::info!("Subscription created for resource");
            Ok(())
        }
    }

    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        async move {
            let mut st = self.state.lock().await;
            let sub = {
                let mut subs = st.subscriptions.lock().await;
                subs.remove(&request.uri).ok_or_else(|| {
                    McpError::invalid_params(
                        format!("No subscription for URI: {}", request.uri),
                        None,
                    )
                })?
            };

            let policy = self.policy.read().await;
            let _ = crate::subscriptions::stop_watch(
                &sub.channel_id,
                &sub.resource_id,
                &mut st.token_cache,
                &policy,
            )
            .await;

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
                    let mut prompt =
                        rmcp::model::Prompt::new(&p.name, Some(&p.description), args);
                    if !p.title.is_empty() {
                        prompt = prompt.with_title(&p.title);
                    }
                    prompt
                })
                .collect();
            Ok(ListPromptsResult {
                meta: None,
                next_cursor: None,
                prompts,
            })
        }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
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

            let messages = vec![PromptMessage::new_text(PromptMessageRole::User, body_text)];

            let mut r = GetPromptResult::new(messages);
            if let Some(desc) = description {
                r = r.with_description(desc);
            }
            Ok(r)
        }
    }

    fn enqueue_task(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CreateTaskResult, McpError>> + Send + '_ {
        let task_id = context.id.to_string();
        let processor = self.processor.clone();
        async move {
            let now = rmcp::task_manager::current_timestamp();
            let descriptor = OperationDescriptor::new(&task_id, request.name.to_string());

            let state = self.state.clone();
            let policy = self.policy.clone();
            let peer = context.peer.clone();
            let progress_token = context.meta.get_progress_token();
            let request_meta = crate::meta::RequestMeta::from_rmcp_meta(&context.meta);
            let tid = task_id.clone();

            let future = Box::pin(async move {
                let tool_name = request.name.to_string();
                let arguments = match request.arguments {
                    Some(map) => Value::Object(map),
                    None => json!({}),
                };
                let params = json!({ "name": tool_name, "arguments": arguments });
                let policy = policy.read().await;

                let result = crate::server::handle_tool_call_concurrent(
                    &params,
                    &request_meta,
                    &policy,
                    &state,
                    Some(&peer),
                    progress_token.as_ref(),
                )
                .await;

                let call_result = match result {
                    Ok(value) => Ok(value_to_call_tool_result(value)),
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                };

                Ok(Box::new(ToolCallTaskResult::new(tid, call_result))
                    as Box<dyn rmcp::task_manager::OperationResultTransport>)
            });

            let message = OperationMessage::new(descriptor, future);
            processor.lock().await.submit_operation(message).map_err(|e| {
                McpError::internal_error(format!("Failed to enqueue task: {e}"), None)
            })?;

            let task = rmcp::model::Task::new(
                task_id,
                TaskStatus::Working,
                now.clone(),
                now,
            ).with_poll_interval(2000);

            Ok(CreateTaskResult::new(task))
        }
    }

    fn list_tasks(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListTasksResult, McpError>> + Send + '_ {
        async move {
            let mut proc = self.processor.lock().await;
            let running = proc.list_running();
            let now = rmcp::task_manager::current_timestamp();
            let tasks: Vec<rmcp::model::Task> = running
                .into_iter()
                .map(|id| rmcp::model::Task::new(id, TaskStatus::Working, now.clone(), now.clone()))
                .collect();
            Ok(ListTasksResult::new(tasks))
        }
    }

    fn get_task_info(
        &self,
        request: GetTaskInfoParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskResult, McpError>> + Send + '_ {
        async move {
            let mut proc = self.processor.lock().await;
            let now = rmcp::task_manager::current_timestamp();

            for result in proc.peek_completed() {
                if result.descriptor.operation_id == request.task_id {
                    let status = if result.result.is_ok() {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Failed
                    };
                    let task = rmcp::model::Task::new(
                        request.task_id, status, now.clone(), now,
                    );
                    return Ok(GetTaskResult { meta: None, task });
                }
            }

            if proc.list_running().contains(&request.task_id) {
                let task = rmcp::model::Task::new(
                    request.task_id, TaskStatus::Working, now.clone(), now,
                );
                return Ok(GetTaskResult { meta: None, task });
            }

            Err(McpError::invalid_params(
                format!("Task '{}' not found", request.task_id), None,
            ))
        }
    }

    fn get_task_result(
        &self,
        request: GetTaskResultParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskPayloadResult, McpError>> + Send + '_ {
        async move {
            loop {
                let mut proc = self.processor.lock().await;
                if let Some(result) = proc.take_completed_result(&request.task_id) {
                    match result.result {
                        Ok(boxed) => {
                            let any = boxed.as_any();
                            if let Some(tcr) = any.downcast_ref::<ToolCallTaskResult>() {
                                let value = match &tcr.result {
                                    Ok(ctr) => serde_json::to_value(ctr).unwrap_or(json!({})),
                                    Err(e) => json!({"error": e.message}),
                                };
                                return Ok(GetTaskPayloadResult::new(value));
                            }
                            return Err(McpError::internal_error(
                                "Unexpected task result type", None,
                            ));
                        }
                        Err(e) => {
                            return Err(McpError::internal_error(
                                format!("Task failed: {e}"), None,
                            ));
                        }
                    }
                }
                drop(proc);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }

    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CancelTaskResult, McpError>> + Send + '_ {
        async move {
            let mut proc = self.processor.lock().await;
            let now = rmcp::task_manager::current_timestamp();
            if proc.cancel_task(&request.task_id) {
                let task = rmcp::model::Task::new(
                    request.task_id, TaskStatus::Cancelled, now.clone(), now,
                );
                Ok(CancelTaskResult { meta: None, task })
            } else {
                Err(McpError::invalid_params(
                    format!("Task '{}' not found or already completed", request.task_id),
                    None,
                ))
            }
        }
    }
}

fn value_to_call_tool_result(value: Value) -> CallToolResult {
    let is_error = value
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let content: Vec<Content> =
        if let Some(arr) = value.get("content").and_then(|v| v.as_array()) {
            arr.iter()
                .filter_map(|item| {
                    let content_type =
                        item.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                    match content_type {
                        "text" => {
                            let text =
                                item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            Some(Content::text(text))
                        }
                        "image" => {
                            let data =
                                item.get("data").and_then(|v| v.as_str()).unwrap_or("");
                            let mime = item
                                .get("mimeType")
                                .and_then(|v| v.as_str())
                                .unwrap_or("image/png");
                            Some(Content::image(data, mime))
                        }
                        _ => {
                            let text = serde_json::to_string(item)
                                .unwrap_or_else(|_| "{}".to_string());
                            Some(Content::text(text))
                        }
                    }
                })
                .collect()
        } else {
            let text =
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
            vec![Content::text(text)]
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
