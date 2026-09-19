//! Linux/R 4.5.2 diagnostic for protected non-local transfer through the native mainloop.
//!
//! The parent mode never initializes R. It runs this executable again in a child,
//! then checks the child's exit status, timeout, and fixed marker sequence. The
//! child mode installs a POD ReadConsole callback and calls `run_Rmainloop`
//! directly. Setup uses `arf_harp::eval_string` before the callback is installed
//! and a minimal `R_ToplevelExec` only around `R_PreserveObject`; the probe
//! actions themselves are not wrapped in `R_ToplevelExec` or `R_tryEval`, and
//! the host does not use `R_ReplDLLdo1`.
//!
//! Each action is evaluated and explicitly printed inside one `R_UnwindProtect`
//! call whose continuation argument is C NULL. Its cleanup callback records only
//! integer state and events; the parent observes those events at later
//! ReadConsole entries. Successful actions return a blank line to the native
//! mainloop to make it request input again. Errors instead propagate to R's
//! `run_Rmainloop`/top-level recovery path, which must call ReadConsole again.
//!
//! The expression list is rooted with `R_PreserveObject`; an outer Rust
//! protection guard is not a persistent root because native REPL iterations
//! restore R's protection stack. ReadConsole, unwind body, cleanup, and helper
//! frames that may be skipped by R's longjmp contain only POD/raw values.
//!
//! This is a Linux/R 4.5.2 diagnostic, not evidence that the product REPL
//! outcome model is implemented. The callback supports only the top-level
//! `"> "` prompt and fails closed on nested or continuation prompts. The blank
//! line used after success is a driving technique, not native user-input
//! equivalence. Native autoprint, task callbacks, `.Last.value`, browser,
//! recover, interrupts, Windows, and product integration are not tested. The
//! parent uses a fixed 90-second timeout for this fixture. Marker writes use a
//! small low-level loop without special EINTR handling; any lost/partial marker
//! makes the parent's exact-sequence check fail closed.

#[cfg(target_os = "linux")]
mod linux {
    use arf_harp::eval_string;
    use arf_libr::{SEXP, r_library};
    use libloading::Library;
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::io::Read;
    use std::mem::MaybeUninit;
    use std::os::raw::{c_char, c_int};
    use std::process::{Command, Stdio};
    use std::ptr;
    use std::thread;
    use std::time::{Duration, Instant};

    type RUnwindBody = unsafe extern "C" fn(*mut c_void) -> SEXP;
    type RUnwindCleanup = unsafe extern "C" fn(*mut c_void, c_int);
    type RUnwindProtect = unsafe extern "C" fn(
        Option<RUnwindBody>,
        *mut c_void,
        Option<RUnwindCleanup>,
        *mut c_void,
        SEXP,
    ) -> SEXP;
    type PreserveObject = unsafe extern "C" fn(SEXP);

    #[repr(C)]
    struct PreserveData {
        preserve: PreserveObject,
        object: SEXP,
    }

    const EVENT_CAPACITY: usize = 64;
    const ACTION_COUNT: c_int = 3;
    const EVENT_READ: c_int = 1;
    const EVENT_BODY: c_int = 2;
    const EVENT_CLEANUP_JUMP: c_int = 3;
    const EVENT_CLEANUP_NORMAL: c_int = 4;
    const EVENT_CALL_RETURNED: c_int = 5;
    const EVENT_EOF: c_int = 6;
    const EVENT_PRE_BODY_FAILURE: c_int = 7;
    const EVENT_REENTRY: c_int = 8;
    const EVENT_OVERFLOW: c_int = 9;
    const EVENT_UNEXPECTED_PROMPT: c_int = 10;
    const EVENT_INVALID_STATE: c_int = 11;
    const EVENT_EVAL_RETURNED: c_int = 12;
    const EVENT_PRINT_STARTED: c_int = 13;
    const EVENT_PRINT_RETURNED: c_int = 14;

