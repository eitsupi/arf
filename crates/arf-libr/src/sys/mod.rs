//! Platform-specific R library loading and initialization.

mod askpass;
mod discovery;
mod error_state;
mod init;
mod interrupt;
mod output;
mod read_console;
mod spinner;

#[cfg(unix)]
pub use askpass::askpass_handler_code;
pub use discovery::{
    ensure_ld_library_path, find_r_library, get_r_home, is_r_auto_discovery_disabled,
    r_home_from_library_path, r_home_from_rhome_output, r_library_path, r_path_vars_from_wrapper,
    set_r_auto_discovery_disabled,
};
#[cfg(unix)]
pub use discovery::{
    ensure_ld_library_path_with_pre_exec, ensure_ld_library_path_with_pre_exec_and_args,
};
pub use error_state::{
    CommandOutcome, command_outcome, command_outcome_tracking_available, global_error_handler_code,
    mark_global_error_handler_initialized, reset_command_error_state, restore_stderr,
    suppress_stderr,
};
pub use init::{initialize_r, initialize_r_for_tests, initialize_r_with_args, run_r_mainloop};
pub use interrupt::{
    ConsoleEvaluationInterruptGuard, InterruptPhase, begin_handler_ipc_execution,
    begin_handler_protected_execution, begin_ipc_execution, begin_ipc_execution_finishing,
    begin_ipc_protected_evaluation, begin_read_console_input, cancel_ipc_deadline,
    enable_ipc_execution_deadline, enable_ipc_silent_boundaries,
    finish_handler_protected_execution, finish_ipc_execution, finish_ipc_execution_outcome,
    finish_ipc_protected_evaluation, is_r_awaiting_console_input, is_r_interrupt_flag_available,
    mark_ipc_interrupt_discarded, mark_ipc_interrupt_observed, process_r_events,
    process_r_events_protected, request_ipc_deadline, request_ipc_interrupt,
    request_ipc_user_code_deadline, set_ipc_execution_phase, set_r_interrupt_pending,
    take_ipc_interrupt_request,
};
pub use output::{
    finish_ipc_capture, flush_reprex_buffer, set_console_output_observer, set_reprex_mode,
    set_write_console_callback, start_ipc_capture,
};
pub use read_console::{
    ConsoleInputBlockGuard, HeadlessReadConsoleCallback, HeadlessReadConsoleResult,
    ReadConsoleInput, ReadConsolePromptInfo, ReadConsoleResult, frame_depth_protected,
    is_browse_prompt, read_console_chunk_end, reset_console_generation,
    set_headless_read_console_callback, set_read_console_callback,
};
pub use spinner::{set_spinner_color, set_spinner_frames, start_spinner, stop_spinner};

#[cfg(test)]
mod test_utils;
