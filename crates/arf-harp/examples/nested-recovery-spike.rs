//! Linux/R 4.5.2 diagnostic for nested ReadConsole calls during an embedded
//! top-level evaluation.
//!
//! The parent does not initialize R. It launches one child with a fixed
//! 90-second timeout and checks the child's status and exact marker sequence.
//! The child installs POD console callbacks and enters `run_Rmainloop`.
//!
//! A custom `Rf_eval`/`Rf_PrintValue` boundary runs inside `R_UnwindProtect`
//! with a C NULL continuation. The first expression enters `browser()` and is
//! continued at the nested browser prompt. The second raises an error; the
//! configured `options(error = utils::recover)` opens a nested selection prompt
//! while the eval body is still active, receives `0`, and then unwinds through
//! cleanup. These fixtures test that neither nested prompt finalizes or
//! consumes the outer command. The option is used only to exercise native
//! recovery compatibility; it is not an error-detection mechanism.
//!
//! The expression list is preserved inside a setup-only `R_ToplevelExec`.
//! ReadConsole and unwind callbacks keep only raw pointers and fixed-size POD
//! state. Cleanup records events only; a later safe ReadConsole entry writes
//! their markers with the low-level file descriptor API. The WriteConsoleEx
//! hook observes only the final `777` display marker.
//!
//! This does not cover all browser or recover behavior, Windows, interrupts, or
//! integration with the product's RefCell-based REPL state. It is not evidence
//! that the product REPL outcome model is implemented.

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

    const EVENT_CAPACITY: usize = 64;
    const ACTION_COUNT: c_int = 3;
    const DISPLAY_NEEDLE: &[u8] = b"[1] 777";
    const MARKER_PREFIX: &[u8] = b"ARF_NESTED_RECOVERY:";

    const EVENT_TOP_READ_1: c_int = 1;
    const EVENT_TOP_READ_2: c_int = 2;
    const EVENT_TOP_READ_3: c_int = 3;
    const EVENT_TOP_READ_4: c_int = 4;
    const EVENT_ACTION_1_START: c_int = 5;
    const EVENT_ACTION_2_START: c_int = 6;
    const EVENT_ACTION_3_START: c_int = 7;
    const EVENT_ACTION_1_BODY: c_int = 8;
    const EVENT_ACTION_2_BODY: c_int = 9;
    const EVENT_ACTION_3_BODY: c_int = 10;
    const EVENT_BROWSER_PROMPT: c_int = 11;
    const EVENT_BROWSER_CONTINUE: c_int = 12;
    const EVENT_RECOVER_PROMPT: c_int = 13;
    const EVENT_RECOVER_ZERO: c_int = 14;
    const EVENT_ACTION_1_EVAL_RETURNED: c_int = 15;
    const EVENT_ACTION_2_EVAL_RETURNED: c_int = 16;
    const EVENT_ACTION_3_EVAL_RETURNED: c_int = 17;
    const EVENT_ACTION_1_PRINT_STARTED: c_int = 18;
    const EVENT_ACTION_2_PRINT_STARTED: c_int = 19;
    const EVENT_ACTION_3_PRINT_STARTED: c_int = 20;
    const EVENT_ACTION_1_PRINT_RETURNED: c_int = 21;
    const EVENT_ACTION_2_PRINT_RETURNED: c_int = 22;
    const EVENT_ACTION_3_PRINT_RETURNED: c_int = 23;
    const EVENT_ACTION_1_CLEANUP_NORMAL: c_int = 24;
    const EVENT_ACTION_2_CLEANUP_NORMAL: c_int = 25;
    const EVENT_ACTION_3_CLEANUP_NORMAL: c_int = 26;
    const EVENT_ACTION_1_CLEANUP_JUMP: c_int = 27;
    const EVENT_ACTION_2_CLEANUP_JUMP: c_int = 28;
    const EVENT_ACTION_3_CLEANUP_JUMP: c_int = 29;
    const EVENT_ACTION_1_CALL_RETURNED: c_int = 30;
    const EVENT_ACTION_2_CALL_RETURNED: c_int = 31;
    const EVENT_ACTION_3_CALL_RETURNED: c_int = 32;
    const EVENT_ACTION_1_FINALIZED: c_int = 33;
    const EVENT_ACTION_2_ABORTED_EVAL: c_int = 34;
    const EVENT_ACTION_3_FINALIZED: c_int = 35;
    const EVENT_OUTPUT_777: c_int = 36;
    const EVENT_EOF: c_int = 37;
    const EVENT_FAIL_REENTRY: c_int = 38;
    const EVENT_FAIL_PROMPT: c_int = 39;
    const EVENT_FAIL_STATE: c_int = 40;
    const EVENT_FAIL_BOUNDARY: c_int = 41;
    const EVENT_FAIL_OVERFLOW: c_int = 42;
    const EVENT_FAIL_FINALIZE: c_int = 43;
    const EVENT_FAIL_CLEANUP: c_int = 44;
    const EVENT_FAIL_NESTED: c_int = 45;
    const EVENT_FAIL_DISPLAY: c_int = 46;

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

    #[repr(C)]
    struct ProbeState {
        next_action: c_int,
        active_action: c_int,
        top_read_count: c_int,
        finalized_mask: c_int,
        top_callback_active: c_int,
        nested_depth: c_int,
        browser_prompt_count: c_int,
        recover_prompt_count: c_int,
        armed: c_int,
        body_entered: c_int,
        cleanup_seen: c_int,
        cleanup_jump: c_int,
        call_returned: c_int,
        failed: c_int,
        display_match_len: usize,
        saw_display_777: c_int,
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
                top_callback_active: 0,
                nested_depth: 0,
                browser_prompt_count: 0,
                recover_prompt_count: 0,
                armed: 0,
                body_entered: 0,
                cleanup_seen: 0,
                cleanup_jump: 0,
                call_returned: 0,
                failed: 0,
                display_match_len: 0,
                saw_display_777: 0,
                event_len: 0,
                events: [0; EVENT_CAPACITY],
            }
        }
    }

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
    }

    struct ProbeApiCell(UnsafeCell<MaybeUninit<ProbeApi>>);

    // R is initialized and used only on this process's main thread.
    unsafe impl Sync for ProbeApiCell {}

    static PROBE_API: ProbeApiCell = ProbeApiCell(UnsafeCell::new(MaybeUninit::uninit()));
    static mut PROBE_STATE: ProbeState = ProbeState::new();

    #[repr(C)]
    struct PreserveData {
        preserve: PreserveObject,
        object: SEXP,
    }

    #[repr(C)]
    struct ActionData {
        api: ProbeApi,
        action: c_int,
    }

    unsafe extern "C" {
        fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
        fn _exit(status: c_int) -> !;
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
        let stdout = stdout_reader
            .join()
            .map_err(|_| std::io::Error::other("stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| std::io::Error::other("stderr reader panicked"))??;
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        match status {
            Some(status) if status.success() => {}
            Some(status) => {
                return Err(format!("child exited with {status}\n{stderr}\n{stdout}").into());
            }
            None => return Err(format!("child exceeded 90 seconds\n{stderr}\n{stdout}").into()),
        }

        let actual: Vec<&str> = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("ARF_NESTED_RECOVERY:"))
            .collect();
        let expected = [
            "READ:TOP:1",
            "ACTION:1:START",
            "ACTION:1:BODY",
            "NESTED:BROWSER",
            "NESTED:BROWSER_CONTINUE",
            "ACTION:1:EVAL_RETURNED",
            "ACTION:1:PRINT_STARTED",
            "ACTION:1:PRINT_RETURNED",
            "ACTION:1:CLEANUP_NORMAL",
            "ACTION:1:CALL_RETURNED",
            "READ:TOP:2",
            "ACTION:1:FINALIZED_COMPLETED",
            "ACTION:2:START",
            "ACTION:2:BODY",
            "NESTED:RECOVER",
            "NESTED:RECOVER_ZERO",
            "ACTION:2:CLEANUP_JUMP",
            "READ:TOP:3",
            "ACTION:2:FINALIZED_ABORTED_EVAL",
            "ACTION:3:START",
            "ACTION:3:BODY",
            "ACTION:3:EVAL_RETURNED",
            "ACTION:3:PRINT_STARTED",
            "OUTPUT:777",
            "ACTION:3:PRINT_RETURNED",
            "ACTION:3:CLEANUP_NORMAL",
            "ACTION:3:CALL_RETURNED",
            "READ:TOP:4",
            "ACTION:3:FINALIZED_COMPLETED",
            "EOF",
        ];
        if actual != expected {
            return Err(format!(
                "nested prompt changed the expected command outcome sequence\nexpected: {expected:?}\nactual:   {actual:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            )
            .into());
        }
        println!("nested browser/recover prompts preserved outer command outcomes");
        println!(
            "browser completed once, recover aborted evaluation once, and 777 printed after recovery"
        );
        Ok(())
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
        let library = unsafe { Library::new(arf_libr::find_r_library()?)? };
        let unwind_protect = unsafe { *library.get::<RUnwindProtect>(b"R_UnwindProtect\0")? };
        let preserve_object = unsafe { *library.get::<PreserveObject>(b"R_PreserveObject\0")? };
        let lang2 = unsafe { *library.get::<Lang2>(b"Rf_lang2\0")? };
        let setup = eval_string(
            r#"
invisible({
options(error = utils::recover)
.arf_nested_browser_function <- function() {
    browser()
    42L
}
.arf_nested_recover_function <- function() {
    stop("nested recover")
}
list(
    list(
        quote(.arf_nested_browser_function()),
        quote(.arf_nested_recover_function()),
        quote(777L)
    ),
    base::withVisible
)
})
"#,
        )?;
        let preserved_setup = setup.sexp();
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
        };
        unsafe {
            ptr::write((*PROBE_API.0.get()).as_mut_ptr(), probe_api);
            ptr::write(ptr::addr_of_mut!(PROBE_STATE), ProbeState::new());
            if api.ptr_r_readconsole.is_null() {
                return Err("R export ptr_R_ReadConsole was null".into());
            }
            if api.ptr_r_writeconsoleex.is_null() {
                return Err("R export ptr_R_WriteConsoleEx was null".into());
            }
            *api.ptr_r_readconsole = Some(read_console);
            *api.ptr_r_writeconsoleex = Some(write_console_ex);
            (api.run_rmainloop)();
        }
        drop(library);
        Ok(())
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
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                append_event(EVENT_FAIL_STATE);
                flush_events();
                return 0;
            }
            if buffer.is_null() || buffer_len < 2 {
                fail(EVENT_FAIL_BOUNDARY);
                flush_events();
                return 0;
            }

            if is_top_prompt(prompt) {
                return handle_top_prompt(buffer, buffer_len);
            }
            if is_browser_prompt(prompt) {
                return handle_browser_prompt(buffer, buffer_len);
            }
            if is_recover_prompt(prompt) {
                return handle_recover_prompt(buffer, buffer_len);
            }

            fail(EVENT_FAIL_PROMPT);
            flush_events();
            0
        }
    }

    unsafe fn handle_top_prompt(buffer: *mut c_char, buffer_len: c_int) -> c_int {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).nested_depth)) != 0
                || ptr::read_volatile(ptr::addr_of!((*state).top_callback_active)) != 0
            {
                fail(EVENT_FAIL_REENTRY);
                flush_events();
                return 0;
            }
            let top_count = ptr::read_volatile(ptr::addr_of!((*state).top_read_count)) + 1;
            ptr::write_volatile(ptr::addr_of_mut!((*state).top_read_count), top_count);
            let read_event = match top_count {
                1 => EVENT_TOP_READ_1,
                2 => EVENT_TOP_READ_2,
                3 => EVENT_TOP_READ_3,
                4 => EVENT_TOP_READ_4,
                _ => {
                    fail(EVENT_FAIL_STATE);
                    flush_events();
                    return 0;
                }
            };
            append_event(read_event);

            let active_action = ptr::read_volatile(ptr::addr_of!((*state).active_action));
            if active_action != 0 && !finalize_action(active_action) {
                flush_events();
                return 0;
            }
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                flush_events();
                return 0;
            }

            let next_action = ptr::read_volatile(ptr::addr_of!((*state).next_action));
            if next_action > ACTION_COUNT {
                if top_count != 4
                    || ptr::read_volatile(ptr::addr_of!((*state).finalized_mask)) != 0b111
                {
                    fail(EVENT_FAIL_STATE);
                } else if ptr::read_volatile(ptr::addr_of!((*state).saw_display_777)) != 1 {
                    fail(EVENT_FAIL_DISPLAY);
                } else {
                    append_event(EVENT_EOF);
                }
                flush_events();
                return 0;
            }
            begin_action(next_action);
            let api = ptr::read((*PROBE_API.0.get()).as_ptr());
            let mut action_data = ActionData {
                api,
                action: next_action,
            };
            ptr::write_volatile(ptr::addr_of_mut!((*state).armed), 1);
            ptr::write_volatile(ptr::addr_of_mut!((*state).body_entered), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).cleanup_seen), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).cleanup_jump), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).call_returned), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).top_callback_active), 1);
            (api.unwind_protect)(
                Some(action_body),
                ptr::addr_of_mut!(action_data).cast::<c_void>(),
                Some(action_cleanup),
                ptr::addr_of_mut!(action_data).cast::<c_void>(),
                ptr::null_mut(),
            );

            if ptr::read_volatile(ptr::addr_of!((*state).cleanup_seen)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).cleanup_jump)) != 0
                || ptr::read_volatile(ptr::addr_of!((*state).top_callback_active)) != 1
            {
                fail(EVENT_FAIL_CLEANUP);
                flush_events();
                return 0;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).top_callback_active), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).call_returned), 1);
            append_action_event(
                next_action,
                EVENT_ACTION_1_CALL_RETURNED,
                EVENT_ACTION_2_CALL_RETURNED,
                EVENT_ACTION_3_CALL_RETURNED,
            );
            write_input_line(buffer, buffer_len, b"\n")
        }
    }

    unsafe fn begin_action(action: c_int) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if action < 1
                || action > ACTION_COUNT
                || ptr::read_volatile(ptr::addr_of!((*state).active_action)) != 0
            {
                fail(EVENT_FAIL_STATE);
                return;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).active_action), action);
            ptr::write_volatile(ptr::addr_of_mut!((*state).next_action), action + 1);
            append_action_event(
                action,
                EVENT_ACTION_1_START,
                EVENT_ACTION_2_START,
                EVENT_ACTION_3_START,
            );
        }
    }

    unsafe fn finalize_action(action: c_int) -> bool {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let mask = 1 << (action - 1);
            let finalized = ptr::read_volatile(ptr::addr_of!((*state).finalized_mask));
            let cleanup_seen = ptr::read_volatile(ptr::addr_of!((*state).cleanup_seen));
            let cleanup_jump = ptr::read_volatile(ptr::addr_of!((*state).cleanup_jump));
            let call_returned = ptr::read_volatile(ptr::addr_of!((*state).call_returned));
            let event = match action {
                1 if cleanup_seen == 1
                    && cleanup_jump == 0
                    && call_returned == 1
                    && ptr::read_volatile(ptr::addr_of!((*state).browser_prompt_count)) == 1
                    && ptr::read_volatile(ptr::addr_of!((*state).nested_depth)) == 0 =>
                {
                    EVENT_ACTION_1_FINALIZED
                }
                2 if cleanup_seen == 1
                    && cleanup_jump != 0
                    && call_returned == 0
                    && ptr::read_volatile(ptr::addr_of!((*state).recover_prompt_count)) == 1 =>
                {
                    EVENT_ACTION_2_ABORTED_EVAL
                }
                3 if cleanup_seen == 1 && cleanup_jump == 0 && call_returned == 1 => {
                    EVENT_ACTION_3_FINALIZED
                }
                _ => {
                    fail(EVENT_FAIL_FINALIZE);
                    return false;
                }
            };
            if finalized & mask != 0 {
                fail(EVENT_FAIL_FINALIZE);
                return false;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).finalized_mask), finalized | mask);
            ptr::write_volatile(ptr::addr_of_mut!((*state).active_action), 0);
            append_event(event);
            true
        }
    }

    unsafe fn handle_browser_prompt(buffer: *mut c_char, buffer_len: c_int) -> c_int {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).active_action)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).top_callback_active)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).armed)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).nested_depth)) != 0
            {
                fail(EVENT_FAIL_NESTED);
                flush_events();
                return 0;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).nested_depth), 1);
            ptr::write_volatile(
                ptr::addr_of_mut!((*state).browser_prompt_count),
                ptr::read_volatile(ptr::addr_of!((*state).browser_prompt_count)) + 1,
            );
            append_event(EVENT_BROWSER_PROMPT);
            ptr::write_volatile(ptr::addr_of_mut!((*state).nested_depth), 0);
            append_event(EVENT_BROWSER_CONTINUE);
            write_input_line(buffer, buffer_len, b"c\n")
        }
    }

    unsafe fn handle_recover_prompt(buffer: *mut c_char, buffer_len: c_int) -> c_int {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).active_action)) != 2
                || ptr::read_volatile(ptr::addr_of!((*state).top_callback_active)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).armed)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).cleanup_seen)) != 0
                || ptr::read_volatile(ptr::addr_of!((*state).nested_depth)) != 0
            {
                fail(EVENT_FAIL_NESTED);
                flush_events();
                return 0;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).nested_depth), 1);
            ptr::write_volatile(
                ptr::addr_of_mut!((*state).recover_prompt_count),
                ptr::read_volatile(ptr::addr_of!((*state).recover_prompt_count)) + 1,
            );
            append_event(EVENT_RECOVER_PROMPT);
            ptr::write_volatile(ptr::addr_of_mut!((*state).nested_depth), 0);
            append_event(EVENT_RECOVER_ZERO);
            write_input_line(buffer, buffer_len, b"0\n")
        }
    }

    unsafe extern "C" fn action_body(data: *mut c_void) -> SEXP {
        unsafe {
            let payload = data.cast::<ActionData>();
            let api = (*payload).api;
            let action = (*payload).action;
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).armed)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).body_entered)) != 0
                || ptr::read_volatile(ptr::addr_of!((*state).active_action)) != action
            {
                fail(EVENT_FAIL_BOUNDARY);
                return api.nil_value;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).body_entered), 1);
            append_action_event(
                action,
                EVENT_ACTION_1_BODY,
                EVENT_ACTION_2_BODY,
                EVENT_ACTION_3_BODY,
            );
            let expression = (api.vector_elt)(api.expressions, (action - 1) as isize);
            let call = (api.lang2)(api.with_visible, expression);
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
            let visible_flag = (api.logical)(visible);
            if visible_flag.is_null() {
                fail(EVENT_FAIL_BOUNDARY);
                (api.unprotect)(2);
                return api.nil_value;
            }
            if ptr::read(visible_flag) != 0 {
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
            }
            (api.unprotect)(2);
            api.nil_value
        }
    }

    unsafe extern "C" fn action_cleanup(data: *mut c_void, jump: c_int) {
        unsafe {
            let payload = data.cast::<ActionData>();
            let action = (*payload).action;
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let armed = ptr::read_volatile(ptr::addr_of!((*state).armed));
            let body_entered = ptr::read_volatile(ptr::addr_of!((*state).body_entered));
            let cleanup_seen = ptr::read_volatile(ptr::addr_of!((*state).cleanup_seen));
            if armed != 1 || body_entered != 1 || cleanup_seen != 0 {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                append_event(EVENT_FAIL_CLEANUP);
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).cleanup_seen), 1);
            ptr::write_volatile(ptr::addr_of_mut!((*state).cleanup_jump), jump);
            ptr::write_volatile(ptr::addr_of_mut!((*state).armed), 0);
            if jump == 0 {
                append_action_event(
                    action,
                    EVENT_ACTION_1_CLEANUP_NORMAL,
                    EVENT_ACTION_2_CLEANUP_NORMAL,
                    EVENT_ACTION_3_CLEANUP_NORMAL,
                );
            } else {
                ptr::write_volatile(ptr::addr_of_mut!((*state).top_callback_active), 0);
                append_action_event(
                    action,
                    EVENT_ACTION_1_CLEANUP_JUMP,
                    EVENT_ACTION_2_CLEANUP_JUMP,
                    EVENT_ACTION_3_CLEANUP_JUMP,
                );
            }
        }
    }

    unsafe extern "C" fn write_console_ex(buffer: *const c_char, len: c_int, output_type: c_int) {
        if buffer.is_null() || len <= 0 || output_type != 0 {
            return;
        }
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).saw_display_777)) != 0 {
                return;
            }
            let mut matched = ptr::read_volatile(ptr::addr_of!((*state).display_match_len));
            let mut index = 0;
            while index < len as usize {
                let byte = buffer.add(index).read() as u8;
                if byte == DISPLAY_NEEDLE[matched] {
                    matched += 1;
                    if matched == DISPLAY_NEEDLE.len() {
                        ptr::write_volatile(ptr::addr_of_mut!((*state).saw_display_777), 1);
                        append_event(EVENT_OUTPUT_777);
                        matched = 0;
                        break;
                    }
                } else if byte == DISPLAY_NEEDLE[0] {
                    matched = 1;
                } else {
                    matched = 0;
                }
                index += 1;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).display_match_len), matched);
        }
    }

    unsafe fn is_top_prompt(prompt: *const c_char) -> bool {
        unsafe { prompt_equals(prompt, b"> ") }
    }

    unsafe fn is_recover_prompt(prompt: *const c_char) -> bool {
        unsafe { prompt_equals(prompt, b"Selection: ") }
    }

    unsafe fn is_browser_prompt(prompt: *const c_char) -> bool {
        if prompt.is_null() {
            return false;
        }
        unsafe {
            let prefix = b"Browse[";
            let mut index = 0;
            while index < prefix.len() {
                if prompt.add(index).read() != prefix[index] as c_char {
                    return false;
                }
                index += 1;
            }
            let mut len = index;
            while len < 64 && prompt.add(len).read() != 0 {
                len += 1;
            }
            len < 64
                && len >= prefix.len() + 3
                && prompt.add(len - 3).read() == b']' as c_char
                && prompt.add(len - 2).read() == b'>' as c_char
                && prompt.add(len - 1).read() == b' ' as c_char
        }
    }

    unsafe fn prompt_equals(prompt: *const c_char, expected: &[u8]) -> bool {
        if prompt.is_null() {
            return false;
        }
        unsafe {
            let mut index = 0;
            while index < expected.len() {
                if prompt.add(index).read() != expected[index] as c_char {
                    return false;
                }
                index += 1;
            }
            prompt.add(expected.len()).read() == 0
        }
    }

    unsafe fn write_input_line(buffer: *mut c_char, buffer_len: c_int, input: &[u8]) -> c_int {
        if input.len() + 1 > buffer_len as usize {
            unsafe { fail(EVENT_FAIL_BOUNDARY) };
            return 0;
        }
        unsafe {
            ptr::copy_nonoverlapping(input.as_ptr().cast::<c_char>(), buffer, input.len());
            buffer.add(input.len()).write(0);
        }
        1
    }

    unsafe fn append_action_event(action: c_int, one: c_int, two: c_int, three: c_int) {
        let event = match action {
            1 => one,
            2 => two,
            3 => three,
            _ => EVENT_FAIL_STATE,
        };
        unsafe { append_event(event) };
    }

    unsafe fn fail(event: c_int) {
        unsafe {
            ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
            append_event(event);
        }
    }

    unsafe fn append_event(event: c_int) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let len = ptr::read_volatile(ptr::addr_of!((*state).event_len));
            if len >= EVENT_CAPACITY {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                ptr::write_volatile(
                    ptr::addr_of_mut!((*state).events)
                        .cast::<c_int>()
                        .add(EVENT_CAPACITY - 1),
                    EVENT_FAIL_OVERFLOW,
                );
                return;
            }
            ptr::write_volatile(
                ptr::addr_of_mut!((*state).events).cast::<c_int>().add(len),
                event,
            );
            ptr::write_volatile(ptr::addr_of_mut!((*state).event_len), len + 1);
        }
    }

    unsafe fn flush_events() {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let len_ptr = ptr::addr_of_mut!((*state).event_len);
            let len = ptr::read_volatile(len_ptr);
            let events = ptr::addr_of_mut!((*state).events).cast::<c_int>();
            let mut index = 0;
            while index < len {
                let event = ptr::read_volatile(events.add(index));
                let marker = event_marker(event);
                write_all(1, MARKER_PREFIX);
                write_all(1, marker);
                write_all(1, b"\n");
                index += 1;
            }
            ptr::write_volatile(len_ptr, 0);
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                write_all(1, MARKER_PREFIX);
                write_all(1, b"FAIL:STATE\n");
                _exit(1);
            }
        }
    }

    fn event_marker(event: c_int) -> &'static [u8] {
        match event {
            EVENT_TOP_READ_1 => b"READ:TOP:1",
            EVENT_TOP_READ_2 => b"READ:TOP:2",
            EVENT_TOP_READ_3 => b"READ:TOP:3",
            EVENT_TOP_READ_4 => b"READ:TOP:4",
            EVENT_ACTION_1_START => b"ACTION:1:START",
            EVENT_ACTION_2_START => b"ACTION:2:START",
            EVENT_ACTION_3_START => b"ACTION:3:START",
            EVENT_ACTION_1_BODY => b"ACTION:1:BODY",
            EVENT_ACTION_2_BODY => b"ACTION:2:BODY",
            EVENT_ACTION_3_BODY => b"ACTION:3:BODY",
            EVENT_BROWSER_PROMPT => b"NESTED:BROWSER",
            EVENT_BROWSER_CONTINUE => b"NESTED:BROWSER_CONTINUE",
            EVENT_RECOVER_PROMPT => b"NESTED:RECOVER",
            EVENT_RECOVER_ZERO => b"NESTED:RECOVER_ZERO",
            EVENT_ACTION_1_EVAL_RETURNED => b"ACTION:1:EVAL_RETURNED",
            EVENT_ACTION_2_EVAL_RETURNED => b"ACTION:2:EVAL_RETURNED",
            EVENT_ACTION_3_EVAL_RETURNED => b"ACTION:3:EVAL_RETURNED",
            EVENT_ACTION_1_PRINT_STARTED => b"ACTION:1:PRINT_STARTED",
            EVENT_ACTION_2_PRINT_STARTED => b"ACTION:2:PRINT_STARTED",
            EVENT_ACTION_3_PRINT_STARTED => b"ACTION:3:PRINT_STARTED",
            EVENT_ACTION_1_PRINT_RETURNED => b"ACTION:1:PRINT_RETURNED",
            EVENT_ACTION_2_PRINT_RETURNED => b"ACTION:2:PRINT_RETURNED",
            EVENT_ACTION_3_PRINT_RETURNED => b"ACTION:3:PRINT_RETURNED",
            EVENT_ACTION_1_CLEANUP_NORMAL => b"ACTION:1:CLEANUP_NORMAL",
            EVENT_ACTION_2_CLEANUP_NORMAL => b"ACTION:2:CLEANUP_NORMAL",
            EVENT_ACTION_3_CLEANUP_NORMAL => b"ACTION:3:CLEANUP_NORMAL",
            EVENT_ACTION_1_CLEANUP_JUMP => b"ACTION:1:CLEANUP_JUMP",
            EVENT_ACTION_2_CLEANUP_JUMP => b"ACTION:2:CLEANUP_JUMP",
            EVENT_ACTION_3_CLEANUP_JUMP => b"ACTION:3:CLEANUP_JUMP",
            EVENT_ACTION_1_CALL_RETURNED => b"ACTION:1:CALL_RETURNED",
            EVENT_ACTION_2_CALL_RETURNED => b"ACTION:2:CALL_RETURNED",
            EVENT_ACTION_3_CALL_RETURNED => b"ACTION:3:CALL_RETURNED",
            EVENT_ACTION_1_FINALIZED => b"ACTION:1:FINALIZED_COMPLETED",
            EVENT_ACTION_2_ABORTED_EVAL => b"ACTION:2:FINALIZED_ABORTED_EVAL",
            EVENT_ACTION_3_FINALIZED => b"ACTION:3:FINALIZED_COMPLETED",
            EVENT_OUTPUT_777 => b"OUTPUT:777",
            EVENT_EOF => b"EOF",
            EVENT_FAIL_REENTRY => b"FAIL:REENTRY",
            EVENT_FAIL_PROMPT => b"FAIL:PROMPT",
            EVENT_FAIL_STATE => b"FAIL:STATE",
            EVENT_FAIL_BOUNDARY => b"FAIL:BOUNDARY",
            EVENT_FAIL_OVERFLOW => b"FAIL:EVENT_OVERFLOW",
            EVENT_FAIL_FINALIZE => b"FAIL:FINALIZE",
            EVENT_FAIL_CLEANUP => b"FAIL:CLEANUP",
            EVENT_FAIL_NESTED => b"FAIL:NESTED_PROMPT",
            EVENT_FAIL_DISPLAY => b"FAIL:DISPLAY",
            _ => b"FAIL:UNKNOWN_EVENT",
        }
    }

    unsafe fn write_all(fd: c_int, mut bytes: &[u8]) {
        unsafe {
            while !bytes.is_empty() {
                let written = write(fd, bytes.as_ptr().cast::<c_void>(), bytes.len());
                if written <= 0 {
                    ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                    return;
                }
                bytes = &bytes[written as usize..];
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::main() {
        eprintln!("nested recovery spike failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("nested recovery spike is supported only on Linux");
    std::process::exit(2);
}