    const MARKER_READ: &[u8] = b"ARF_MAINLOOP_SPIKE:READ\n";
    const MARKER_BODY: &[u8] = b"ARF_MAINLOOP_SPIKE:BODY\n";
    const MARKER_CLEANUP_JUMP: &[u8] = b"ARF_MAINLOOP_SPIKE:CLEANUP_JUMP\n";
    const MARKER_CLEANUP_NORMAL: &[u8] = b"ARF_MAINLOOP_SPIKE:CLEANUP_NORMAL\n";
    const MARKER_CALL_RETURNED: &[u8] = b"ARF_MAINLOOP_SPIKE:CALL_RETURNED\n";
    const MARKER_EOF: &[u8] = b"ARF_MAINLOOP_SPIKE:EOF\n";
    const MARKER_PRE_BODY_FAILURE: &[u8] = b"ARF_MAINLOOP_SPIKE:PRE_BODY_FAILURE\n";
    const MARKER_REENTRY: &[u8] = b"ARF_MAINLOOP_SPIKE:REENTRY\n";
    const MARKER_OVERFLOW: &[u8] = b"ARF_MAINLOOP_SPIKE:EVENT_OVERFLOW\n";
    const MARKER_UNEXPECTED_PROMPT: &[u8] = b"ARF_MAINLOOP_SPIKE:UNEXPECTED_PROMPT\n";
    const MARKER_INVALID_STATE: &[u8] = b"ARF_MAINLOOP_SPIKE:INVALID_STATE\n";
    const MARKER_EVAL_RETURNED: &[u8] = b"ARF_MAINLOOP_SPIKE:EVAL_RETURNED\n";
    const MARKER_PRINT_STARTED: &[u8] = b"ARF_MAINLOOP_SPIKE:PRINT_STARTED\n";
    const MARKER_PRINT_RETURNED: &[u8] = b"ARF_MAINLOOP_SPIKE:PRINT_RETURNED\n";

    #[derive(Clone, Copy)]
    struct ProbeApi {
        eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
        protect: unsafe extern "C" fn(SEXP) -> SEXP,
        unprotect: unsafe extern "C" fn(c_int),
        print_value: unsafe extern "C" fn(SEXP),
        vector_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,
        unwind_protect: RUnwindProtect,
        global_env: SEXP,
        nil_value: SEXP,
        expressions: SEXP,
    }

    struct ProbeApiCell(UnsafeCell<MaybeUninit<ProbeApi>>);

    // The independent host is single-threaded: R initializes this once before
    // entering run_Rmainloop, and only its main thread reads it afterwards.
    unsafe impl Sync for ProbeApiCell {}

    static PROBE_API: ProbeApiCell = ProbeApiCell(UnsafeCell::new(MaybeUninit::uninit()));

    #[repr(C)]
    struct ProbeState {
        next_action: c_int,
        armed: c_int,
        body_entered: c_int,
        cleanup_seen: c_int,
        cleanup_jump: c_int,
        call_returned: c_int,
        callback_active: c_int,
        failed: c_int,
        event_len: usize,
        events: [c_int; EVENT_CAPACITY],
    }

    impl ProbeState {
        const fn new() -> Self {
            Self {
                next_action: 0,
                armed: 0,
                body_entered: 0,
                cleanup_seen: 0,
                cleanup_jump: 0,
                call_returned: 0,
                callback_active: 0,
                failed: 0,
                event_len: 0,
                events: [0; EVENT_CAPACITY],
            }
        }
    }

    static mut PROBE_STATE: ProbeState = ProbeState::new();

    #[repr(C)]
    struct UnwindData {
        expression: SEXP,
        environment: SEXP,
        nil_value: SEXP,
        eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
        protect: unsafe extern "C" fn(SEXP) -> SEXP,
        unprotect: unsafe extern "C" fn(c_int),
        print_value: unsafe extern "C" fn(SEXP),
    }

    unsafe extern "C" {
        fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    }

    pub fn main() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::args().nth(1).as_deref() == Some("--child") {
            child_main()?;
            return Ok(());
        }

