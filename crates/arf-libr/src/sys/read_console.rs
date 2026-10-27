use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::SexpType;

#[repr(C)]
struct FrameDepthPayload {
    library: *const crate::functions::RLibrary,
    depth: c_int,
    succeeded: bool,
}

/// Return `sys.nframe()` behind an R longjmp boundary.
///
/// This is used by headless ReadConsole to distinguish a real top-level prompt
/// from nested R input requests that happen to reuse the configured prompt.
pub fn frame_depth_protected() -> Result<c_int, String> {
    let lib = crate::r_library().map_err(|error| error.to_string())?;
    let mut payload = FrameDepthPayload {
        library: lib as *const crate::functions::RLibrary,
        depth: -1,
        succeeded: false,
    };
    let completed = unsafe {
        (lib.r_toplevelexec)(
            Some(frame_depth_callback),
            (&mut payload as *mut FrameDepthPayload).cast(),
        )
    } != 0;
    if completed && payload.succeeded && payload.depth >= 0 {
        Ok(payload.depth)
    } else {
        Err("R failed while checking the current evaluation frame".to_string())
    }
}

unsafe extern "C-unwind" fn frame_depth_callback(payload: *mut std::ffi::c_void) {
    let payload = payload.cast::<FrameDepthPayload>();
    let library = unsafe { (*payload).library };
    if library.is_null() {
        return;
    }
    let lib = unsafe { &*library };
    unsafe {
        let function = (lib.rf_install)(c"sys.nframe".as_ptr());
        let call = (lib.rf_protect)((lib.rf_lcons)(function, *lib.r_nilvalue));
        let result = (lib.rf_protect)((lib.rf_eval)(call, *lib.r_baseenv));
        if (lib.rf_typeof)(result) == crate::SexpType::IntSxp as c_int
            && (lib.rf_length)(result) == 1
        {
            (*payload).depth = *(lib.integer)(result);
            (*payload).succeeded = true;
        }
        (lib.rf_unprotect)(2);
    }
}

#[cfg(unix)]
use super::askpass::{ASKPASS_PROMPT_PREFIX, read_password_from_tty, recover_pending_termios};
use super::interrupt::{
    AwaitConsoleInputGuard, begin_read_console_input, finish_read_console_input,
    mark_ipc_interrupt_discarded, take_ipc_interrupt_request,
};
use super::output::REPREX_SETTINGS;
use super::spinner::stop_spinner;

/// Option-derived metadata for one ReadConsole invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadConsolePromptInfo {
    /// Whether the raw prompt exactly matches R's current `options(prompt)` value.
    pub is_main_prompt: bool,
    /// Whether the raw prompt exactly matches R's current `options(continue)` value.
    pub is_continuation: bool,
    /// Whether R's current `options(prompt)` and `options(continue)` values are identical.
    pub options_are_ambiguous: bool,
    /// R's `hist` argument. Zero means this read is nested and must not draw
    /// from the headless request queue.
    pub hist: c_int,
    /// Maximum bytes available for input text, excluding the trailing NUL.
    pub buffer_capacity: usize,
    /// Whether both prompt options were readable, distinct non-empty strings.
    pub prompt_options_are_valid: bool,
}

/// ReadConsole callback receives the raw R prompt and option-derived metadata.
type ReadConsoleCallback = fn(&str, ReadConsolePromptInfo) -> ReadConsoleResult;
pub type HeadlessReadConsoleCallback = fn(&str, ReadConsolePromptInfo) -> Option<ReadConsoleInput>;

/// Result from the interactive ReadConsole callback.
#[derive(Debug)]
pub enum ReadConsoleResult {
    Input(String),
    Interrupt,
    Eof,
}

/// One request-owned chunk for R's ReadConsole buffer.
#[derive(Debug)]
pub struct ReadConsoleInput {
    pub text: String,
    /// True when R should call ReadConsole again for the rest of this line.
    pub has_more: bool,
    /// Return through the interrupt path after the callback's Rust frames end.
    pub interrupt: bool,
}

