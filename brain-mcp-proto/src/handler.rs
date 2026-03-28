use std::sync::Arc;

use serde_json::json;

use brain_core::error::BrainError;
use brain_core::model::Filter;
use brain_core::service::MemoryService;

use crate::jsonrpc::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND, Request, Response};
use crate::schema::tool_definitions;

pub struct McpHandler {
    service: Arc<MemoryService>,
}

impl McpHandler {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }

    pub async fn handle(&self, request: Request) -> Response {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(&request),
            "notifications/initialized" => Response::success(request.id, json!({})),
            "tools/list" => self.handle_tools_list(&request),
            "tools/call" => self.handle_tools_call(request).await,
            _ => Response::error(request.id, METHOD_NOT_FOUND, "Method not found"),
        }
    }

    fn handle_initialize(&self, request: &Request) -> Response {
        Response::success(
            request.id.clone(),
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "brain-mcp",
                    "version": "0.1.0"
                }
            }),
        )
    }

    fn handle_tools_list(&self, request: &Request) -> Response {
        Response::success(request.id.clone(), json!({ "tools": tool_definitions() }))
    }

    async fn handle_tools_call(&self, request: Request) -> Response {
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
            "memory_store" => self.tool_store(&args).await,
            "memory_search" => self.tool_search(&args).await,
            "memory_list" => self.tool_list(&args).await,
            "memory_update" => self.tool_update(&args).await,
            "memory_delete" => self.tool_delete(&args).await,
            "memory_reindex" => self.tool_reindex().await,
            _ => {
                return Response::error(
                    request.id,
                    METHOD_NOT_FOUND,
                    format!("Unknown tool: {name}"),
                );
            }
        };

        match result {
            Ok(value) => Response::success(request.id, value),
            Err(e) => {
                let code = match &e {
                    BrainError::NotFound(_) => INVALID_PARAMS,
                    _ => INTERNAL_ERROR,
                };
                Response::error(request.id, code, e.to_string())
            }
        }
    }

    async fn tool_store(&self, args: &serde_json::Value) -> Result<serde_json::Value, BrainError> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrainError::Vault("Missing required field: content".into()))?
            .to_string();
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrainError::Vault("Missing required field: title".into()))?
            .to_string();
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| BrainError::Vault("Missing required field: tags".into()))?;
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("learnings")
            .to_string();
        let project = args
            .get("project")
            .and_then(|v| v.as_str())
            .map(String::from);

        let memory = self
            .service
            .store(title, content, tags, category, project)
            .await?;

        Ok(text_content(serde_json::to_string(&memory).unwrap()))
    }

    async fn tool_search(&self, args: &serde_json::Value) -> Result<serde_json::Value, BrainError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrainError::Vault("Missing required field: query".into()))?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let tags: Option<Vec<String>> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let filter = Filter {
            tags,
            ..Default::default()
        };

        let results = self.service.search(query, limit, &filter).await?;

        let output: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                json!({
                    "memory": r.memory,
                    "score": r.score,
                })
            })
            .collect();

        Ok(text_content(serde_json::to_string(&output).unwrap()))
    }

    async fn tool_list(&self, args: &serde_json::Value) -> Result<serde_json::Value, BrainError> {
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

        Ok(text_content(serde_json::to_string(&metadata).unwrap()))
    }

    async fn tool_update(&self, args: &serde_json::Value) -> Result<serde_json::Value, BrainError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrainError::Vault("Missing required field: id".into()))?;
        let title = args.get("title").and_then(|v| v.as_str()).map(String::from);
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tags: Option<Vec<String>> = args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let memory = self.service.update(id, title, content, tags).await?;

        Ok(text_content(serde_json::to_string(&memory).unwrap()))
    }

    async fn tool_delete(&self, args: &serde_json::Value) -> Result<serde_json::Value, BrainError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| BrainError::Vault("Missing required field: id".into()))?;

        self.service.delete(id).await?;

        Ok(text_content(json!({"deleted": id}).to_string()))
    }

    async fn tool_reindex(&self) -> Result<serde_json::Value, BrainError> {
        let count = self.service.reindex().await?;

        Ok(text_content(json!({"reindexed": count}).to_string()))
    }
}

fn text_content(text: String) -> serde_json::Value {
    json!({
        "content": [{ "type": "text", "text": text }]
    })
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
        let resp = handler.handle(req).await;

        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "brain-mcp");
    }

    #[tokio::test]
    async fn test_tools_list_returns_6_tools() {
        let handler = make_handler();
        let req = make_request("tools/list", Some(json!(2)), None);
        let resp = handler.handle(req).await;

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
        let resp = handler.handle(req).await;

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
        handler.handle(store_req).await;

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
        let resp = handler.handle(search_req).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let results: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["memory"]["title"], "Rust Lifetimes");
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
        let resp = handler.handle(req).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("nonexistent_tool"));
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let handler = make_handler();
        let req = make_request("something/weird", Some(json!(5)), None);
        let resp = handler.handle(req).await;

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }
}
