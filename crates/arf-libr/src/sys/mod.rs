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
    ConsoleEvaluationInterruptGuard, is_r_awaiting_console_input, is_r_interrupt_flag_available,
    process_r_events, process_r_events_protected, set_r_interrupt_pending,
};
pub use output::{
    finish_ipc_capture, flush_reprex_buffer, set_reprex_mode, set_write_console_callback,
    start_ipc_capture,
};
pub use read_console::{
    HeadlessReadConsoleCallback, ReadConsoleInput, ReadConsolePromptInfo, frame_depth_protected,
    read_console_chunk_end, reset_console_generation, set_headless_read_console_callback,
    set_read_console_callback,
};
pub use spinner::{set_spinner_color, set_spinner_frames, start_spinner, stop_spinner};

#[cfg(test)]
mod test_utils;