/// Find the end of the next UTF-8-safe ReadConsole chunk.
///
/// `capacity` is the maximum number of input bytes available in R's buffer,
/// excluding the trailing NUL byte. The final chunk also includes a newline
/// when `text` does not already end with one. Returns the byte index after the
/// chunk and whether more text remains, or `None` when the capacity cannot
/// advance by one complete UTF-8 character. `None` also means the text fits
/// exactly but leaves no room for its terminating newline: in that case this
/// helper leaves a character for the next call so interactive input never
/// needs to queue an empty final chunk. Interactive pending input uses an empty
/// string to mean "no queued input", so it cannot represent that extra call.
/// An empty line itself requires capacity for its newline.
pub fn read_console_chunk_end(text: &str, capacity: usize) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();
    let ends_with_newline = bytes.last() == Some(&b'\n');
    let final_len = bytes.len() + usize::from(!ends_with_newline);

    if final_len <= capacity {
        return Some((bytes.len(), false));
    }

    // If all remaining text would fit but its terminating newline would not,
    // leave at least one byte for the next ReadConsole call.
    let mut max_end = capacity.min(bytes.len());
    if max_end == bytes.len() {
        max_end = max_end.saturating_sub(1);
    }
    while max_end > 0 && !text.is_char_boundary(max_end) {
        max_end -= 1;
    }
    (max_end > 0).then_some((max_end, true))
}

