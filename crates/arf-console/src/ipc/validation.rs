//! Validation shared by IPC request modes before they reach the R thread.

use crate::editor::validator::RValidator;

use super::execution::DEFAULT_EVAL_TIMEOUT;
use super::policy;
use super::protocol::{
    INCOMPLETE_INPUT, INVALID_PARAMS, IpcMethod, JsonRpcResponse, R_EVAL_NOT_ALLOWED,
};

const MAX_TIMEOUT_MS: u64 = 86_400_000;
type ValidationError = Box<JsonRpcResponse>;

/// Validate code input before converting it into an internal IPC operation.
/// NUL is checked first because R's console callback receives a C string.
pub(super) fn validate_code(
    id: &Option<serde_json::Value>,
    code: &str,
) -> Result<(), ValidationError> {
    if code.contains('\0') {
        return Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            INVALID_PARAMS,
            "R code contains a NUL byte".to_string(),
        )));
    }

    if !RValidator::new().is_complete(code) {
        return Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            INCOMPLETE_INPUT,
            "R code is syntactically incomplete".to_string(),
        )));
    }

    Ok(())
}

fn validate_silent_evaluation(
    id: &Option<serde_json::Value>,
    code: &str,
) -> Result<(), ValidationError> {
    if let Err(reason) = policy::validate(code) {
        return Err(Box::new(JsonRpcResponse::error(
            id.clone(),
            R_EVAL_NOT_ALLOWED,
            format!("IPC evaluation rejected by policy: {reason}"),
        )));
    }

    Ok(())
}

/// Construct the internal method after shared request validation.
pub(super) fn validated_method(
    id: &Option<serde_json::Value>,
    code: String,
    visible: bool,
    timeout_ms: Option<u64>,
) -> Result<IpcMethod, ValidationError> {
    validate_code(id, &code)?;
    let timeout = match timeout_ms {
        Some(ms) if ms > MAX_TIMEOUT_MS => {
            return Err(Box::new(JsonRpcResponse::error(
                id.clone(),
                INVALID_PARAMS,
                format!("timeout_ms too large (max {MAX_TIMEOUT_MS} ms, got {ms})"),
            )));
        }
        Some(ms) => std::time::Duration::from_millis(ms),
        None => DEFAULT_EVAL_TIMEOUT,
    };
    if !visible {
        validate_silent_evaluation(id, &code)?;
    }
    Ok(if visible {
        IpcMethod::VisibleEvaluate { code, timeout }
    } else {
        IpcMethod::SilentEvaluate { code, timeout }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::{INCOMPLETE_INPUT, INVALID_PARAMS};

    #[test]
    fn nul_is_rejected_before_incomplete_syntax() {
        let id = Some(serde_json::json!(7));
        let error = validate_code(&id, "function(x) {\0").unwrap_err();
        assert_eq!(error.id, id);
        assert_eq!(error.error.unwrap().code, INVALID_PARAMS);
    }

    #[test]
    fn incomplete_code_is_rejected_by_shared_validation() {
        let error = validate_code(&None, "function(x) {").unwrap_err();
        assert_eq!(error.error.unwrap().code, INCOMPLETE_INPUT);
    }

    #[test]
    fn validated_evaluate_methods_encode_visibility_in_the_variant() {
        assert!(matches!(
            validated_method(&None, "1".to_string(), false, Some(4_000)).unwrap(),
            IpcMethod::SilentEvaluate { code, timeout: actual }
                if code == "1" && actual == std::time::Duration::from_secs(4)
        ));
        assert!(matches!(
            validated_method(&None, "1".to_string(), true, Some(4_000)).unwrap(),
            IpcMethod::VisibleEvaluate { code, timeout: actual }
                if code == "1" && actual == std::time::Duration::from_secs(4)
        ));
    }
}
