//! Low-level R FFI bindings using dynamic loading.
//!
//! This crate provides dynamic bindings to R's C API using `libloading`.
//! It allows loading R at runtime without compile-time linking.

mod error;
mod functions;
mod types;

mod sys;

// error
pub use error::{RError, RResult};

// functions
pub use functions::{RLibrary, init_r_library, r_global_env, r_library, r_nil_value};

// types
pub use types::{
    ParseStatus, R_FALSE, R_TRUE, Rboolean, ReadConsoleFunc, SEXP, SEXPREC, SexpType,
    WriteConsoleExFunc,
};
#[cfg(windows)]
pub use types::{Rstart, SaType, UImode};

// sys
#[cfg(unix)]
pub use sys::askpass_handler_code;
#[cfg(any(windows, test))]
pub use sys::strip_cr;
pub use sys::{
    command_had_error, ensure_ld_library_path, find_r_library, flush_reprex_buffer, get_r_home,
    global_error_handler_code, initialize_r, initialize_r_with_args, is_spinner_active,
    mark_error_condition, mark_global_error_handler_initialized, peek_r_event,
    polled_events_for_repl, process_r_events, reset_command_error_state, restore_stderr,
    run_r_mainloop, set_read_console_callback, set_reprex_mode, set_spinner_color,
    set_spinner_frames, set_write_console_callback, start_spinner, stop_spinner, suppress_stderr,
};