static mut READ_CONSOLE_CALLBACK: Option<ReadConsoleCallback> = None;
static mut HEADLESS_READ_CONSOLE_CALLBACK: Option<HeadlessReadConsoleCallback> = None;
static RESET_CONSOLE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Public R reset callback. Keep this signal handler limited to an atomic
/// update; request state is reconciled from ReadConsole after R regains control.
pub(super) unsafe extern "C" fn r_reset_console() {
    RESET_CONSOLE_GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Return R's reset generation for request-scoped failure accounting.
pub fn reset_console_generation() -> u64 {
    RESET_CONSOLE_GENERATION.load(Ordering::Acquire)
}

const DEFAULT_CONTINUATION_PROMPT: &str = "+ ";

/// Read a scalar string option without evaluating any R code.
fn string_option(option_name: &'static [u8]) -> Option<String> {
    let lib = crate::r_library().ok()?;

    // SAFETY: R is initialized while ReadConsole is invoked. Callers provide a
    // static NUL-terminated option name. The returned SEXP is borrowed from R.
    let option = unsafe {
        let symbol = (lib.rf_install)(option_name.as_ptr() as *const c_char);
        (lib.rf_get_option1)(symbol)
    };

    if option.is_null() {
        return None;
    }

    // Prompt options must be character vectors with exactly one element. Treat
    // all other values as malformed.
    let is_string = unsafe { (lib.rf_typeof)(option) == SexpType::StrSxp as c_int };
    if !is_string || unsafe { (lib.rf_length)(option) } != 1 {
        return None;
    }

    // SAFETY: The type and length were checked above. R's STRING_ELT and R_CHAR
    // accessors return borrowed objects valid for this ReadConsole invocation.
    let chars = unsafe {
        let element = (lib.string_elt)(option, 0);
        (lib.r_charsxp)(element)
    };
    if chars.is_null() {
        return None;
    }

    // R strings are not necessarily valid UTF-8. The console callback accepts
    // UTF-8, so reject malformed values rather than panicking.
    unsafe { CStr::from_ptr(chars).to_str().ok().map(str::to_owned) }
}

fn prompt_info_from_options(
    raw_prompt: &str,
    main_prompt: Option<&str>,
    continuation_prompt: Option<&str>,
) -> ReadConsolePromptInfo {
    let prompt_options_are_valid = matches!(
        (main_prompt, continuation_prompt),
        (Some(main), Some(continuation))
            if !main.is_empty()
                && !continuation.is_empty()
                && main != continuation
                && !is_browse_prompt(main)
                && !is_browse_prompt(continuation)
    );
    let options_are_ambiguous = matches!(
        (main_prompt, continuation_prompt),
        (Some(main), Some(continuation)) if main == continuation
    );
    let continuation_prompt = continuation_prompt.unwrap_or(DEFAULT_CONTINUATION_PROMPT);

    ReadConsolePromptInfo {
        is_main_prompt: main_prompt.is_some_and(|main| raw_prompt == main),
        is_continuation: raw_prompt == continuation_prompt,
        options_are_ambiguous,
        hist: 1,
        buffer_capacity: 0,
        prompt_options_are_valid,
    }
}

/// Inspect the prompt options for one ReadConsole invocation.
///
/// This is intentionally performed for every invocation because R code may
/// change either option during a session. Values are not cached.
fn read_console_prompt_info(raw_prompt: &str) -> ReadConsolePromptInfo {
    let main_prompt = string_option(b"prompt\0");
    let continuation_prompt = string_option(b"continue\0");
    prompt_info_from_options(
        raw_prompt,
        main_prompt.as_deref(),
        continuation_prompt.as_deref(),
    )
}

/// Buffer for input that exceeds R's buffer size.
/// When input is longer than buflen, the remainder is stored here
/// and returned on subsequent ReadConsole calls.
///
/// Note: This is accessed only from R's main thread in the ReadConsole callback.
struct PendingConsoleInput {
    text: String,
    depth: c_int,
    nested_prompt: bool,
    generation: u64,
}

static PENDING_INPUT: std::sync::Mutex<Vec<PendingConsoleInput>> =
    std::sync::Mutex::new(Vec::new());

/// R's ReadConsole callback.
///
/// # Safety
/// This function is called by R and must match the expected signature.
pub(super) unsafe extern "C-unwind" fn r_read_console(
    prompt: *const c_char,
    buf: *mut c_char,
    buflen: c_int,
    hist: c_int,
) -> c_int {
    // Keep the full callback implementation, including its owned values and
    // input-wait guard, in a helper frame. Rf_onintr() can longjmp to an
    // enclosing R context, so it must run only after that frame has returned
    // and dropped all Rust values with destructors.
    let mut previous_execution = begin_read_console_input();
    loop {
        let result = unsafe { r_read_console_inner(prompt, buf, buflen, hist) };
        if result != READ_CONSOLE_INTERRUPT {
            finish_read_console_input(previous_execution, result == 1);
            return result;
        }
        let discarded_input = {
            let mut pending = PENDING_INPUT.lock().unwrap();
            let discarded = !pending.is_empty();
            pending.clear();
            discarded
        };
        if discarded_input {
            mark_ipc_interrupt_discarded();
        }
        // Rf_onintr() may longjmp out of this frame when a handler exits, and
        // the code that continues from there never reaches the normal return
        // path above. Leave the input phase before handing control to R, so
        // later IPC interrupt requests reach R's flag while the evaluation runs.
        finish_read_console_input(previous_execution, false);
        let Ok(library) = crate::r_library() else {
            return 0;
        };
        // R's standard terminal enables interrupts while reading console
        // input, even inside suspendInterrupts(). Do the same only here,
        // after the Rust input frames have returned. R restores its context's
        // suspension state on a jump; if a handler resumes, restore it here.
        let suspended = library.r_interrupts_suspended;
        let previous = if suspended.is_null() {
            0
        } else {
            unsafe {
                let previous = std::ptr::read_volatile(suspended);
                std::ptr::write_volatile(suspended, 0);
                previous
            }
        };
        unsafe { (library.rf_onintr)() };
        if !suspended.is_null() {
            unsafe { std::ptr::write_volatile(suspended, previous) };
        }
        // An interrupt handler can invoke the resume restart. Read again
        // rather than submitting EOF or an empty browser step to R, and
        // re-enter the input phase for that read.
        previous_execution = begin_read_console_input();
    }
}

const READ_CONSOLE_INTERRUPT: c_int = 2;

unsafe fn r_read_console_inner(
    prompt: *const c_char,
    buf: *mut c_char,
    buflen: c_int,
    hist: c_int,
) -> c_int {
    log::info!("r_read_console: called with buflen={}", buflen);

    // Stop the spinner when a new prompt is displayed
    // This handles cases where R finishes evaluation without producing output
    stop_spinner();

    // Clear any pending interrupt flag at the start of every ReadConsole
    // invocation (including nested prompts such as readline(), browser(), etc.).
    // This prevents stale Ctrl+C signals from interrupting the next input read.

    // Safety net: if a previous password read (via rpassword) was interrupted
    // by longjmp (SIGINT), the terminal settings snapshot stored in
    // PENDING_TERMIOS_RESTORE may still need to be reapplied. Recover here at
    // the earliest safe point by restoring any pending termios state.
    #[cfg(unix)]
    recover_pending_termios();

    // Read the prompt options once per invocation and share the resulting
    // metadata with both the reprex bookkeeping and high-level callback.
    let prompt_str: &str = if prompt.is_null() {
        ""
    } else {
        // SAFETY: prompt is a valid C string from R.
        unsafe { CStr::from_ptr(prompt) }
            .to_str()
            .unwrap_or_default()
    };
    let prompt_info = ReadConsolePromptInfo {
        hist,
        buffer_capacity: (buflen as usize).saturating_sub(1),
        ..read_console_prompt_info(prompt_str)
    };

    // Askpass mode: detect magic prefix, strip it, read from /dev/tty with echo disabled.
    // This runs before the input-wait guard on purpose: no Rust loop pumps R
    // events during the blocking tty read, so the longjmp hazard the guard
    // prevents does not exist here, and dropping SIGINT would make Ctrl+C at
    // a password prompt entirely inert. With the flag allowed through,
    // SA_RESTART resumes the read and the pending interrupt cancels the
    // requesting operation as soon as the read returns.
    #[cfg(unix)]
    let headless_callback_installed = unsafe {
        std::ptr::addr_of!(HEADLESS_READ_CONSOLE_CALLBACK)
            .read()
            .is_some()
    };
    #[cfg(unix)]
    if !prompt.is_null() && !headless_callback_installed {
        let prompt_bytes = unsafe { std::ffi::CStr::from_ptr(prompt) }.to_bytes();
        if prompt_bytes.starts_with(ASKPASS_PROMPT_PREFIX) {
            let real_prompt = unsafe { prompt.add(ASKPASS_PROMPT_PREFIX.len()) };
            return unsafe { read_password_from_tty(real_prompt, buf, buflen) };
        }
    }

    // Mark that R is waiting for console input until this call returns, so
    // the Ctrl+C handler drops interrupts instead of setting R's flag while
    // Rust input loops are pumping R events (see R_AWAITING_CONSOLE_INPUT).
    let _await_input_guard = AwaitConsoleInputGuard::new();

    // In reprex mode, print a blank line between expressions for readability
    // Only print for main prompts (not continuation prompts like "+")
    if let Ok(mut settings) = REPREX_SETTINGS.write()
        && settings.enabled
        && settings.had_output
    {
        // Check if this is a main prompt (not continuation). Use the same
        // option-derived classification passed to the high-level callback.
        let is_main_prompt = !prompt_info.is_continuation && !prompt_str.trim().is_empty();

        if is_main_prompt {
            println!();
            settings.had_output = false;
        }
    }

    // A headless callback owns its complete request buffer. Invoke it before
    // the interactive shared remainder so nested prompts cannot consume it.
    if let Some(callback) = unsafe { HEADLESS_READ_CONSOLE_CALLBACK } {
        let Some(input) = callback(prompt_str, prompt_info) else {
            return 0;
        };
        if input.interrupt {
            return READ_CONSOLE_INTERRUPT;
        }
        return unsafe { write_read_console_input(input, buf, buflen) };
    }

    // Nested input owns its own chunks: an outer request's remaining code
    // must never become the answer to readline() or a browser command.
    let input_depth = frame_depth_protected().ok();
    let nested_prompt = hist == 0 || is_browse_prompt(prompt_str);
    let buffered = {
        let mut pending = PENDING_INPUT.lock().unwrap();
        let generation = reset_console_generation();
        pending.retain(|input| input.generation == generation);
        let index = pending.iter().position(|input| {
            Some(input.depth) == input_depth && input.nested_prompt == nested_prompt
        });
        if let Some(index) = index {
            if take_ipc_interrupt_request() {
                pending.clear();
                mark_ipc_interrupt_discarded();
                return READ_CONSOLE_INTERRUPT;
            }
            Some(pending.remove(index).text)
        } else {
            None
        }
    };
    // Release the pending-input lock before invoking any callback or R code.
    let input = match buffered {
        Some(input) => input,
        None => {
            log::debug!("r_read_console: prompt={:?}", prompt_str);

            // Call the callback to get new input
            // SAFETY: READ_CONSOLE_CALLBACK is only accessed from this single-threaded context
            if let Some(callback) = unsafe { READ_CONSOLE_CALLBACK } {
                log::debug!("r_read_console: calling callback");
                match callback(prompt_str, prompt_info) {
                    ReadConsoleResult::Input(s) => {
                        log::debug!("r_read_console: got input len={}", s.len());
                        s
                    }
                    ReadConsoleResult::Interrupt => return READ_CONSOLE_INTERRUPT,
                    ReadConsoleResult::Eof => {
                        log::debug!("r_read_console: callback returned None (EOF)");
                        return 0; // EOF
                    }
                }
            } else {
                log::debug!("r_read_console: no callback set, returning 0");
                return 0; // No callback set
            }
        }
    };

    let Some((chunk_end, has_more)) = read_console_chunk_end(&input, prompt_info.buffer_capacity)
    else {
        log::error!(
            "r_read_console: buffer capacity {} cannot hold input chunk",
            prompt_info.buffer_capacity
        );
        return 0;
    };
    let result = unsafe {
        write_read_console_input(
            ReadConsoleInput {
                text: input[..chunk_end].to_owned(),
                has_more,
                interrupt: false,
            },
            buf,
            buflen,
        )
    };
    if result == 1 && has_more {
        let Some(depth) = input_depth else {
            log::error!("Cannot retain console input without a verified R frame depth");
            return 0;
        };
        let mut pending = PENDING_INPUT.lock().unwrap();
        pending.push(PendingConsoleInput {
            text: input[chunk_end..].to_owned(),
            depth,
            nested_prompt,
            generation: reset_console_generation(),
        });
    }
    result
}

unsafe fn write_read_console_input(
    input: ReadConsoleInput,
    buf: *mut c_char,
    buflen: c_int,
) -> c_int {
    if buf.is_null() || buflen <= 0 {
        return 0;
    }
    let bytes = input.text.as_bytes();
    let adds_newline = !input.has_more && (bytes.is_empty() || bytes.last() != Some(&b'\n'));
    let written_len = bytes.len() + if adds_newline { 1 } else { 0 };
    if written_len >= buflen as usize {
        return 0;
    }
    unsafe {
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), bytes.len());
        }
        let mut pos = bytes.len();
        if adds_newline {
            *buf.add(pos) = b'\n' as c_char;
            pos += 1;
        }
        *buf.add(pos) = 0;
    }
    1
}

