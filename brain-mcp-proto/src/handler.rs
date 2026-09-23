use std::sync::Arc;

use serde_json::json;

use brain_core::error::BrainError;
use brain_core::model::{CallContext, Filter};
use brain_core::service::MemoryService;

use crate::jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND, Request, Response};
use crate::mcp::{initialize_result, tool_error, tool_text};
use crate::schema::tool_definitions;

pub struct McpHandler {
    service: Arc<MemoryService>,
}

enum ToolError {
    InvalidParams(String),
    Failed(BrainError),
}

impl From<BrainError> for ToolError {
    fn from(e: BrainError) -> Self {
        ToolError::Failed(e)
    }
}

fn missing(field: &str) -> ToolError {
    ToolError::InvalidParams(format!("Missing required field: {field}"))
}

impl McpHandler {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }

    pub async fn handle(&self, request: Request, ctx: &CallContext) -> Response {
        match request.method.as_str() {
            "initialize" => {
                Response::success(request.id, initialize_result(request.params.as_ref()))
            }
            "ping" => Response::success(request.id, json!({})),
            m if m.starts_with("notifications/") => Response::success(request.id, json!({})),
            "tools/list" => self.handle_tools_list(&request),
            "tools/call" => self.handle_tools_call(request, ctx).await,
            _ => Response::error(request.id, METHOD_NOT_FOUND, "Method not found"),
        }
    }

    fn handle_tools_list(&self, request: &Request) -> Response {
        Response::success(request.id.clone(), json!({ "tools": tool_definitions() }))
    }

    async fn handle_tools_call(&self, request: Request, ctx: &CallContext) -> Response {
        let params = match &request.params {
            Some(p) => p,
            None => return Response::error(request.id, INVALID_PARAMS, "Missing params"),
        };

        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return Response::error(request.id, INVALID_PARAMS, "Missing tool name"),
        };

        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = match name {
            "memory_store" => self.tool_store(ctx, &args).await,
            "memory_search" => self.tool_search(ctx, &args).await,
            "memory_list" => self.tool_list(&args).await,
            "memory_update" => self.tool_update(&args).await,
            "memory_delete" => self.tool_delete(&args).await,
            "memory_reindex" => self.tool_reindex().await,
            _ => Err(ToolError::InvalidParams(format!("Unknown tool: {name}"))),
        };

        match result {
            Ok(value) => Response::success(request.id, value),
            Err(ToolError::InvalidParams(msg)) => Response::error(request.id, INVALID_PARAMS, msg),
            Err(ToolError::Failed(e)) => {
                tracing::warn!(tool = name, error = %e, "tool call failed");
                Response::success(request.id, tool_error(e.to_string()))
            }
        }
    }

    async fn tool_store(
        &self,
        ctx: &CallContext,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing("content"))?
            .to_string();
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing("title"))?
            .to_string();
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| missing("tags"))?;
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("learnings")
            .to_string();
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from);
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        let memory = self
            .service
            .store_as(ctx, title, content, tags, category, project, force)
            .await?;

        Ok(tool_text(serde_json::to_string(&memory).unwrap()))
    }

    async fn tool_search(
        &self,
        ctx: &CallContext,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing("query"))?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let tags: Option<Vec<String>> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let filter = Filter {
            tags,
            ..Default::default()
        };

        let results = self.service.search_as(ctx, query, limit, &filter).await?;

        if results.is_empty() {
            return Ok(tool_text(
                "No relevant memories found for this query.".to_string(),
            ));
        }

        let output: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "memory": r.memory,
                    "score": r.score,
                })
            })
            .collect();

        Ok(tool_text(serde_json::to_string(&output).unwrap()))
    }

    async fn tool_list(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let tags: Option<Vec<String>> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(String::from);
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from);
        let since = args
            .get("since")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        let filter = Filter {
            tags,
            category,
            project,
            since,
        };

        let metadata = self.service.list(&filter).await?;

        Ok(tool_text(serde_json::to_string(&metadata).unwrap()))
    }

    async fn tool_update(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing("id"))?;
        let title = args.get("title").and_then(|v| v.as_str()).map(String::from);
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tags: Option<Vec<String>> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let memory = self.service.update(id, title, content, tags).await?;

        Ok(tool_text(serde_json::to_string(&memory).unwrap()))
    }

    async fn tool_delete(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| missing("id"))?;

        self.service.delete(id).await?;

        Ok(tool_text(json!({"deleted": id}).to_string()))
    }

    async fn tool_reindex(&self) -> Result<serde_json::Value, ToolError> {
        let count = self.service.reindex().await?;

        Ok(tool_text(json!({"reindexed": count}).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_core::mocks::{MockEmbedder, MockIndex, MockVault};
    use serde_json::json;

    fn make_handler() -> McpHandler {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let service = Arc::new(MemoryService::new(vault, embedder, index));
        McpHandler::new(service)
    }

    fn make_request(
        method: &str,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    ) -> Request {
        Request {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn test_initialize_returns_capabilities() {
        let handler = make_handler();
        let req = make_request("initialize", Some(json!(1)), None);
        let resp = handler.handle(req, &CallContext::default()).await;

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "brain-mcp");
    }

    #[tokio::test]
    async fn test_initialize_echoes_client_version() {
        let handler = make_handler();
        let req = make_request(
            "initialize",
            Some(json!(1)),
            Some(json!({"protocolVersion": "2024-11-05"})),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn test_ping_returns_empty_result() {
        let handler = make_handler();
        let req = make_request("ping", Some(json!(1)), None);
        let resp = handler.handle(req, &CallContext::default()).await;

        assert_eq!(resp.result, Some(json!({})));
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_tools_list_returns_6_tools() {
        let handler = make_handler();
        let req = make_request("tools/list", Some(json!(2)), None);
        let resp = handler.handle(req, &CallContext::default()).await;

        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"memory_store"));
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"memory_list"));
        assert!(names.contains(&"memory_update"));
        assert!(names.contains(&"memory_delete"));
        assert!(names.contains(&"memory_reindex"));

        let search = tools.iter().find(|t| t["name"] == "memory_search").unwrap();
        assert_eq!(search["annotations"]["readOnlyHint"], true);
        let delete = tools.iter().find(|t| t["name"] == "memory_delete").unwrap();
        assert_eq!(delete["annotations"]["destructiveHint"], true);
    }

    #[tokio::test]
    async fn test_tools_call_memory_store() {
        let handler = make_handler();
        let req = make_request(
            "tools/call",
            Some(json!(3)),
            Some(json!({
                "name": "memory_store",
                "arguments": {
                    "title": "Test Memory",
                    "content": "Some content here",
                    "tags": ["rust", "test"]
                }
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let memory: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(memory["title"], "Test Memory");
        assert_eq!(memory["content"], "Some content here");
        assert_eq!(memory["category"], "learnings");
    }

    #[tokio::test]
    async fn test_tools_call_memory_search() {
        let handler = make_handler();

        // Store a memory first
        let store_req = make_request(
            "tools/call",
            Some(json!(1)),
            Some(json!({
                "name": "memory_store",
                "arguments": {
                    "title": "Rust Lifetimes",
                    "content": "Lifetimes ensure references are valid",
                    "tags": ["rust"]
                }
            })),
        );
        handler.handle(store_req, &CallContext::default()).await;

        // Now search
        let search_req = make_request(
            "tools/call",
            Some(json!(2)),
            Some(json!({
                "name": "memory_search",
                "arguments": {
                    "query": "Lifetimes ensure references are valid",
                    "limit": 5
                }
            })),
        );
        let resp = handler.handle(search_req, &CallContext::default()).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let results: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["memory"]["title"], "Rust Lifetimes");
    }

    #[tokio::test]
    async fn test_tools_call_memory_search_empty() {
        let vault = Arc::new(MockVault::new());
        let embedder = Arc::new(MockEmbedder::new(8));
        let index = Arc::new(MockIndex::new());
        let service = Arc::new(MemoryService::new(vault, embedder, index).with_min_score(0.99));
        let handler = McpHandler::new(service);

        let req = make_request(
            "tools/call",
            Some(json!(1)),
            Some(json!({
                "name": "memory_search",
                "arguments": { "query": "anything" }
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "No relevant memories found for this query.");
    }

    #[tokio::test]
    async fn test_tools_call_unknown_tool() {
        let handler = make_handler();
        let req = make_request(
            "tools/call",
            Some(json!(4)),
            Some(json!({
                "name": "nonexistent_tool",
                "arguments": {}
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("nonexistent_tool"));
    }

    #[tokio::test]
    async fn test_tools_call_missing_argument_is_invalid_params() {
        let handler = make_handler();
        let req = make_request(
            "tools/call",
            Some(json!(5)),
            Some(json!({
                "name": "memory_store",
                "arguments": { "title": "t", "tags": [] }
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        let err = resp.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
        assert_eq!(err.message, "Missing required field: content");
    }

    #[tokio::test]
    async fn test_tools_call_service_error_is_tool_error() {
        let handler = make_handler();
        let req = make_request(
            "tools/call",
            Some(json!(6)),
            Some(json!({
                "name": "memory_update",
                "arguments": { "id": "20990101-nope", "title": "x" }
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "Memory not found: 20990101-nope"
        );
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let handler = make_handler();
        let req = make_request("something/weird", Some(json!(5)), None);
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_tools_call_store_invalid_category() {
        let handler = make_handler();
        let req = make_request(
            "tools/call",
            Some(json!(6)),
            Some(json!({
                "name": "memory_store",
                "arguments": {
                    "title": "Test Memory",
                    "content": "Some content here",
                    "tags": [],
                    "category": "feedback"
                }
            })),
        );
        let resp = handler.handle(req, &CallContext::default()).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Allowed categories:")
        );
    }

    #[tokio::test]
    async fn test_store_schema_lists_all_default_categories() {
        let tools = tool_definitions();
        let store = tools.iter().find(|t| t["name"] == "memory_store").unwrap();
        let description = store["inputSchema"]["properties"]["category"]["description"]
            .as_str()
            .unwrap();

        for category in brain_core::config::default_categories() {
            assert!(
                description.contains(&category),
                "description missing category {category}"
            );
        }
    }
}
