//! Shared admission limits and R-thread registry for exposed IPC handlers.

use super::protocol::{
    HANDLER_ARGUMENT_ERROR, HANDLER_INTERRUPTED, HANDLER_NOT_FOUND, HANDLER_QUEUE_TIMEOUT,
    HANDLER_R_ERROR, HANDLER_RESULT_ERROR, IpcMethod, IpcRequest, IpcResponse,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

const MAX_QUEUED_CALLS: usize = 64;
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerApiState {
    Unexposed,
    Locked,
    Revoked,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandlerApiStatus {
    pub state: HandlerApiState,
    pub handlers: Vec<String>,
}

impl Default for HandlerApiStatus {
    fn default() -> Self {
        Self {
            state: HandlerApiState::Unexposed,
            handlers: Vec::new(),
        }
    }
}

#[derive(Default)]
struct QueueUsage {
    count: usize,
    bytes: usize,
}

static QUEUE_USAGE: Mutex<QueueUsage> = Mutex::new(QueueUsage { count: 0, bytes: 0 });
static STATUS: RwLock<HandlerApiStatus> = RwLock::new(HandlerApiStatus {
    state: HandlerApiState::Unexposed,
    handlers: Vec::new(),
});

/// A reservation held while a call waits for the R thread.
///
/// It is released when the call starts or when its queued request is dropped.
pub struct HandlerQueuePermit {
    bytes: usize,
}

impl HandlerQueuePermit {
    pub fn try_acquire(handler: &str, args: &serde_json::Value) -> Option<Self> {
        let bytes = handler.len().checked_add(args.to_string().len())?;
        let mut usage = QUEUE_USAGE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if usage.count >= MAX_QUEUED_CALLS
            || usage
                .bytes
                .checked_add(bytes)
                .is_none_or(|total| total > MAX_QUEUED_BYTES)
        {
            return None;
        }
        usage.count += 1;
        usage.bytes += bytes;
        Some(Self { bytes })
    }

    /// Mark the call as started unless its queue deadline already cancelled it.
    /// Dropping this permit releases its count and byte reservation.
    pub fn start(self, state: &AtomicU8) -> bool {
        let started = state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        drop(self);
        started
    }
}

impl Drop for HandlerQueuePermit {
    fn drop(&mut self) {
        let mut usage = QUEUE_USAGE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        usage.count = usage.count.saturating_sub(1);
        usage.bytes = usage.bytes.saturating_sub(self.bytes);
    }
}

thread_local! {
    /// R objects in the registry must only be accessed on the R thread.
    static REGISTRY: std::cell::RefCell<Option<arf_harp::handlers::HandlerRegistry>> = const { std::cell::RefCell::new(None) };
}

fn replace_status(status: HandlerApiStatus) {
    *STATUS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
}

pub fn handler_status() -> HandlerApiStatus {
    STATUS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Evaluate and atomically expose the named functions returned by `expression`.
///
/// This must be called on the R thread after startup profiles have completed.
pub fn expose_handlers(expression: &str) -> Result<(), String> {
    REGISTRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if handler_status().state == HandlerApiState::Revoked {
            return Err("handlers have been permanently revoked for this session".to_string());
        }
        if slot.is_some() {
            return Err("handlers have already been exposed".to_string());
        }
        let mut registry = arf_harp::handlers::HandlerRegistry::new()?;
        let names = registry.expose(expression)?;
        replace_status(HandlerApiStatus {
            state: HandlerApiState::Locked,
            handlers: names,
        });
        *slot = Some(registry);
        Ok(())
    })
}

/// Permanently revoke handler calls for this R session.
pub fn revoke_handlers() {
    REGISTRY.with(|slot| {
        if let Some(registry) = slot.borrow_mut().as_mut() {
            registry.revoke();
        }
    });
    replace_status(HandlerApiStatus {
        state: HandlerApiState::Revoked,
        handlers: Vec::new(),
    });
}

/// Call an exposed function on the R thread.
pub fn call_handler(
    name: &str,
    args: &serde_json::Value,
) -> Result<arf_harp::handlers::HandlerResult, arf_harp::handlers::HandlerError> {
    REGISTRY.with(|slot| match slot.borrow().as_ref() {
        Some(registry) => registry.call(name, args),
        None => Err(arf_harp::handlers::HandlerError::not_found(name)),
    })
}

pub fn timeout_error_state() -> Arc<AtomicU8> {
    Arc::new(AtomicU8::new(0))
}

/// Execute a call that has reached the R thread. Returns immediately for a
/// closed reply or a call whose queue deadline expired before it started.
pub(crate) fn execute_handler_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;
    let IpcMethod::Call {
        handler,
        args,
        queue_deadline,
        queue,
        state,
        ..
    } = method
    else {
        log::error!("execute_handler_request received a non-call request");
        return;
    };
    if reply.is_closed() {
        return;
    }
    if Instant::now() >= queue_deadline {
        if state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = reply.send(IpcResponse::error(
                HANDLER_QUEUE_TIMEOUT,
                "Handler call expired before it started".to_string(),
            ));
        }
        return;
    }
    if !queue.start(&state) {
        return;
    }

    struct PromptGuard;
    impl Drop for PromptGuard {
        fn drop(&mut self) {
            super::state::set_r_at_prompt(true);
        }
    }
    super::state::set_r_at_prompt(false);
    let _prompt_guard = PromptGuard;
    let response = match call_handler(&handler, &args) {
        Ok(result) => IpcResponse::Call(serde_json::json!({
            "value": result.value,
            "stdout": result.stdout,
            "stderr": result.stderr,
        })),
        Err(error) => {
            let (code, message) = match error.kind {
                arf_harp::handlers::HandlerErrorKind::ArgumentConversion => {
                    (HANDLER_ARGUMENT_ERROR, error.message)
                }
                arf_harp::handlers::HandlerErrorKind::NotFound => {
                    (HANDLER_NOT_FOUND, error.message)
                }
                arf_harp::handlers::HandlerErrorKind::RError => (HANDLER_R_ERROR, error.message),
                arf_harp::handlers::HandlerErrorKind::ReturnConversion => {
                    (HANDLER_RESULT_ERROR, error.message)
                }
                arf_harp::handlers::HandlerErrorKind::Interrupted => {
                    (HANDLER_INTERRUPTED, error.message)
                }
            };
            IpcResponse::error_with_data(
                code,
                message.clone(),
                serde_json::json!({
                    "message": message.clone(),
                    "class": error.class,
                    "stdout": error.stdout,
                    "stderr": error.stderr,
                }),
            )
        }
    };
    let _ = reply.send(response);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn handler_queue_enforces_count_and_serialized_byte_limits() {
        let mut permits = Vec::new();
        for _ in 0..MAX_QUEUED_CALLS {
            permits.push(
                HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null)
                    .expect("admission below the count limit"),
            );
        }
        assert!(HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null).is_none());
        drop(permits);

        let exact_limit = serde_json::Value::String("x".repeat(MAX_QUEUED_BYTES - 2));
        let permit = HandlerQueuePermit::try_acquire("", &exact_limit)
            .expect("serialized payload exactly at the byte limit is admitted");
        assert!(HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null).is_none());
        drop(permit);
        assert!(HandlerQueuePermit::try_acquire("h", &serde_json::Value::Null).is_some());
    }
}