/// Whether a raw prompt is one of R's browser prompts (`Browse[n]>`).
pub fn is_browse_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim_end();
    let Some(number) = prompt
        .strip_prefix("Browse[")
        .and_then(|tail| tail.strip_suffix("]>"))
    else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Set the console read callback.
///
/// The callback receives the raw R prompt and option-derived metadata. The
/// current `options(prompt)` and `options(continue)` values are read once for
/// each ReadConsole invocation before the callback is called. The callback
/// should return input, an interrupt, or EOF (exit R).
pub fn set_read_console_callback(callback: ReadConsoleCallback) {
    unsafe {
        READ_CONSOLE_CALLBACK = Some(callback);
    }
}

/// Install a request-owned callback for headless mainloop input.
pub fn set_headless_read_console_callback(callback: HeadlessReadConsoleCallback) {
    unsafe {
        HEADLESS_READ_CONSOLE_CALLBACK = Some(callback);
        READ_CONSOLE_CALLBACK = None;
        PENDING_INPUT.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_boundaries_preserve_utf8_and_reconstruct_the_input() {
        let input = "aé🙂z\n";
        let mut remainder = input;
        let mut reconstructed = String::new();

        loop {
            let (end, has_more) = read_console_chunk_end(remainder, 4).unwrap();
            assert!(remainder.is_char_boundary(end));
            reconstructed.push_str(&remainder[..end]);
            if !has_more {
                break;
            }
            assert!(end > 0);
            remainder = &remainder[end..];
        }

        assert_eq!(reconstructed, input);
    }

    #[test]
    fn final_chunk_accounts_for_existing_or_added_newline() {
        assert_eq!(read_console_chunk_end("abc\n", 4), Some((4, false)));
        assert_eq!(read_console_chunk_end("abc", 4), Some((3, false)));
        assert_eq!(read_console_chunk_end("", 1), Some((0, false)));
        assert_eq!(read_console_chunk_end("", 0), None);
    }

    #[test]
    fn full_capacity_without_newline_is_split_to_make_room_for_terminator() {
        assert_eq!(read_console_chunk_end("abc", 3), Some((2, true)));
        assert_eq!(read_console_chunk_end("é", 1), None);
        assert_eq!(read_console_chunk_end("é", 2), None);
        assert_eq!(read_console_chunk_end("a", 1), None);
        assert_eq!(read_console_chunk_end("a", 2), Some((1, false)));
    }

    #[test]
    fn complete_input_requires_room_for_newline_and_nul() {
        let mut buffer = [0xA5_u8; 4];
        let result = unsafe {
            write_read_console_input(
                ReadConsoleInput {
                    text: "abc".to_string(),
                    has_more: false,
                    interrupt: false,
                },
                buffer.as_mut_ptr().cast(),
                buffer.len() as c_int,
            )
        };

        assert_eq!(result, 0);
        assert_eq!(buffer, [0xA5; 4]);
    }

    #[test]
    fn nonfinal_input_can_fill_capacity_before_nul() {
        let mut buffer = [0xA5_u8; 4];
        let result = unsafe {
            write_read_console_input(
                ReadConsoleInput {
                    text: "abc".to_string(),
                    has_more: true,
                    interrupt: false,
                },
                buffer.as_mut_ptr().cast(),
                buffer.len() as c_int,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(buffer, [b'a', b'b', b'c', 0]);
    }

    #[test]
    fn prompt_info_detects_ambiguity_without_changing_exact_classification() {
        let info = prompt_info_from_options("same> ", Some("same> "), Some("same> "));

        assert!(info.is_continuation);
        assert!(info.options_are_ambiguous);
    }

    #[test]
    fn prompt_info_matches_only_the_current_continuation_option() {
        let info = prompt_info_from_options("... ", Some("> "), Some("... "));

        assert!(info.is_continuation);
        assert!(!info.options_are_ambiguous);

        let main_prompt = prompt_info_from_options("> ", Some("> "), Some("... "));
        assert!(!main_prompt.is_continuation);
        assert!(!main_prompt.options_are_ambiguous);
    }

    #[test]
    fn prompt_info_uses_default_continuation_only_when_option_is_unavailable() {
        let fallback = prompt_info_from_options("+ ", Some("> "), None);
        let custom = prompt_info_from_options("+ ", Some("> "), Some("... "));

        assert!(fallback.is_continuation);
        assert!(!fallback.options_are_ambiguous);
        assert!(!custom.is_continuation);
    }

    #[test]
    fn prompt_info_does_not_mark_malformed_options_as_ambiguous() {
        let missing_main = prompt_info_from_options("+ ", None, Some("+ "));
        let missing_continue = prompt_info_from_options("+ ", Some("+ "), None);

        assert!(!missing_main.options_are_ambiguous);
        assert!(!missing_continue.options_are_ambiguous);
    }

    #[test]
    fn prompt_options_cannot_reuse_the_browse_prompt_pattern() {
        let browse_main = prompt_info_from_options("> ", Some("Browse[12]> "), Some("+ "));
        let browse_continue = prompt_info_from_options("> ", Some("> "), Some("Browse[12]> "));

        assert!(!browse_main.prompt_options_are_valid);
        assert!(!browse_continue.prompt_options_are_valid);
    }
}
