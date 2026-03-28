use serde::{Deserialize, Serialize};

pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Response {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_jsonrpc_request_deserialization() {
        let raw = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "memory_store", "arguments": {}}
        }"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "tools/call");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_jsonrpc_response_success_serialization() {
        let resp = Response::success(Some(json!(1)), json!({"ok": true}));
        let val = serde_json::to_value(&resp).unwrap();
        assert_eq!(val["jsonrpc"], "2.0");
        assert_eq!(val["id"], 1);
        assert_eq!(val["result"]["ok"], true);
        assert!(val.get("error").is_none());
    }

    #[test]
    fn test_jsonrpc_response_error_serialization() {
        let resp = Response::error(Some(json!(2)), INVALID_PARAMS, "bad params");
        let val = serde_json::to_value(&resp).unwrap();
        assert_eq!(val["jsonrpc"], "2.0");
        assert_eq!(val["id"], 2);
        assert_eq!(val["error"]["code"], INVALID_PARAMS);
        assert_eq!(val["error"]["message"], "bad params");
        assert!(val.get("result").is_none());
    }
}
