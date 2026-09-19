//! Linux/R 4.5.2 diagnostic for externally delivered SIGINT outcomes.
//!
//! The parent never initializes R. It runs a child with embedded R, waits for
//! a READY marker written from inside each R expression, and only then sends
//! SIGINT to that child. The child deliberately keeps `R_SignalHandlers = 0`,
//! as `arf-libr` does, and installs a minimal frontend-style SIGINT handler
//! that only sets R's initialized, exported `R_interrupts_pending` flag. This
//! mirrors arf's existing frontend signal path; it does not enable R's own
//! signal-handler setup.
//!
//! The first expression leaves the interrupt uncaught and must unwind through
//! the same POD `R_UnwindProtect` boundary used by the other command probes.
//! The next top-level ReadConsole entry finalizes it as Aborted exactly once,
//! proving that native recovery returns to the top-level prompt. At that prompt,
//! this diagnostic driver supplies a separate `777` action and observes its
//! visible print. A later expression catches the same SIGINT with R's
//! `tryCatch(interrupt = ...)`, asserts the condition class, returns `42`, and
//! must complete eval and visible printing normally.
//!
//! The child uses only fixed-size POD state in ReadConsole, the unwind body,
//! cleanup, and output callbacks. ReadConsole/body/output callbacks emit their
//! own markers immediately; cleanup only records integers, which are emitted
//! at the next safe top-level ReadConsole entry. R console output is copied to
//! a fixed buffer and dumped at final EOF or a fail-closed ReadConsole return,
//! before R cleanup can exit the child. The parent uses `WriteConsoleEx`
//! observation of each READY marker, emitted synchronously by `cat()`, rather
//! than a timing delay or an R connection flush to decide when to send signals.
//!
//! This is a Linux/R 4.5.2 diagnostic only. The direct callback bypasses arf's
//! `AwaitConsoleInputGuard` and does not prove product ReadConsole/SIGINT
//! integration, RefCell/lock release, Windows behavior, or all R interrupt
//! paths. Product integration must also leave the input-wait guard before
//! entering the custom evaluation boundary. No R, ark, or GPL implementation
//! code is copied or translated here.

#[cfg(target_os = "linux")]
mod linux {
    use arf_harp::eval_string;
    use arf_libr::{SEXP, r_library};
    use libloading::Library;
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::io::{BufRead, BufReader, Read};
    use std::mem::MaybeUninit;
    use std::os::raw::{c_char, c_int};
    use std::process::{Command, Stdio};
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    const EVENT_CAPACITY: usize = 64;
    const MARKER_PREFIX: &[u8] = b"ARF_INTERRUPT_OUTCOME:";
    const READY_UNCAUGHT_TEXT: &[u8] = b"ARF_INTERRUPT_OUTCOME:READY:UNCAUGHT";
    const READY_CAUGHT_TEXT: &[u8] = b"ARF_INTERRUPT_OUTCOME:READY:CAUGHT";
    const CAUGHT_INTERRUPT_TEXT: &[u8] = b"ARF_INTERRUPT_OUTCOME:CAUGHT:INTERRUPT";
    const DISPLAY_777: &[u8] = b"[1] 777";
    const DISPLAY_42: &[u8] = b"[1] 42";
    const LINUX_SIGINT: c_int = 2;

    const EVENT_READ_1: c_int = 1;
    const EVENT_READ_2: c_int = 2;
    const EVENT_READ_3: c_int = 3;
    const EVENT_READ_4: c_int = 4;
    const EVENT_ACTION_1_START: c_int = 5;
    const EVENT_ACTION_1_BODY: c_int = 6;
    const EVENT_ACTION_1_EVAL_RETURNED: c_int = 7;
    const EVENT_ACTION_1_PRINT_STARTED: c_int = 8;
    const EVENT_ACTION_1_PRINT_RETURNED: c_int = 9;
    const EVENT_ACTION_1_CLEANUP_NORMAL: c_int = 10;
    const EVENT_ACTION_1_CLEANUP_JUMP: c_int = 11;
    const EVENT_ACTION_1_CALL_RETURNED: c_int = 12;
    const EVENT_ACTION_1_FINALIZED_ABORTED: c_int = 13;
    const EVENT_ACTION_1_FINALIZED_COMPLETED: c_int = 14;
    const EVENT_ACTION_2_START: c_int = 15;
    const EVENT_ACTION_2_BODY: c_int = 16;
    const EVENT_ACTION_2_EVAL_RETURNED: c_int = 17;
    const EVENT_ACTION_2_PRINT_STARTED: c_int = 18;
    const EVENT_ACTION_2_OUTPUT_777: c_int = 19;
    const EVENT_ACTION_2_PRINT_RETURNED: c_int = 20;
    const EVENT_ACTION_2_CLEANUP_NORMAL: c_int = 21;
    const EVENT_ACTION_2_CLEANUP_JUMP: c_int = 22;
    const EVENT_ACTION_2_CALL_RETURNED: c_int = 23;
    const EVENT_ACTION_2_FINALIZED_ABORTED: c_int = 24;
    const EVENT_ACTION_2_FINALIZED_COMPLETED: c_int = 25;
    const EVENT_ACTION_3_START: c_int = 26;
    const EVENT_ACTION_3_BODY: c_int = 27;
    const EVENT_ACTION_3_EVAL_RETURNED: c_int = 28;
    const EVENT_ACTION_3_PRINT_STARTED: c_int = 29;
    const EVENT_ACTION_3_OUTPUT_42: c_int = 30;
    const EVENT_ACTION_3_PRINT_RETURNED: c_int = 31;
    const EVENT_ACTION_3_CLEANUP_NORMAL: c_int = 32;
    const EVENT_ACTION_3_CLEANUP_JUMP: c_int = 33;
    const EVENT_ACTION_3_CALL_RETURNED: c_int = 34;
    const EVENT_ACTION_3_FINALIZED_ABORTED: c_int = 35;
    const EVENT_ACTION_3_FINALIZED_COMPLETED: c_int = 36;
    const EVENT_BOUNDARY_INTERRUPTS_ACTIVE: c_int = 37;
    const EVENT_BODY_INTERRUPTS_ACTIVE: c_int = 38;
    const EVENT_EOF: c_int = 39;
    const EVENT_FAIL_REENTRY: c_int = 40;
    const EVENT_FAIL_PROMPT: c_int = 41;
    const EVENT_FAIL_STATE: c_int = 42;
    const EVENT_FAIL_BOUNDARY: c_int = 43;
    const EVENT_FAIL_OVERFLOW: c_int = 44;
    const EVENT_FAIL_FINALIZE: c_int = 45;
    const EVENT_FAIL_DISPLAY: c_int = 46;
    const EVENT_FAIL_SIGNAL_COUNT: c_int = 47;

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
    type Lang2 = unsafe extern "C" fn(SEXP, SEXP) -> SEXP;

