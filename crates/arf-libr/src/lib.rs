//! Low-level R FFI bindings using dynamic loading.
//!
//! This crate provides dynamic bindings to R's C API using `libloading`.
//! It allows loading R at runtime without compile-time linking.

mod bounded_process;
mod error;
mod functions;
mod library_selection;
mod rhome_probe;
mod types;
#[cfg(windows)]
mod windows_loader;

mod sys;

// error
pub use bounded_process::{BoundedProcessError, run_bounded_command};
pub use error::{RError, RResult};
pub use library_selection::select_r_library;
pub use rhome_probe::{RHomeProbeError, probe_r_home};

// functions
pub use functions::{RLibrary, r_library, r_nil_value};

// types
pub use types::{
    CE_BYTES, CE_LATIN1, CE_NATIVE, CE_UTF8, DL_FUNC, DllInfo, ParseStatus, R_CMethodDef,
    R_CallMethodDef, R_ExternalMethodDef, R_FALSE, R_FortranMethodDef, R_NativePrimitiveArgType,
    R_TRUE, Rboolean, ReadConsoleFunc, SEXP, SEXPREC, SexpType, WriteConsoleExFunc,
};
#[cfg(windows)]
pub use types::{Rstart, SaType, UImode};

// sys
#[cfg(unix)]
pub use sys::askpass_handler_code;
pub use sys::{
    CommandOutcome, ConsoleEvaluationInterruptGuard, HeadlessReadConsoleCallback, InterruptPhase,
    ReadConsoleInput, ReadConsolePromptInfo, ReadConsoleResult, begin_ipc_execution,
    begin_ipc_execution_finishing, begin_read_console_input, command_outcome,
    command_outcome_tracking_available, ensure_ld_library_path, find_r_library, finish_ipc_capture,
    finish_ipc_execution, flush_reprex_buffer, frame_depth_protected, get_r_home,
    global_error_handler_code, initialize_r, initialize_r_for_tests, initialize_r_with_args,
    is_browse_prompt, is_r_auto_discovery_disabled, is_r_awaiting_console_input,
    is_r_interrupt_flag_available, mark_global_error_handler_initialized,
    mark_ipc_interrupt_discarded, mark_ipc_interrupt_observed, process_r_events,
    process_r_events_protected, r_home_from_library_path, r_home_from_rhome_output, r_library_path,
    r_path_vars_from_wrapper, read_console_chunk_end, request_ipc_interrupt,
    reset_command_error_state, reset_console_generation, restore_stderr, run_r_mainloop,
    set_console_output_observer, set_headless_read_console_callback, set_ipc_execution_phase,
    set_r_auto_discovery_disabled, set_r_interrupt_pending, set_read_console_callback,
    set_reprex_mode, set_spinner_color, set_spinner_frames, set_write_console_callback,
    start_ipc_capture, start_spinner, stop_spinner, suppress_stderr, take_ipc_interrupt_request,
};
#[cfg(unix)]
pub use sys::{
    ensure_ld_library_path_with_pre_exec, ensure_ld_library_path_with_pre_exec_and_args,
};
