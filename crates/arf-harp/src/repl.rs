//! Initialization for the C-owned incremental command driver.
//!
//! The parser closure keeps a text connection alive and yields one expression
//! per call. Evaluation and printing remain in the native C driver so R's
//! longjmp never crosses a Rust frame. The custom boundary intentionally does
//! not reproduce `.Last.value` updates or R's task callback dispatch.

use crate::{RObject, eval_string_with_visibility};
use std::sync::atomic::{AtomicU8, Ordering};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplEngineState {
    Uninitialized = 0,
    Installing = 1,
    Ready = 2,
    Failed = 3,
}

static ENGINE_STATE: AtomicU8 = AtomicU8::new(ReplEngineState::Uninitialized as u8);

pub fn repl_engine_state() -> ReplEngineState {
    match ENGINE_STATE.load(Ordering::Acquire) {
        1 => ReplEngineState::Installing,
        2 => ReplEngineState::Ready,
        3 => ReplEngineState::Failed,
        _ => ReplEngineState::Uninitialized,
    }
}

/// Install the R-owned incremental parser used by `arf_libr::install_repl_driver`.
/// Call after R initialization and before entering `run_Rmainloop`.
pub fn initialize_repl_engine() -> crate::HarpResult<RObject> {
    match ENGINE_STATE.compare_exchange(
        ReplEngineState::Uninitialized as u8,
        ReplEngineState::Installing as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => {}
        Err(state) if state == ReplEngineState::Ready as u8 => {}
        Err(state) if state == ReplEngineState::Installing as u8 => {
            return Err(crate::HarpError::RError(arf_libr::RError::EvalError(
                "REPL engine initialization is already in progress".into(),
            )));
        }
        Err(_) => {
            return Err(crate::HarpError::RError(arf_libr::RError::EvalError(
                "REPL engine initialization previously failed".into(),
            )));
        }
    }

    let install = eval_string_with_visibility(
        r#"
function(text) {
    connection <- base::textConnection(text, open = "r")
    list(
        function() base::parse(connection, n = 1L),
        function() try(base::close(connection), silent = TRUE)
    )
}
"#,
    );

    match install {
        Ok(value) => {
            ENGINE_STATE.store(ReplEngineState::Ready as u8, Ordering::Release);
            Ok(value.value)
        }
        Err(error) => {
            ENGINE_STATE.store(ReplEngineState::Failed as u8, Ordering::Release);
            Err(error)
        }
    }
}
