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

// Standard JSON-RPC internal error
pub const INTERNAL_ERROR: i32 = -32603;

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
    /// Timeout in milliseconds. If the evaluation does not complete within
    /// this duration, the server returns a timeout error. `None` means use
    /// the default (300 seconds).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
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

/// Result of the `shutdown` method.
#[derive(Debug, Serialize)]
pub struct ShutdownResult {
    pub accepted: bool,
}

/// R session information collected from base R functions.
///
/// Only available when R is idle (at the prompt). When R is busy,
/// this is `None` in the parent `SessionResult`.
#[derive(Debug, Serialize, Deserialize)]
pub struct RSessionInfo {
    pub version: String,
    pub platform: String,
    pub locale: String,
    pub cwd: String,
    pub loaded_namespaces: Vec<String>,
    pub attached_packages: Vec<String>,
    pub lib_paths: Vec<String>,
}

/// Result of the `session` method.
///
/// Always contains arf-side information. R session information is included
/// when R is idle, or `null` with an explanation when R is busy.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResult {
    pub arf_version: String,
    pub pid: u32,
    pub os: String,
    pub arch: String,
    pub socket_path: String,
    pub started_at: String,
    /// R session information, or `null` if R is busy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<RSessionInfo>,
    /// Reason why R information is unavailable, or `null` if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r_unavailable_reason: Option<String>,
    /// Hint for the caller on what to do next, or `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Internal request type sent from IPC server thread to main thread.
pub struct IpcRequest {
    pub method: IpcMethod,
    pub reply: tokio::sync::oneshot::Sender<IpcResponse>,
}

/// IPC method variants for internal dispatch.
pub enum IpcMethod {
    Evaluate {
        code: String,
        visible: bool,
        timeout_ms: Option<u64>,
    },
    UserInput {
        code: String,
    },
    /// Collect session information (arf + R if available).
    Session,
}

/// Internal response type sent from main thread back to IPC server thread.
pub enum IpcResponse {
    Evaluate(EvaluateResult),
    UserInput(UserInputResult),
    Session(Box<SessionResult>),
    Error { code: i32, message: String },
}