        parent_main()
    }

    fn parent_main() -> Result<(), Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        let mut child = Command::new(executable)
            .arg("--child")
            .env("R_DEFAULT_PACKAGES", "NULL")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().ok_or("child stdout was not piped")?;
        let stderr = child.stderr.take().ok_or("child stderr was not piped")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));

        let deadline = Instant::now() + Duration::from_secs(90);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(20));
        };

        let child_stdout = stdout_reader
            .join()
            .map_err(|_| std::io::Error::other("stdout reader panicked"))??;
        let child_stderr = stderr_reader
            .join()
            .map_err(|_| std::io::Error::other("stderr reader panicked"))??;
        let stdout_text = String::from_utf8_lossy(&child_stdout);
        let stderr_text = String::from_utf8_lossy(&child_stderr);

        if let Some(status) = status {
            if !status.success() {
                return Err(format!("child exited with {status}\n{stderr_text}").into());
            }
        } else {
            return Err(format!("child exceeded the 90-second timeout\n{stderr_text}").into());
        }

        let expected = [
            "ARF_MAINLOOP_SPIKE:READ",
            "ARF_MAINLOOP_SPIKE:BODY",
            "ARF_MAINLOOP_SPIKE:CLEANUP_JUMP",
            "ARF_MAINLOOP_SPIKE:READ",
            "ARF_MAINLOOP_SPIKE:BODY",
            "ARF_MAINLOOP_SPIKE:EVAL_RETURNED",
            "ARF_MAINLOOP_SPIKE:PRINT_STARTED",
            "ARF_MAINLOOP_SPIKE:CLEANUP_JUMP",
            "ARF_MAINLOOP_SPIKE:READ",
            "ARF_MAINLOOP_SPIKE:BODY",
            "ARF_MAINLOOP_SPIKE:EVAL_RETURNED",
            "ARF_MAINLOOP_SPIKE:PRINT_STARTED",
            "ARF_MAINLOOP_SPIKE:PRINT_RETURNED",
            "ARF_MAINLOOP_SPIKE:CLEANUP_NORMAL",
            "ARF_MAINLOOP_SPIKE:CALL_RETURNED",
            "ARF_MAINLOOP_SPIKE:READ",
            "ARF_MAINLOOP_SPIKE:EOF",
        ];
        let actual: Vec<&str> = stdout_text
            .lines()
            .filter(|line| line.starts_with("ARF_MAINLOOP_SPIKE:"))
            .collect();
        if actual != expected {
            return Err(format!(
                "child marker sequence differed\nexpected: {expected:?}\nactual:   {actual:?}\nstdout:\n{stdout_text}\nstderr:\n{stderr_text}"
            )
            .into());
        }

        print!("{stdout_text}");
        eprint!("{stderr_text}");
        println!("mainloop unwind propagation/recovery probe passed");
        Ok(())
    }

    fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn child_main() -> Result<(), Box<dyn std::error::Error>> {
        // Keep this diagnostic limited to base R and the installed Linux/R 4.5.2 host.
        unsafe {
            std::env::set_var("R_DEFAULT_PACKAGES", "NULL");
            arf_libr::initialize_r_with_args(&[
                "--vanilla",
                "--no-save",
                "--quiet",
                "--interactive",
            ])?;
        }

        let api = r_library()?;
        let library = unsafe { Library::new(arf_libr::find_r_library()?)? };
        let preserve_object = unsafe { *library.get::<PreserveObject>(b"R_PreserveObject\0")? };
        let unwind_protect = unsafe { *library.get::<RUnwindProtect>(b"R_UnwindProtect\0")? };
        let global_env = unsafe { *api.r_globalenv };
        let nil_value = unsafe { *api.r_nilvalue };

        let script = eval_string(
            r#"
.arf_mainloop_unwind_print_abort <- structure(
    1L,
    class = "arf_mainloop_unwind_print_abort"
)
print.arf_mainloop_unwind_print_abort <- function(x, ...) {
    stop("intentional S3 print abort")
}
.arf_mainloop_unwind_expressions <- list(
    quote(stop("intentional evaluation abort")),
    quote(.arf_mainloop_unwind_print_abort),
    quote(42L)
)
invisible(.arf_mainloop_unwind_expressions)
"#,
        )?;
        let expressions = script.sexp();
        let mut preserve_data = PreserveData {
            preserve: preserve_object,
            object: expressions,
        };
        let preserve_succeeded = unsafe {
            (api.r_toplevelexec)(
                Some(preserve_object_callback),
                ptr::addr_of_mut!(preserve_data).cast::<c_void>(),
            )
        };
        if preserve_succeeded == 0 {
            return Err("R_PreserveObject failed inside R_ToplevelExec".into());
        }
        drop(script);

        let probe_api = ProbeApi {
            eval: api.rf_eval,
            protect: api.rf_protect,
            unprotect: api.rf_unprotect,
            print_value: api.rf_printvalue,
            vector_elt: api.vector_elt,
            unwind_protect,
            global_env,
            nil_value,
            expressions,
        };
        unsafe {
            ptr::write((*PROBE_API.0.get()).as_mut_ptr(), probe_api);
            ptr::write(ptr::addr_of_mut!(PROBE_STATE), ProbeState::new());
            if api.ptr_r_readconsole.is_null() {
                return Err("R export ptr_R_ReadConsole was null".into());
            }
            *api.ptr_r_readconsole = Some(read_console);
            // initialize_r_with_args set up R's mainloop already. Enter the
            // native loop directly instead of arf's product wrapper.
            (api.run_rmainloop)();
        }

        // Some embedded R builds return after EOF; the parent validates the
        // marker sequence and status and does not rely on child-side Drop/asserts.
        Ok(())
    }

    unsafe extern "C" fn read_console(
        prompt: *const c_char,
        buffer: *mut c_char,
        buffer_len: c_int,
        _history: c_int,
    ) -> c_int {
        unsafe {
            if read_state(ptr::addr_of_mut!(PROBE_STATE.callback_active)) != 0 {
                let event = if read_state(ptr::addr_of_mut!(PROBE_STATE.body_entered)) == 0 {
                    EVENT_PRE_BODY_FAILURE
                } else {
                    EVENT_REENTRY
                };
                append_event(event);
                write_pending_events();
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                return 0;
            }

            write_pending_events();
            append_event(EVENT_READ);

            if !is_top_level_prompt(prompt) {
                append_event(EVENT_UNEXPECTED_PROMPT);
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                write_pending_events();
                return 0;
            }

            if read_state(ptr::addr_of_mut!(PROBE_STATE.armed)) != 0 {
                let body_entered = read_state(ptr::addr_of_mut!(PROBE_STATE.body_entered));
                let cleanup_seen = read_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_seen));
                let cleanup_jump = read_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_jump));
                let call_returned = read_state(ptr::addr_of_mut!(PROBE_STATE.call_returned));

                if body_entered == 0 || cleanup_seen == 0 {
                    append_event(EVENT_PRE_BODY_FAILURE);
                    write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                } else if cleanup_jump != 0 {
                    if call_returned != 0 {
                        append_event(EVENT_INVALID_STATE);
                        write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                    }
                } else if call_returned == 0 {
                    append_event(EVENT_INVALID_STATE);
                    write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                }

                write_state(ptr::addr_of_mut!(PROBE_STATE.armed), 0);
            }

            if read_state(ptr::addr_of_mut!(PROBE_STATE.failed)) != 0 {
                write_pending_events();
                return 0;
            }

            let action = read_state(ptr::addr_of_mut!(PROBE_STATE.next_action));
            if action >= ACTION_COUNT {
                append_event(EVENT_EOF);
                write_pending_events();
                return 0;
            }

            if buffer.is_null() || buffer_len < 2 {
                append_event(EVENT_INVALID_STATE);
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                write_pending_events();
                return 0;
            }

            let api = ptr::read((*PROBE_API.0.get()).as_ptr());
            let expression = (api.vector_elt)(api.expressions, action as isize);
            let mut unwind_data = UnwindData {
                expression,
                environment: api.global_env,
                nil_value: api.nil_value,
                eval: api.eval,
                protect: api.protect,
                unprotect: api.unprotect,
                print_value: api.print_value,
            };

            // Advance the action before entering R. If setup fails before body
            // or cleanup, the next ReadConsole entry observes the armed state.
            write_state(ptr::addr_of_mut!(PROBE_STATE.next_action), action + 1);
            write_state(ptr::addr_of_mut!(PROBE_STATE.armed), 1);
            write_state(ptr::addr_of_mut!(PROBE_STATE.body_entered), 0);
            write_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_seen), 0);
            write_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_jump), 0);
            write_state(ptr::addr_of_mut!(PROBE_STATE.call_returned), 0);
            write_state(ptr::addr_of_mut!(PROBE_STATE.callback_active), 1);

            (api.unwind_protect)(
                Some(unwind_body),
                ptr::addr_of_mut!(unwind_data).cast::<c_void>(),
                Some(unwind_cleanup),
                ptr::addr_of_mut!(unwind_data).cast::<c_void>(),
                ptr::null_mut(),
            );

            write_state(ptr::addr_of_mut!(PROBE_STATE.call_returned), 1);
            append_event(EVENT_CALL_RETURNED);
            buffer.write(b'\n' as c_char);
            buffer.add(1).write(0);
            1
        }
    }

    unsafe extern "C" fn preserve_object_callback(data: *mut c_void) {
        unsafe {
            let payload = data.cast::<PreserveData>();
            ((*payload).preserve)((*payload).object);
        }
    }

    unsafe extern "C" fn unwind_body(data: *mut c_void) -> SEXP {
        unsafe {
            if read_state(ptr::addr_of_mut!(PROBE_STATE.armed)) == 0
                || read_state(ptr::addr_of_mut!(PROBE_STATE.body_entered)) != 0
            {
                append_event(EVENT_REENTRY);
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                let api = ptr::read((*PROBE_API.0.get()).as_ptr());
                return api.nil_value;
            }

            write_state(ptr::addr_of_mut!(PROBE_STATE.body_entered), 1);
            append_event(EVENT_BODY);

            let unwind_data = data.cast::<UnwindData>();
            let result =
                ((*unwind_data).eval)((*unwind_data).expression, (*unwind_data).environment);
            // Record eval return before protecting the value, then record print
            // start only after PROTECT and immediately before explicit printing.
            append_event(EVENT_EVAL_RETURNED);
            let value = ((*unwind_data).protect)(result);
            append_event(EVENT_PRINT_STARTED);
            ((*unwind_data).print_value)(value);
            append_event(EVENT_PRINT_RETURNED);
            ((*unwind_data).unprotect)(1);
            (*unwind_data).nil_value
        }
    }

    unsafe extern "C" fn unwind_cleanup(_data: *mut c_void, jump: c_int) {
        unsafe {
            if read_state(ptr::addr_of_mut!(PROBE_STATE.armed)) == 0
                || read_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_seen)) != 0
            {
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                append_event(EVENT_INVALID_STATE);
            }
            write_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_seen), 1);
            write_state(ptr::addr_of_mut!(PROBE_STATE.cleanup_jump), jump);
            write_state(ptr::addr_of_mut!(PROBE_STATE.callback_active), 0);
            if jump == 0 {
                append_event(EVENT_CLEANUP_NORMAL);
            } else {
                append_event(EVENT_CLEANUP_JUMP);
            }
        }
    }

    unsafe fn is_top_level_prompt(prompt: *const c_char) -> bool {
        if prompt.is_null() {
            return false;
        }
        unsafe {
            prompt.read() == b'>' as c_char
                && prompt.add(1).read() == b' ' as c_char
                && prompt.add(2).read() == 0
        }
    }

    unsafe fn read_state(field: *mut c_int) -> c_int {
        unsafe { ptr::read_volatile(field) }
    }

    unsafe fn write_state(field: *mut c_int, value: c_int) {
        unsafe { ptr::write_volatile(field, value) };
    }

    unsafe fn append_event(event: c_int) {
        unsafe {
            let len_ptr = ptr::addr_of_mut!(PROBE_STATE.event_len);
            let len = ptr::read_volatile(len_ptr);
            let events_ptr = ptr::addr_of_mut!(PROBE_STATE.events).cast::<c_int>();
            if len >= EVENT_CAPACITY - 1 {
                ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                if len < EVENT_CAPACITY {
                    ptr::write_volatile(events_ptr.add(len), EVENT_OVERFLOW);
                    ptr::write_volatile(len_ptr, len + 1);
                }
                return;
            }
            ptr::write_volatile(events_ptr.add(len), event);
            ptr::write_volatile(len_ptr, len + 1);
        }
    }

    unsafe fn event_marker(event: c_int) -> (*const u8, usize) {
        let marker = match event {
            EVENT_READ => MARKER_READ,
            EVENT_BODY => MARKER_BODY,
            EVENT_CLEANUP_JUMP => MARKER_CLEANUP_JUMP,
            EVENT_CLEANUP_NORMAL => MARKER_CLEANUP_NORMAL,
            EVENT_CALL_RETURNED => MARKER_CALL_RETURNED,
            EVENT_EOF => MARKER_EOF,
            EVENT_PRE_BODY_FAILURE => MARKER_PRE_BODY_FAILURE,
            EVENT_REENTRY => MARKER_REENTRY,
            EVENT_OVERFLOW => MARKER_OVERFLOW,
            EVENT_UNEXPECTED_PROMPT => MARKER_UNEXPECTED_PROMPT,
            EVENT_EVAL_RETURNED => MARKER_EVAL_RETURNED,
            EVENT_PRINT_STARTED => MARKER_PRINT_STARTED,
            EVENT_PRINT_RETURNED => MARKER_PRINT_RETURNED,
            _ => MARKER_INVALID_STATE,
        };
        (marker.as_ptr(), marker.len())
    }

    unsafe fn write_pending_events() {
        unsafe {
            let len_ptr = ptr::addr_of_mut!(PROBE_STATE.event_len);
            let len = ptr::read_volatile(len_ptr);
            let events_ptr = ptr::addr_of_mut!(PROBE_STATE.events).cast::<c_int>();
            let mut index = 0;
            while index < len {
                let event = ptr::read_volatile(events_ptr.add(index));
                let (mut bytes, mut remaining) = event_marker(event);
                while remaining > 0 {
                    let written = write(1, bytes.cast::<c_void>(), remaining);
                    if written <= 0 {
                        ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                        break;
                    }
                    bytes = bytes.add(written as usize);
                    remaining -= written as usize;
                }
                index += 1;
            }
            ptr::write_volatile(len_ptr, 0);
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::main() {
        eprintln!("mainloop unwind spike failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("mainloop unwind spike is supported only on Linux");
    std::process::exit(2);
}
