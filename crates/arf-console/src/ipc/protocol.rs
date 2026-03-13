//! JSON-RPC 2.0 protocol types for arf IPC.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response object.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

// Standard JSON-RPC error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

// Application-specific error codes
pub const R_BUSY: i32 = -32000;
pub const R_NOT_AT_PROMPT: i32 = -32001;
pub const INPUT_ALREADY_PENDING: i32 = -32002;
pub const USER_IS_TYPING: i32 = -32003;

/// Parameters for the `evaluate` method.
#[derive(Debug, Deserialize)]
pub struct EvaluateParams {
    pub code: String,
    #[serde(default)]
    pub visible: bool,
}

/// Result of the `evaluate` method.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvaluateResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameters for the `user_input` method.
#[derive(Debug, Deserialize)]
pub struct UserInputParams {
    pub code: String,
}

/// Result of the `user_input` method.
#[derive(Debug, Serialize)]
pub struct UserInputResult {
    pub accepted: bool,
}

/// Internal request type sent from IPC server thread to main thread.
pub struct IpcRequest {
    pub method: IpcMethod,
    pub reply: tokio::sync::oneshot::Sender<IpcResponse>,
}

/// IPC method variants for internal dispatch.
pub enum IpcMethod {
    Evaluate { code: String, visible: bool },
    UserInput { code: String },
}

/// Internal response type sent from main thread back to IPC server thread.
pub enum IpcResponse {
    Evaluate(EvaluateResult),
    UserInput(UserInputResult),
    Error { code: i32, message: String },
}