    #[derive(Clone, Copy)]
    struct ProbeApi {
        eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
        protect: unsafe extern "C" fn(SEXP) -> SEXP,
        unprotect: unsafe extern "C" fn(c_int),
        print_value: unsafe extern "C" fn(SEXP),
        vector_elt: unsafe extern "C" fn(SEXP, isize) -> SEXP,
        logical: unsafe extern "C" fn(SEXP) -> *mut c_int,
        lang2: Lang2,
        unwind_protect: RUnwindProtect,
        global_env: SEXP,
        nil_value: SEXP,
        expressions: SEXP,
        with_visible: SEXP,
        interrupts_pending: *mut c_int,
        interrupts_suspended: *mut c_int,
    }

    struct ProbeApiCell(UnsafeCell<MaybeUninit<ProbeApi>>);

    // R is initialized and used only on this process's main thread.
    unsafe impl Sync for ProbeApiCell {}

    static PROBE_API: ProbeApiCell = ProbeApiCell(UnsafeCell::new(MaybeUninit::uninit()));
    static mut PROBE_STATE: ProbeState = ProbeState::new();
    // The handler reads this pointer only after setup has written it and
    // installed the handler. The pending flag itself is R's interrupt state.
    static mut R_INTERRUPT_PENDING_PTR: *mut c_int = ptr::null_mut();
    // This counter is verification bookkeeping; it does not decide R outcomes.
    static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" {
        fn kill(pid: c_int, signal: c_int) -> c_int;
        fn signal(signal: c_int, handler: usize) -> usize;
        fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    }

    #[repr(C)]
    struct ProbeState {
        next_action: c_int,
        active_action: c_int,
        top_read_count: c_int,
        finalized_mask: c_int,
        callback_active: c_int,
        armed: c_int,
        body_entered: c_int,
        cleanup_seen: c_int,
        cleanup_jump: c_int,
        call_returned: c_int,
        failed: c_int,
        display_match_len: c_int,
        ready_match_len: c_int,
        caught_match_len: c_int,
        ready_count: [c_int; 4],
        caught_count: c_int,
        display_count: [c_int; 4],
        output_capture_len: usize,
        output_capture_overflow: c_int,
        output_capture_dumped: c_int,
        output_capture: [u8; 4096],
        event_len: usize,
        events: [c_int; EVENT_CAPACITY],
    }

    impl ProbeState {
        const fn new() -> Self {
            Self {
                next_action: 1,
                active_action: 0,
                top_read_count: 0,
                finalized_mask: 0,
                callback_active: 0,
                armed: 0,
                body_entered: 0,
                cleanup_seen: 0,
                cleanup_jump: 0,
                call_returned: 0,
                failed: 0,
                display_match_len: 0,
                ready_match_len: 0,
                caught_match_len: 0,
                ready_count: [0; 4],
                caught_count: 0,
                display_count: [0; 4],
                output_capture_len: 0,
                output_capture_overflow: 0,
                output_capture_dumped: 0,
                output_capture: [0; 4096],
                event_len: 0,
                events: [0; EVENT_CAPACITY],
            }
        }
    }

    #[repr(C)]
    struct PreserveData {
        preserve: PreserveObject,
        object: SEXP,
    }

    #[repr(C)]
    struct ActionData {
        api: ProbeApi,
        action: c_int,
        expression: SEXP,
    }

    enum ReaderEvent {
        Line(String),
        Closed(std::io::Result<()>),
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
        let r_library = arf_libr::find_r_library()?;
        let r_library_dir = r_library
            .parent()
            .ok_or("R shared library path did not have a parent directory")?;
        let mut library_paths: Vec<_> =
            std::env::split_paths(&std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default())
                .collect();
        if !library_paths.iter().any(|path| path == r_library_dir) {
            library_paths.push(r_library_dir.to_path_buf());
        }
        let child_library_path = std::env::join_paths(library_paths)?;
        let mut child = Command::new(executable)
            .arg("--child")
            .env("R_DEFAULT_PACKAGES", "NULL")
            .env("LD_LIBRARY_PATH", child_library_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().ok_or("child stdout was not piped")?;
        let stderr = child.stderr.take().ok_or("child stderr was not piped")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let (sender, receiver) = mpsc::channel();
        let stderr_reader = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(ReaderEvent::Closed(Ok(())));
                        break;
                    }
                    Ok(_) => {
                        if sender.send(ReaderEvent::Line(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(ReaderEvent::Closed(Err(error)));
                        break;
                    }
                }
            }
        });

        let deadline = Instant::now() + Duration::from_secs(90);
        let mut diagnostics = Vec::new();
        let mut actual_markers = Vec::new();
        let mut sent_uncaught = false;
        let mut sent_caught = false;
        let mut stderr_closed = false;
        let mut stderr_error = None;
        let mut child_status = None;

        loop {
            if child_status.is_none() {
                child_status = child.try_wait()?;
            }
            if stderr_closed && child_status.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait()?;
                let stdout_text = stdout_reader
                    .join()
                    .map_err(|_| std::io::Error::other("stdout reader panicked"))??;
                let _ = stderr_reader.join();
                return Err(format!(
                    "child exceeded the 90-second timeout (status {status})\nstderr:\n{}\nstdout:\n{}",
                    diagnostics.concat(),
                    String::from_utf8_lossy(&stdout_text)
                )
                .into());
            }

            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(ReaderEvent::Line(line)) => {
                    if let Some(marker) = extract_marker(&line) {
                        match marker.as_str() {
                            "READY:UNCAUGHT" => {
                                if sent_uncaught {
                                    terminate_child(&mut child);
                                    return Err(format!(
                                        "duplicate uncaught READY marker\n{}",
                                        diagnostics.concat()
                                    )
                                    .into());
                                }
                                if let Err(error) = send_sigint(child.id()) {
                                    terminate_child(&mut child);
                                    return Err(error);
                                }
                                sent_uncaught = true;
                            }
                            "READY:CAUGHT" => {
                                if !sent_uncaught || sent_caught {
                                    terminate_child(&mut child);
                                    return Err(format!(
                                        "caught READY marker arrived out of order\n{}",
                                        diagnostics.concat()
                                    )
                                    .into());
                                }
                                if let Err(error) = send_sigint(child.id()) {
                                    terminate_child(&mut child);
                                    return Err(error);
                                }
                                sent_caught = true;
                            }
                            _ => {}
                        }
                        actual_markers.push(marker);
                    }
                    diagnostics.push(line);
                }
                Ok(ReaderEvent::Closed(result)) => {
                    stderr_closed = true;
                    if let Err(error) = result {
                        stderr_error = Some(error);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stderr_closed = true;
                }
            }
        }

        let stdout = stdout_reader
            .join()
            .map_err(|_| std::io::Error::other("stdout reader panicked"))??;
        stderr_reader
            .join()
            .map_err(|_| std::io::Error::other("stderr reader panicked"))?;
        let stdout = String::from_utf8_lossy(&stdout);
        let diagnostic_text = diagnostics.concat();
        if let Some(error) = stderr_error {
            return Err(format!("failed reading child stderr: {error}\n{diagnostic_text}").into());
        }
        let status = child_status.ok_or("child status was lost")?;
        if !status.success() {
            return Err(format!(
                "child exited with {status}\nstderr:\n{diagnostic_text}\nstdout:\n{stdout}"
            )
            .into());
        }

        let expected = [
            "INIT:R_SIGNAL_HANDLERS:0",
            "INIT:R_INTERRUPT_FLAG:AVAILABLE",
            "INIT:R_INTERRUPTS_SUSPENDED:0",
            "READ:TOP:1",
            "ACTION:UNCAUGHT:START",
            "BOUNDARY:INTERRUPTS_ACTIVE",
            "ACTION:UNCAUGHT:BODY",
            "BODY:INTERRUPTS_ACTIVE",
            "READY:UNCAUGHT",
            "ACTION:UNCAUGHT:CLEANUP_JUMP",
            "READ:TOP:2",
            "ACTION:UNCAUGHT:FINALIZED_ABORTED",
            "ACTION:CONTINUATION:START",
            "BOUNDARY:INTERRUPTS_ACTIVE",
            "ACTION:CONTINUATION:BODY",
            "BODY:INTERRUPTS_ACTIVE",
            "ACTION:CONTINUATION:EVAL_RETURNED",
            "ACTION:CONTINUATION:PRINT_STARTED",
            "OUTPUT:777",
            "ACTION:CONTINUATION:PRINT_RETURNED",
            "ACTION:CONTINUATION:CLEANUP_NORMAL",
            "ACTION:CONTINUATION:CALL_RETURNED",
            "READ:TOP:3",
            "ACTION:CONTINUATION:FINALIZED_COMPLETED",
            "ACTION:CAUGHT:START",
            "BOUNDARY:INTERRUPTS_ACTIVE",
            "ACTION:CAUGHT:BODY",
            "BODY:INTERRUPTS_ACTIVE",
            "READY:CAUGHT",
            "CAUGHT:INTERRUPT",
            "CAUGHT:PENDING:0",
            "CAUGHT:SIGINTS:2",
            "ACTION:CAUGHT:EVAL_RETURNED",
            "ACTION:CAUGHT:PRINT_STARTED",
            "OUTPUT:42",
            "ACTION:CAUGHT:PRINT_RETURNED",
            "ACTION:CAUGHT:CLEANUP_NORMAL",
            "ACTION:CAUGHT:CALL_RETURNED",
            "READ:TOP:4",
            "ACTION:CAUGHT:FINALIZED_COMPLETED",
            "EOF",
        ];
        if actual_markers != expected {
            return Err(format!(
                "SIGINT outcomes did not match the exact expected sequence\nexpected: {expected:?}\nactual:   {actual_markers:?}\nstderr:\n{diagnostic_text}\nstdout:\n{stdout}"
            )
            .into());
        }
        if !sent_uncaught || !sent_caught {
            return Err(format!(
                "parent did not send both synchronized SIGINTs (uncaught={sent_uncaught}, caught={sent_caught})\n{diagnostic_text}"
            )
            .into());
        }

        println!("SIGINT abort, recovery, and R-level catch outcomes verified");
        println!(
            "uncaught SIGINT finalized Aborted once; R-caught SIGINT completed and printed 42"
        );
        Ok(())
    }

    fn send_sigint(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
        let result = unsafe { kill(pid as c_int, LINUX_SIGINT) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn terminate_child(child: &mut std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn extract_marker(line: &str) -> Option<String> {
        let marker_start =
            line.find(std::str::from_utf8(MARKER_PREFIX).ok()?)? + MARKER_PREFIX.len();
        let marker = line[marker_start..].trim_end_matches(['\r', '\n']);
        Some(marker.to_owned())
    }

    fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn child_main() -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("R_DEFAULT_PACKAGES", "NULL");
            arf_libr::initialize_r_with_args(&[
                "--vanilla",
                "--no-save",
                "--quiet",
                "--interactive",
            ])?;
        }

        let version = eval_string(
            r#"
if (!identical(as.character(getRversion()), "4.5.2")) {
    stop("this diagnostic is limited to R 4.5.2")
}
"#,
        )?;
        drop(version);

        let api = r_library()?;
        if api.r_signalhandlers.is_null()
            || unsafe { ptr::read_volatile(api.r_signalhandlers) } != 0
        {
            return Err(
                "expected arf-libr initialization to leave R_SignalHandlers disabled".into(),
            );
        }
        if api.r_interrupts_pending.is_null() {
            return Err("R_interrupts_pending was not available".into());
        }
        unsafe { ptr::write_volatile(api.r_interrupts_pending, 0) };

        let setup = eval_string(
            r#"
        invisible(list(
    list(
        quote({
            cat("ARF_INTERRUPT_OUTCOME:READY:UNCAUGHT\n", file = stderr())
            repeat { }
        }),
        quote(777L),
        quote(tryCatch({
            cat("ARF_INTERRUPT_OUTCOME:READY:CAUGHT\n", file = stderr())
            repeat { }
        }, interrupt = function(condition) {
            if (!inherits(condition, "interrupt")) {
                stop("caught condition did not inherit from interrupt")
            }
            cat("ARF_INTERRUPT_OUTCOME:CAUGHT:INTERRUPT\n", file = stderr())
            42L
        }))
    ),
    base::withVisible
        ))
"#,
        )?;
        let preserved_setup = setup.sexp();
        let library = unsafe { Library::new(arf_libr::find_r_library()?)? };
        let preserve_object = unsafe { *library.get::<PreserveObject>(b"R_PreserveObject\0")? };
        let lang2 = unsafe { *library.get::<Lang2>(b"Rf_lang2\0")? };
        let unwind_protect = unsafe { *library.get::<RUnwindProtect>(b"R_UnwindProtect\0")? };
        let mut preserve_data = PreserveData {
            preserve: preserve_object,
            object: preserved_setup,
        };
        let preserve_succeeded = unsafe {
            (api.r_toplevelexec)(
                Some(preserve_setup_callback),
                ptr::addr_of_mut!(preserve_data).cast::<c_void>(),
            )
        };
        if preserve_succeeded == 0 {
            return Err("R_PreserveObject failed inside the setup top-level boundary".into());
        }
        let expressions = unsafe { (api.vector_elt)(preserved_setup, 0) };
        let with_visible = unsafe { (api.vector_elt)(preserved_setup, 1) };
        drop(setup);

        unsafe {
            write_marker(b"INIT:R_SIGNAL_HANDLERS:0");
            write_marker(b"INIT:R_INTERRUPT_FLAG:AVAILABLE");
        }
        let interrupts_suspended = api.r_interrupts_suspended;
        if interrupts_suspended.is_null() {
            unsafe { write_marker(b"INIT:R_INTERRUPTS_SUSPENDED:UNAVAILABLE") };
        } else {
            let suspended = unsafe { ptr::read_volatile(interrupts_suspended) };
            if suspended != 0 {
                return Err(format!(
                    "R_interrupts_suspended was {suspended} before mainloop entry"
                )
                .into());
            }
            unsafe { write_marker(b"INIT:R_INTERRUPTS_SUSPENDED:0") };
        }
        let probe_api = ProbeApi {
            eval: api.rf_eval,
            protect: api.rf_protect,
            unprotect: api.rf_unprotect,
            print_value: api.rf_printvalue,
            vector_elt: api.vector_elt,
            logical: api.logical,
            lang2,
            unwind_protect,
            global_env: unsafe { *api.r_globalenv },
            nil_value: unsafe { *api.r_nilvalue },
            expressions,
            with_visible,
            interrupts_pending: api.r_interrupts_pending,
            interrupts_suspended,
        };

        unsafe {
            ptr::write((*PROBE_API.0.get()).as_mut_ptr(), probe_api);
            ptr::write(ptr::addr_of_mut!(PROBE_STATE), ProbeState::new());
            ptr::write_volatile(
                ptr::addr_of_mut!(R_INTERRUPT_PENDING_PTR),
                api.r_interrupts_pending,
            );
            if api.ptr_r_readconsole.is_null() {
                return Err("R export ptr_R_ReadConsole was null".into());
            }
            if api.ptr_r_writeconsoleex.is_null() {
                return Err("R export ptr_R_WriteConsoleEx was null".into());
            }
            *api.ptr_r_readconsole = Some(read_console);
            *api.ptr_r_writeconsoleex = Some(write_console_ex);
            install_sigint_handler()?;
            (api.run_rmainloop)();
        }

        unsafe { dump_console_capture() };

        let state = ptr::addr_of_mut!(PROBE_STATE);
        let failed = unsafe { ptr::read_volatile(ptr::addr_of!((*state).failed)) };
        let finalized_mask = unsafe { ptr::read_volatile(ptr::addr_of!((*state).finalized_mask)) };
        let signal_count = unsafe { read_sigint_count() };
        if failed != 0 {
            return Err("the child recorded a fail-closed callback invariant".into());
        }
        if finalized_mask != 0b111 {
            return Err(format!(
                "expected three exactly-once finalizations, got mask {finalized_mask:#b}"
            )
            .into());
        }
        if signal_count != 2 {
            return Err(format!("expected two delivered SIGINTs, got {signal_count}").into());
        }
        if unsafe { ptr::read_volatile(ptr::addr_of!((*state).output_capture_overflow)) } != 0 {
            return Err("R console output capture overflowed".into());
        }
        drop(library);
        Ok(())
    }

    unsafe fn install_sigint_handler() -> std::io::Result<()> {
        if unsafe { signal(LINUX_SIGINT, frontend_sigint_handler as *const () as usize) }
            == usize::MAX
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    unsafe extern "C" fn frontend_sigint_handler(_signal: c_int) {
        unsafe {
            let flag = ptr::read_volatile(ptr::addr_of!(R_INTERRUPT_PENDING_PTR));
            if !flag.is_null() {
                ptr::write_volatile(flag, 1);
            }
            SIGINT_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    unsafe fn read_sigint_count() -> usize {
        SIGINT_COUNT.load(Ordering::Relaxed)
    }

    unsafe extern "C" fn preserve_setup_callback(data: *mut c_void) {
        unsafe {
            let payload = data.cast::<PreserveData>();
            ((*payload).preserve)((*payload).object);
        }
    }

    unsafe extern "C" fn read_console(
        prompt: *const c_char,
        buffer: *mut c_char,
        buffer_len: c_int,
        _history: c_int,
    ) -> c_int {
        unsafe {
            flush_events();
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if read_state(ptr::addr_of!((*state).callback_active)) != 0 {
                return fail_closed_read(EVENT_FAIL_REENTRY);
            }
            if !is_top_level_prompt(prompt) {
                return fail_closed_read(EVENT_FAIL_PROMPT);
            }
            if buffer.is_null() || buffer_len < 2 {
                return fail_closed_read(EVENT_FAIL_BOUNDARY);
            }

            let top_read_count = read_state(ptr::addr_of!((*state).top_read_count)) + 1;
            write_state(ptr::addr_of_mut!((*state).top_read_count), top_read_count);
            let read_event = match top_read_count {
                1 => EVENT_READ_1,
                2 => EVENT_READ_2,
                3 => EVENT_READ_3,
                4 => EVENT_READ_4,
                _ => {
                    return fail_closed_read(EVENT_FAIL_STATE);
                }
            };
            write_marker(event_payload(read_event));

            if read_state(ptr::addr_of!((*state).armed)) != 0 && !finalize_previous_action() {
                return fail_closed_read(EVENT_FAIL_FINALIZE);
            }
            if read_state(ptr::addr_of!((*state).failed)) != 0 {
                flush_events();
                dump_console_capture();
                return 0;
            }

            let action = read_state(ptr::addr_of!((*state).next_action));
            if action > 3 {
                if read_state(ptr::addr_of!((*state).finalized_mask)) != 0b111
                    || read_sigint_count() != 2
                    || read_state(ptr::addr_of!((*state).output_capture_overflow)) != 0
                {
                    return fail_closed_read(EVENT_FAIL_FINALIZE);
                } else {
                    write_marker(event_payload(EVENT_EOF));
                }
                flush_events();
                dump_console_capture();
                return 0;
            }

            let api = ptr::read((*PROBE_API.0.get()).as_ptr());
            let expression = (api.vector_elt)(api.expressions, (action - 1) as isize);
            let mut action_data = ActionData {
                api,
                action,
                expression,
            };
            write_state(ptr::addr_of_mut!((*state).next_action), action + 1);
            write_state(ptr::addr_of_mut!((*state).active_action), action);
            write_state(ptr::addr_of_mut!((*state).armed), 1);
            write_state(ptr::addr_of_mut!((*state).body_entered), 0);
            write_state(ptr::addr_of_mut!((*state).cleanup_seen), 0);
            write_state(ptr::addr_of_mut!((*state).cleanup_jump), 0);
            write_state(ptr::addr_of_mut!((*state).call_returned), 0);
            write_state(ptr::addr_of_mut!((*state).display_match_len), 0);
            write_state(ptr::addr_of_mut!((*state).ready_match_len), 0);
            write_state(ptr::addr_of_mut!((*state).caught_match_len), 0);
            if !interrupts_active(api.interrupts_suspended) {
                return fail_closed_read(EVENT_FAIL_STATE);
            }
            write_marker(event_payload(match action {
                1 => EVENT_ACTION_1_START,
                2 => EVENT_ACTION_2_START,
                3 => EVENT_ACTION_3_START,
                _ => EVENT_FAIL_STATE,
            }));
            write_marker(event_payload(EVENT_BOUNDARY_INTERRUPTS_ACTIVE));
            write_state(ptr::addr_of_mut!((*state).callback_active), 1);

            (api.unwind_protect)(
                Some(unwind_body),
                ptr::addr_of_mut!(action_data).cast::<c_void>(),
                Some(unwind_cleanup),
                ptr::addr_of_mut!(action_data).cast::<c_void>(),
                ptr::null_mut(),
            );

            write_state(ptr::addr_of_mut!((*state).call_returned), 1);
            write_state(ptr::addr_of_mut!((*state).callback_active), 0);
            append_action_event(
                action,
                EVENT_ACTION_1_CALL_RETURNED,
                EVENT_ACTION_2_CALL_RETURNED,
                EVENT_ACTION_3_CALL_RETURNED,
            );
            buffer.write(b'\n' as c_char);
            buffer.add(1).write(0);
            1
        }
    }

    unsafe extern "C" fn unwind_body(data: *mut c_void) -> SEXP {
        unsafe {
            let action_data = data.cast::<ActionData>();
            let action = (*action_data).action;
            let api = (*action_data).api;
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if read_state(ptr::addr_of!((*state).armed)) != 1
                || read_state(ptr::addr_of!((*state).body_entered)) != 0
                || read_state(ptr::addr_of!((*state).active_action)) != action
            {
                fail(EVENT_FAIL_BOUNDARY);
                return api.nil_value;
            }
            write_state(ptr::addr_of_mut!((*state).body_entered), 1);
            write_marker(event_payload(match action {
                1 => EVENT_ACTION_1_BODY,
                2 => EVENT_ACTION_2_BODY,
                3 => EVENT_ACTION_3_BODY,
                _ => EVENT_FAIL_STATE,
            }));
            if !interrupts_active(api.interrupts_suspended) {
                fail(EVENT_FAIL_STATE);
                return api.nil_value;
            }
            write_marker(event_payload(EVENT_BODY_INTERRUPTS_ACTIVE));

            let call = (api.lang2)(api.with_visible, (*action_data).expression);
            (api.protect)(call);
            let result = (api.eval)(call, api.global_env);
            (api.protect)(result);
            append_action_event(
                action,
                EVENT_ACTION_1_EVAL_RETURNED,
                EVENT_ACTION_2_EVAL_RETURNED,
                EVENT_ACTION_3_EVAL_RETURNED,
            );

            let value = (api.vector_elt)(result, 0);
            let visible = (api.vector_elt)(result, 1);
            let visible_ptr = (api.logical)(visible);
            if visible_ptr.is_null() || ptr::read(visible_ptr) != 1 {
                fail(EVENT_FAIL_BOUNDARY);
                (api.unprotect)(2);
                return api.nil_value;
            }
            append_action_event(
                action,
                EVENT_ACTION_1_PRINT_STARTED,
                EVENT_ACTION_2_PRINT_STARTED,
                EVENT_ACTION_3_PRINT_STARTED,
            );
            (api.print_value)(value);
            append_action_event(
                action,
                EVENT_ACTION_1_PRINT_RETURNED,
                EVENT_ACTION_2_PRINT_RETURNED,
                EVENT_ACTION_3_PRINT_RETURNED,
            );
            (api.unprotect)(2);
            api.nil_value
        }
    }

    unsafe extern "C" fn unwind_cleanup(_data: *mut c_void, jump: c_int) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let action = read_state(ptr::addr_of!((*state).active_action));
            if read_state(ptr::addr_of!((*state).armed)) != 1
                || read_state(ptr::addr_of!((*state).cleanup_seen)) != 0
            {
                fail(EVENT_FAIL_BOUNDARY);
            }
            write_state(ptr::addr_of_mut!((*state).cleanup_seen), 1);
            write_state(ptr::addr_of_mut!((*state).cleanup_jump), jump);
            write_state(ptr::addr_of_mut!((*state).callback_active), 0);
            if jump == 0 {
                append_action_event(
                    action,
                    EVENT_ACTION_1_CLEANUP_NORMAL,
                    EVENT_ACTION_2_CLEANUP_NORMAL,
                    EVENT_ACTION_3_CLEANUP_NORMAL,
                );
            } else {
                append_action_event(
                    action,
                    EVENT_ACTION_1_CLEANUP_JUMP,
                    EVENT_ACTION_2_CLEANUP_JUMP,
                    EVENT_ACTION_3_CLEANUP_JUMP,
                );
            }
        }
    }

    unsafe extern "C" fn write_console_ex(buffer: *const c_char, len: c_int, _otype: c_int) {
        unsafe {
            if buffer.is_null() || len <= 0 {
                return;
            }
            capture_console_output(buffer.cast::<u8>(), len as usize);
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let action = read_state(ptr::addr_of!((*state).active_action));
            let display_needle = match action {
                2 => DISPLAY_777,
                3 => DISPLAY_42,
                _ => &[][..],
            };
            let mut display_matched =
                read_state(ptr::addr_of!((*state).display_match_len)) as usize;
            let mut ready_matched = read_state(ptr::addr_of!((*state).ready_match_len)) as usize;
            let mut caught_matched = read_state(ptr::addr_of!((*state).caught_match_len)) as usize;
            for offset in 0..len as usize {
                let byte = buffer.add(offset).cast::<u8>().read();

                if action == 1 {
                    let (next, complete) = advance_match(ready_matched, byte, READY_UNCAUGHT_TEXT);
                    ready_matched = next;
                    if complete {
                        let count = read_state(ptr::addr_of!((*state).ready_count[1])) + 1;
                        write_state(ptr::addr_of_mut!((*state).ready_count[1]), count);
                        if count == 1 {
                            write_marker(b"READY:UNCAUGHT");
                        } else {
                            fail(EVENT_FAIL_STATE);
                        }
                    }
                } else if action == 3 {
                    let (next, complete) = advance_match(ready_matched, byte, READY_CAUGHT_TEXT);
                    ready_matched = next;
                    if complete {
                        let count = read_state(ptr::addr_of!((*state).ready_count[3])) + 1;
                        write_state(ptr::addr_of_mut!((*state).ready_count[3]), count);
                        if count == 1 {
                            write_marker(b"READY:CAUGHT");
                        } else {
                            fail(EVENT_FAIL_STATE);
                        }
                    }
                    let (next, complete) =
                        advance_match(caught_matched, byte, CAUGHT_INTERRUPT_TEXT);
                    caught_matched = next;
                    if complete {
                        let count = read_state(ptr::addr_of!((*state).caught_count)) + 1;
                        write_state(ptr::addr_of_mut!((*state).caught_count), count);
                        if count == 1 {
                            write_marker(b"CAUGHT:INTERRUPT");
                            let api = ptr::read((*PROBE_API.0.get()).as_ptr());
                            if api.interrupts_pending.is_null()
                                || ptr::read_volatile(api.interrupts_pending) == 0
                            {
                                write_marker(b"CAUGHT:PENDING:0");
                            } else {
                                write_marker(b"CAUGHT:PENDING:1");
                            }
                            if read_sigint_count() == 2 {
                                write_marker(b"CAUGHT:SIGINTS:2");
                            } else {
                                write_marker(b"CAUGHT:SIGINTS:UNEXPECTED");
                            }
                        } else {
                            fail(EVENT_FAIL_STATE);
                        }
                    }
                }

                if !display_needle.is_empty() {
                    let (next, complete) = advance_match(display_matched, byte, display_needle);
                    display_matched = next;
                    if complete {
                        let index = action as usize;
                        let count = read_state(ptr::addr_of!((*state).display_count[index])) + 1;
                        write_state(ptr::addr_of_mut!((*state).display_count[index]), count);
                        if count != 1 {
                            fail(EVENT_FAIL_DISPLAY);
                        } else if action == 2 {
                            append_event(EVENT_ACTION_2_OUTPUT_777);
                        } else {
                            append_event(EVENT_ACTION_3_OUTPUT_42);
                        }
                    }
                }
            }
            write_state(
                ptr::addr_of_mut!((*state).display_match_len),
                display_matched as c_int,
            );
            write_state(
                ptr::addr_of_mut!((*state).ready_match_len),
                ready_matched as c_int,
            );
            write_state(
                ptr::addr_of_mut!((*state).caught_match_len),
                caught_matched as c_int,
            );
        }
    }

    unsafe fn capture_console_output(bytes: *const u8, len: usize) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let len_ptr = ptr::addr_of_mut!((*state).output_capture_len);
            let current = ptr::read_volatile(len_ptr);
            let capacity = (*state).output_capture.len();
            let available = capacity.saturating_sub(current);
            let copied = available.min(len);
            if copied > 0 {
                let output = ptr::addr_of_mut!((*state).output_capture).cast::<u8>();
                ptr::copy_nonoverlapping(bytes, output.add(current), copied);
                ptr::write_volatile(len_ptr, current + copied);
            }
            if copied != len {
                ptr::write_volatile(ptr::addr_of_mut!((*state).output_capture_overflow), 1);
            }
        }
    }

    unsafe fn dump_console_capture() {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if read_state(ptr::addr_of!((*state).output_capture_dumped)) != 0 {
                return;
            }
            write_state(ptr::addr_of_mut!((*state).output_capture_dumped), 1);
            let header = b"\nR console output captured by WriteConsoleEx:\n";
            let _ = write(1, header.as_ptr().cast::<c_void>(), header.len());
            let len = ptr::read_volatile(ptr::addr_of!((*state).output_capture_len));
            let output = ptr::addr_of!((*state).output_capture).cast::<u8>();
            let _ = write(1, output.cast::<c_void>(), len);
            let overflow = read_state(ptr::addr_of!((*state).output_capture_overflow)) != 0;
            if overflow {
                let warning = b"\nR console output capture overflowed\n";
                let _ = write(1, warning.as_ptr().cast::<c_void>(), warning.len());
            } else {
                let newline = b"\n";
                let _ = write(1, newline.as_ptr().cast::<c_void>(), newline.len());
            }
        }
    }

    fn advance_match(matched: usize, byte: u8, needle: &[u8]) -> (usize, bool) {
        if needle.is_empty() {
            return (0, false);
        }
        let next = if byte == needle[matched] {
            matched + 1
        } else if byte == needle[0] {
            1
        } else {
            0
        };
        if next == needle.len() {
            (0, true)
        } else {
            (next, false)
        }
    }

    unsafe fn finalize_previous_action() -> bool {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let action = read_state(ptr::addr_of!((*state).active_action));
            let cleanup_seen = read_state(ptr::addr_of!((*state).cleanup_seen));
            let cleanup_jump = read_state(ptr::addr_of!((*state).cleanup_jump));
            let call_returned = read_state(ptr::addr_of!((*state).call_returned));
            let finalized_mask = read_state(ptr::addr_of!((*state).finalized_mask));
            let signal_count = read_sigint_count();
            let output_count = if action == 2 || action == 3 {
                read_state(ptr::addr_of!((*state).display_count[action as usize]))
            } else {
                0
            };
            let mask = 1 << (action - 1);
            if finalized_mask & mask != 0 || cleanup_seen != 1 {
                fail(EVENT_FAIL_FINALIZE);
                return false;
            }

            let event = match action {
                1 if cleanup_jump != 0
                    && call_returned == 0
                    && signal_count == 1
                    && read_state(ptr::addr_of!((*state).ready_count[1])) == 1 =>
                {
                    EVENT_ACTION_1_FINALIZED_ABORTED
                }
                2 if cleanup_jump == 0
                    && call_returned == 1
                    && signal_count == 1
                    && output_count == 1 =>
                {
                    EVENT_ACTION_2_FINALIZED_COMPLETED
                }
                3 if cleanup_jump == 0
                    && call_returned == 1
                    && signal_count == 2
                    && read_state(ptr::addr_of!((*state).ready_count[3])) == 1
                    && read_state(ptr::addr_of!((*state).caught_count)) == 1
                    && output_count == 1 =>
                {
                    EVENT_ACTION_3_FINALIZED_COMPLETED
                }
                _ => {
                    if signal_count != if action == 3 { 2 } else { 1 } {
                        fail(EVENT_FAIL_SIGNAL_COUNT);
                    } else {
                        fail(EVENT_FAIL_FINALIZE);
                    }
                    return false;
                }
            };
            write_state(
                ptr::addr_of_mut!((*state).finalized_mask),
                finalized_mask | mask,
            );
            write_state(ptr::addr_of_mut!((*state).active_action), 0);
            write_state(ptr::addr_of_mut!((*state).armed), 0);
            write_marker(event_payload(event));
            true
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

    unsafe fn interrupts_active(suspended: *mut c_int) -> bool {
        suspended.is_null() || unsafe { ptr::read_volatile(suspended) == 0 }
    }

    unsafe fn read_state(field: *const c_int) -> c_int {
        unsafe { ptr::read_volatile(field) }
    }

    unsafe fn fail_closed_read(event: c_int) -> c_int {
        unsafe {
            fail(event);
            flush_events();
            dump_console_capture();
        }
        0
    }

    unsafe fn write_state(field: *mut c_int, value: c_int) {
        unsafe { ptr::write_volatile(field, value) };
    }

    unsafe fn append_action_event(action: c_int, action1: c_int, action2: c_int, action3: c_int) {
        let event = match action {
            1 => action1,
            2 => action2,
            3 => action3,
            _ => EVENT_FAIL_STATE,
        };
        unsafe { append_event(event) };
    }

    unsafe fn append_event(event: c_int) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let len_ptr = ptr::addr_of_mut!((*state).event_len);
            let len = ptr::read_volatile(len_ptr);
            let events_ptr = ptr::addr_of_mut!((*state).events).cast::<c_int>();
            if len >= EVENT_CAPACITY - 1 {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                if len < EVENT_CAPACITY {
                    ptr::write_volatile(events_ptr.add(len), EVENT_FAIL_OVERFLOW);
                    ptr::write_volatile(len_ptr, len + 1);
                }
                return;
            }
            ptr::write_volatile(events_ptr.add(len), event);
            ptr::write_volatile(len_ptr, len + 1);
        }
    }

    unsafe fn fail(event: c_int) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if read_state(ptr::addr_of!((*state).failed)) == 0 {
                write_state(ptr::addr_of_mut!((*state).failed), 1);
                append_event(event);
            }
        }
    }

    unsafe fn flush_events() {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let len_ptr = ptr::addr_of_mut!((*state).event_len);
            let len = ptr::read_volatile(len_ptr);
            let events_ptr = ptr::addr_of_mut!((*state).events).cast::<c_int>();
            for index in 0..len {
                write_marker(event_payload(ptr::read_volatile(events_ptr.add(index))));
            }
            ptr::write_volatile(len_ptr, 0);
        }
    }

    fn event_payload(event: c_int) -> &'static [u8] {
        match event {
            EVENT_READ_1 => b"READ:TOP:1",
            EVENT_READ_2 => b"READ:TOP:2",
            EVENT_READ_3 => b"READ:TOP:3",
            EVENT_READ_4 => b"READ:TOP:4",
            EVENT_ACTION_1_START => b"ACTION:UNCAUGHT:START",
            EVENT_ACTION_1_BODY => b"ACTION:UNCAUGHT:BODY",
            EVENT_ACTION_1_EVAL_RETURNED => b"ACTION:UNCAUGHT:EVAL_RETURNED",
            EVENT_ACTION_1_PRINT_STARTED => b"ACTION:UNCAUGHT:PRINT_STARTED",
            EVENT_ACTION_1_PRINT_RETURNED => b"ACTION:UNCAUGHT:PRINT_RETURNED",
            EVENT_ACTION_1_CLEANUP_NORMAL => b"ACTION:UNCAUGHT:CLEANUP_NORMAL",
            EVENT_ACTION_1_CLEANUP_JUMP => b"ACTION:UNCAUGHT:CLEANUP_JUMP",
            EVENT_ACTION_1_CALL_RETURNED => b"ACTION:UNCAUGHT:CALL_RETURNED",
            EVENT_ACTION_1_FINALIZED_ABORTED => b"ACTION:UNCAUGHT:FINALIZED_ABORTED",
            EVENT_ACTION_1_FINALIZED_COMPLETED => b"ACTION:UNCAUGHT:FINALIZED_COMPLETED",
            EVENT_ACTION_2_START => b"ACTION:CONTINUATION:START",
            EVENT_ACTION_2_BODY => b"ACTION:CONTINUATION:BODY",
            EVENT_ACTION_2_EVAL_RETURNED => b"ACTION:CONTINUATION:EVAL_RETURNED",
            EVENT_ACTION_2_PRINT_STARTED => b"ACTION:CONTINUATION:PRINT_STARTED",
            EVENT_ACTION_2_OUTPUT_777 => b"OUTPUT:777",
            EVENT_ACTION_2_PRINT_RETURNED => b"ACTION:CONTINUATION:PRINT_RETURNED",
            EVENT_ACTION_2_CLEANUP_NORMAL => b"ACTION:CONTINUATION:CLEANUP_NORMAL",
            EVENT_ACTION_2_CLEANUP_JUMP => b"ACTION:CONTINUATION:CLEANUP_JUMP",
            EVENT_ACTION_2_CALL_RETURNED => b"ACTION:CONTINUATION:CALL_RETURNED",
            EVENT_ACTION_2_FINALIZED_ABORTED => b"ACTION:CONTINUATION:FINALIZED_ABORTED",
            EVENT_ACTION_2_FINALIZED_COMPLETED => b"ACTION:CONTINUATION:FINALIZED_COMPLETED",
            EVENT_ACTION_3_START => b"ACTION:CAUGHT:START",
            EVENT_ACTION_3_BODY => b"ACTION:CAUGHT:BODY",
            EVENT_ACTION_3_EVAL_RETURNED => b"ACTION:CAUGHT:EVAL_RETURNED",
            EVENT_ACTION_3_PRINT_STARTED => b"ACTION:CAUGHT:PRINT_STARTED",
            EVENT_ACTION_3_OUTPUT_42 => b"OUTPUT:42",
            EVENT_ACTION_3_PRINT_RETURNED => b"ACTION:CAUGHT:PRINT_RETURNED",
            EVENT_ACTION_3_CLEANUP_NORMAL => b"ACTION:CAUGHT:CLEANUP_NORMAL",
            EVENT_ACTION_3_CLEANUP_JUMP => b"ACTION:CAUGHT:CLEANUP_JUMP",
            EVENT_ACTION_3_CALL_RETURNED => b"ACTION:CAUGHT:CALL_RETURNED",
            EVENT_ACTION_3_FINALIZED_ABORTED => b"ACTION:CAUGHT:FINALIZED_ABORTED",
            EVENT_ACTION_3_FINALIZED_COMPLETED => b"ACTION:CAUGHT:FINALIZED_COMPLETED",
            EVENT_BOUNDARY_INTERRUPTS_ACTIVE => b"BOUNDARY:INTERRUPTS_ACTIVE",
            EVENT_BODY_INTERRUPTS_ACTIVE => b"BODY:INTERRUPTS_ACTIVE",
            EVENT_EOF => b"EOF",
            EVENT_FAIL_REENTRY => b"FAIL:REENTRY",
            EVENT_FAIL_PROMPT => b"FAIL:UNEXPECTED_PROMPT",
            EVENT_FAIL_STATE => b"FAIL:STATE",
            EVENT_FAIL_BOUNDARY => b"FAIL:BOUNDARY",
            EVENT_FAIL_OVERFLOW => b"FAIL:EVENT_OVERFLOW",
            EVENT_FAIL_FINALIZE => b"FAIL:FINALIZE",
            EVENT_FAIL_DISPLAY => b"FAIL:DISPLAY",
            EVENT_FAIL_SIGNAL_COUNT => b"FAIL:SIGNAL_COUNT",
            _ => b"FAIL:UNKNOWN_EVENT",
        }
    }

    unsafe fn write_marker(payload: &[u8]) {
        unsafe {
            let mut buffer = [0_u8; 128];
            let total_len = MARKER_PREFIX.len() + payload.len() + 1;
            if total_len > buffer.len() {
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                return;
            }
            ptr::copy_nonoverlapping(
                MARKER_PREFIX.as_ptr(),
                buffer.as_mut_ptr(),
                MARKER_PREFIX.len(),
            );
            ptr::copy_nonoverlapping(
                payload.as_ptr(),
                buffer.as_mut_ptr().add(MARKER_PREFIX.len()),
                payload.len(),
            );
            buffer[total_len - 1] = b'\n';
            let written = write(2, buffer.as_ptr().cast::<c_void>(), total_len);
            if written != total_len as isize {
                write_state(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("interrupt outcome spike is supported on Linux only");
}
