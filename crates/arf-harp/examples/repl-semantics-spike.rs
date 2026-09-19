//! Linux/R 4.5.2 diagnostic comparing native REPL behavior with a public-API
//! expression driver.
//!
//! The parent never initializes R. It starts one native and one candidate
//! child, enforces a 90-second timeout per child, and checks their exit status
//! and fixed markers. Each child embeds R, installs POD console callbacks, and
//! enters `run_Rmainloop`.
//!
//! The native child returns each fixture line unchanged. The candidate child
//! processes that line from its ReadConsole callback with a line-scoped R
//! `textConnection`, `parse(n = 1)`, `eval`, and `withVisible`. The helper keeps
//! that connection while parsing the expressions in one fixture line and
//! returns their results. The same `R_UnwindProtect` body then calls
//! `Rf_PrintValue` for visible values; its continuation is C NULL. A successful
//! candidate command returns an empty line to the native loop. Setup occurs
//! before either callback is installed. SEXP values that outlive an R
//! evaluation remain bound in the global environment; frames crossed by an R
//! longjmp retain only raw pointers and fixed-size data.
//!
//! `addTaskCallback()` is registered by setup R code. Its observations and
//! WriteConsoleEx traffic are copied into fixed POD buffers and emitted in
//! callback order at the next ReadConsole entry. The task callback API from
//! `R_ext/Callbacks.h` is non-API, so these observations are diagnostic only.
//! The results intentionally record both matches and known gaps; they do not
//! establish candidate compatibility. Native autoprint is performed inside R's
//! private REPL path. `Rf_PrintValue` uses `PrintValueEnv(..., R_GlobalEnv)` for
//! the global environment; this probe does not compare that behavior with an
//! explicit R-level `print()` call and makes no browser/local-environment claim.
//! Because candidate evaluation results are collected before visible values
//! are printed, this fixture does not establish general eval/print interleaving.
//! Print-bearing fixture lines contain one visible expression; the multi-
//! expression lines cover side effects followed by syntax failure or an
//! invisible `cat()` result followed by the final visible value.
//!
//! This fixture covers scalar and NULL visibility, an invisible value, an S3
//! print method that returns invisibly, `.Last.value` observations, warning
//! output order, print failure recovery and callback behavior, handled and
//! unhandled evaluation errors, incremental parse side effects, and a later
//! normal command. It does not cover interrupts, cancellation, nested prompts,
//! browser/recover, task callback arguments beyond the recorded fields, or
//! non-Linux/R 4.5.2 environments.

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

    const CONSOLE_EVENT_CAPACITY: usize = 256;
    const CONSOLE_BYTES: usize = 4096;
    const FIXTURES: &[&str] = &[
        "41L",
        "NULL",
        "invisible(6L)",
        "100L",
        "structure(7L, class = 'arf_repl_last_probe')",
        "local({ warning('eval-warning'); structure(8L, class = 'arf_repl_warning_probe') })",
        "structure(9L, class = 'arf_repl_print_stop_probe')",
        "cat('ARF_AFTER_PRINT_STOP_LAST|', .arf_last_tag(), '\\n', sep = ''); 42L",
        "tryCatch(stop('handled-evaluation-stop'), error = function(e) 43L)",
        "stop('unhandled-evaluation-stop')",
        ".arf_sequential_side_effect <- 1L; )",
        "cat('ARF_SEQUENTIAL_SIDE_EFFECT|', exists('.arf_sequential_side_effect', envir = .GlobalEnv, inherits = FALSE), '\\n', sep = '')",
        "777L",
    ];

    type RUnwindBody = unsafe extern "C" fn(*mut c_void) -> SEXP;
    type RUnwindCleanup = unsafe extern "C" fn(*mut c_void, c_int);
    type RUnwindProtect = unsafe extern "C" fn(
        Option<RUnwindBody>,
        *mut c_void,
        Option<RUnwindCleanup>,
        *mut c_void,
        SEXP,
    ) -> SEXP;
    type MkString = unsafe extern "C" fn(*const c_char) -> SEXP;
    type Lang2 = unsafe extern "C" fn(SEXP, SEXP) -> SEXP;
    type Install = unsafe extern "C" fn(*const c_char) -> SEXP;
    type FindVar = unsafe extern "C" fn(SEXP, SEXP) -> SEXP;
    type VectorElt = unsafe extern "C" fn(SEXP, isize) -> SEXP;
    type Logical = unsafe extern "C" fn(SEXP) -> *mut c_int;
    const MARKER_PREFIX: &[u8] = b"ARF_REPL_SEMANTICS:";

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProbeEvent {
        kind: c_int,
        output_type: c_int,
        detail: usize,
        len: usize,
        bytes: [u8; CONSOLE_BYTES],
    }

    impl ProbeEvent {
        const EMPTY: Self = Self {
            kind: 0,
            output_type: 0,
            detail: 0,
            len: 0,
            bytes: [0; CONSOLE_BYTES],
        };
    }

    #[repr(C)]
    struct ProbeState {
        mode: c_int,
        next_fixture: usize,
        callback_active: c_int,
        armed: c_int,
        body_entered: c_int,
        cleanup_count: usize,
        cleanup_jumps: usize,
        failed: c_int,
        event_count: usize,
        events: [ProbeEvent; CONSOLE_EVENT_CAPACITY],
    }

    impl ProbeState {
        const fn new() -> Self {
            Self {
                mode: 0,
                next_fixture: 0,
                callback_active: 0,
                armed: 0,
                body_entered: 0,
                cleanup_count: 0,
                cleanup_jumps: 0,
                failed: 0,
                event_count: 0,
                events: [ProbeEvent::EMPTY; CONSOLE_EVENT_CAPACITY],
            }
        }
    }

    #[derive(Clone, Copy)]
    struct ProbeApi {
        global_env: SEXP,
        nil_value: SEXP,
        eval: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
        protect: unsafe extern "C" fn(SEXP) -> SEXP,
        unprotect: unsafe extern "C" fn(c_int),
        mk_string: MkString,
        lang2: Lang2,
        install: Install,
        find_var: FindVar,
        vector_elt: VectorElt,
        logical: Logical,
        print_value: unsafe extern "C" fn(SEXP),
        length: unsafe extern "C" fn(SEXP) -> c_int,
        unbound_value: SEXP,
        unwind_protect: RUnwindProtect,
    }

    struct ProbeApiCell(UnsafeCell<MaybeUninit<ProbeApi>>);

    // R is initialized and used only on this process's main thread.
    unsafe impl Sync for ProbeApiCell {}

    static PROBE_API: ProbeApiCell = ProbeApiCell(UnsafeCell::new(MaybeUninit::uninit()));
    static mut PROBE_STATE: ProbeState = ProbeState::new();

    #[repr(C)]
    struct CandidateCall {
        api: ProbeApi,
        source: *const u8,
        source_len: usize,
    }

    unsafe extern "C" {
        fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    }

    pub fn main() -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mode) = std::env::args().nth(1)
            && mode.starts_with("--child-")
        {
            child_main(&mode)?;
            return Ok(());
        }
        parent_main()
    }

    fn parent_main() -> Result<(), Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        let native = run_child(&executable, "native")?;
        let candidate = run_child(&executable, "candidate")?;

        assert_child_transcript(&native, "native")?;
        assert_child_transcript(&candidate, "candidate")?;
        let observations = assert_semantic_observations(&native, &candidate)?;

        println!("native/candidate expected matches and known gaps detected");
        println!(
            ".Last.value in print: native={} candidate={}; after print stop: native={} candidate={}",
            observations.native_last,
            observations.candidate_last,
            observations.native_after_print_stop,
            observations.candidate_after_print_stop
        );
        println!(
            "original-expression callback: native=present candidate=absent; sequential parse side effect: native={} candidate={}",
            observations.native_sequence, observations.candidate_sequence
        );
        print_console_summary("native", &native.console);
        print_console_summary("candidate", &candidate.console);
        Ok(())
    }

    struct ChildTranscript {
        mode: &'static str,
        markers: Vec<String>,
        console: Vec<(c_int, Vec<u8>)>,
        stderr: String,
    }

    fn run_child(
        executable: &std::path::Path,
        mode: &'static str,
    ) -> Result<ChildTranscript, Box<dyn std::error::Error>> {
        let mut child = Command::new(executable)
            .arg(format!("--child-{mode}"))
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
        let stdout = stdout_reader
            .join()
            .map_err(|_| std::io::Error::other("child stdout reader panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| std::io::Error::other("child stderr reader panicked"))??;
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        match status {
            Some(status) if status.success() => {}
            Some(status) => {
                return Err(
                    format!("{mode} child exited with {status}\n{stderr}\n{stdout}").into(),
                );
            }
            None => {
                return Err(
                    format!("{mode} child exceeded its 90-second timeout\n{stderr}").into(),
                );
            }
        }
        decode_transcript(mode, &stdout, stderr)
    }

    fn read_all(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn decode_transcript(
        mode: &'static str,
        stdout: &str,
        stderr: String,
    ) -> Result<ChildTranscript, Box<dyn std::error::Error>> {
        let mut markers = Vec::new();
        let mut console = Vec::new();
        for line in stdout.lines() {
            let Some(rest) = line.strip_prefix("ARF_REPL_SEMANTICS:") else {
                continue;
            };
            if let Some(rest) = rest.strip_prefix("CONSOLE:") {
                let (output_type, hex) = rest.split_once(':').ok_or("malformed console marker")?;
                let output_type = output_type.parse::<c_int>()?;
                console.push((output_type, decode_hex(hex)?));
            } else {
                markers.push(rest.to_owned());
            }
        }
        Ok(ChildTranscript {
            mode,
            markers,
            console,
            stderr,
        })
    }

    fn decode_hex(encoded: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        if encoded.len() % 2 != 0 {
            return Err("console marker contained odd-length hex".into());
        }
        let mut decoded = Vec::with_capacity(encoded.len() / 2);
        for pair in encoded.as_bytes().chunks_exact(2) {
            let high = hex_nibble(pair[0]).ok_or("console marker had invalid hex")?;
            let low = hex_nibble(pair[1]).ok_or("console marker had invalid hex")?;
            decoded.push(high * 16 + low);
        }
        Ok(decoded)
    }

    fn hex_nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    fn assert_child_transcript(
        transcript: &ChildTranscript,
        expected_mode: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(failure) = transcript
            .markers
            .iter()
            .find(|marker| marker.starts_with("FAIL:"))
        {
            return Err(format!(
                "{} child emitted failure marker {failure:?}",
                transcript.mode
            )
            .into());
        }
        let reads: Vec<String> = transcript
            .markers
            .iter()
            .filter(|marker| marker.starts_with("READ:"))
            .cloned()
            .collect();
        let expected_reads: Vec<String> = (1..=FIXTURES.len() + 1)
            .map(|number| format!("READ:{number}"))
            .collect();
        if reads != expected_reads {
            return Err(format!(
                "{} child did not recover through all inputs\nexpected {expected_reads:?}\nactual   {reads:?}\nstderr: {}",
                transcript.mode, transcript.stderr
            )
            .into());
        }
        if !transcript.markers.iter().any(|marker| marker == "EOF") {
            return Err(format!("{} child did not reach EOF", transcript.mode).into());
        }
        if transcript.console.is_empty() {
            return Err(format!(
                "{} child captured no WriteConsoleEx events",
                transcript.mode
            )
            .into());
        }
        if transcript.mode != expected_mode {
            return Err("child mode bookkeeping was inconsistent".into());
        }
        if expected_mode == "candidate" {
            let normal = transcript
                .markers
                .iter()
                .filter(|marker| marker.starts_with("CLEANUP:NORMAL:"))
                .count();
            let jumped = transcript
                .markers
                .iter()
                .filter(|marker| marker.starts_with("CLEANUP:JUMP:"))
                .count();
            if normal + jumped != FIXTURES.len() {
                return Err(format!(
                    "candidate cleanup count was {normal} normal + {jumped} jumped, expected {}",
                    FIXTURES.len()
                )
                .into());
            }
            if normal < FIXTURES.len() - 3 || jumped < 3 {
                return Err(format!(
                    "candidate cleanup distribution was unexpected: {normal} normal, {jumped} jumped"
                )
                .into());
            }
        }
        Ok(())
    }

    struct SemanticObservations {
        native_last: String,
        candidate_last: String,
        native_after_print_stop: String,
        candidate_after_print_stop: String,
        native_sequence: String,
        candidate_sequence: String,
    }

    fn assert_semantic_observations(
        native: &ChildTranscript,
        candidate: &ChildTranscript,
    ) -> Result<SemanticObservations, Box<dyn std::error::Error>> {
        let native_text = console_text(native);
        let candidate_text = console_text(candidate);

        for (name, text) in [("native", &native_text), ("candidate", &candidate_text)] {
            for visible_output in ["[1] 41", "NULL\n", "[1] 43", "[1] 42", "[1] 777"] {
                if !text.contains(visible_output) {
                    return Err(format!(
                        "{name} child missed expected visible output {visible_output:?}\n{text}"
                    )
                    .into());
                }
            }
            for invisible_output in ["[1] 6\n", "[1] 7\n", "[1] 8\n", "[1] 9\n"] {
                if text.contains(invisible_output) {
                    return Err(format!(
                        "{name} child printed an invisible or print-method-owned value {invisible_output:?}\n{text}"
                    )
                    .into());
                }
            }
            for marker in [
                "ARF_LAST_VALUE|",
                "ARF_WARNING_PRINT_BODY|",
                "ARF_AFTER_PRINT_STOP_LAST|",
                "ARF_SEQUENTIAL_SIDE_EFFECT|",
            ] {
                if marker != "ARF_TASK|" && !text.contains(marker) {
                    return Err(format!("{name} child did not emit {marker:?}\n{text}").into());
                }
            }
        }
        assert_task_callback_record(
            &native_text,
            "ARF_TASK|kind=scalar|ast_match=TRUE|source_match=FALSE|expr=41L|value=integer:41|ok=TRUE|visible=TRUE",
        )?;
        assert_task_callback_record(
            &native_text,
            "ARF_TASK|kind=s3|ast_match=TRUE|source_match=FALSE|expr=structure(7L, class = \"arf_repl_last_probe\")|value=integer:7|ok=TRUE|visible=TRUE",
        )?;
        assert_task_callback_record(
            &native_text,
            "ARF_TASK|kind=warning|ast_match=FALSE|source_match=TRUE|",
        )?;
        if candidate_text.contains("ARF_TASK|") {
            return Err(format!(
                "candidate emitted {} task callback markers; expected exactly zero\n{candidate_text}",
                candidate_text.matches("ARF_TASK|").count()
            )
            .into());
        }
        assert_warning_event_order(native, true)?;
        assert_warning_event_order(candidate, false)?;

        let native_last = extract_tag(&native_text, "ARF_LAST_VALUE|")?;
        let candidate_last = extract_tag(&candidate_text, "ARF_LAST_VALUE|")?;
        if native_last != "S3" || candidate_last != "logical:TRUE" {
            return Err(format!(
                "unexpected .Last.value seen by print methods: native={native_last:?} (expected S3), candidate={candidate_last:?} (expected logical:TRUE)\nNATIVE:\n{native_text}\nCANDIDATE:\n{candidate_text}"
            )
            .into());
        }
        let native_after_stop = extract_tag(&native_text, "ARF_AFTER_PRINT_STOP_LAST|")?;
        let candidate_after_stop = extract_tag(&candidate_text, "ARF_AFTER_PRINT_STOP_LAST|")?;
        if native_after_stop != "integer:9" || candidate_after_stop != "logical:TRUE" {
            return Err(format!(
                "unexpected .Last.value after print-stop recovery: native={native_after_stop:?} (expected integer:9), candidate={candidate_after_stop:?} (expected logical:TRUE)"
            )
            .into());
        }

        let native_sequence = extract_tag(&native_text, "ARF_SEQUENTIAL_SIDE_EFFECT|")?;
        let candidate_sequence = extract_tag(&candidate_text, "ARF_SEQUENTIAL_SIDE_EFFECT|")?;
        if native_sequence != "TRUE" || candidate_sequence != "TRUE" {
            return Err(format!(
                "incremental parse side-effect result differed from the recorded expectation: native={native_sequence:?}, candidate={candidate_sequence:?}"
            )
            .into());
        }

        let native_print_stop_callbacks =
            task_callback_mentions(&native_text, "arf_repl_print_stop_probe");
        let candidate_print_stop_callbacks =
            task_callback_mentions(&candidate_text, "arf_repl_print_stop_probe");
        let native_eval_abort_callbacks =
            task_callback_mentions(&native_text, "unhandled-evaluation-stop");
        if native_print_stop_callbacks
            || candidate_print_stop_callbacks
            || native_eval_abort_callbacks
        {
            return Err(format!(
                "a callback was observed for an aborted expression: native-print={native_print_stop_callbacks}, candidate-print={candidate_print_stop_callbacks}, native-eval={native_eval_abort_callbacks}"
            )
            .into());
        }

        Ok(SemanticObservations {
            native_last: native_last.to_owned(),
            candidate_last: candidate_last.to_owned(),
            native_after_print_stop: native_after_stop.to_owned(),
            candidate_after_print_stop: candidate_after_stop.to_owned(),
            native_sequence: native_sequence.to_owned(),
            candidate_sequence: candidate_sequence.to_owned(),
        })
    }

    fn console_text(transcript: &ChildTranscript) -> String {
        let mut text = String::new();
        for (_, bytes) in &transcript.console {
            text.push_str(&String::from_utf8_lossy(bytes));
        }
        text
    }

    fn assert_task_callback_record(
        text: &str,
        expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !text.contains(expected) {
            return Err(format!("missing exact task callback record {expected:?}\n{text}").into());
        }
        Ok(())
    }

    struct ConsoleStream {
        bytes: Vec<u8>,
        output_types: Vec<c_int>,
        event_ordinals: Vec<usize>,
    }

    struct LocatedConsoleText {
        start: usize,
        end: usize,
        first_event: usize,
        last_event: usize,
    }

    impl ConsoleStream {
        fn new(events: &[(c_int, Vec<u8>)]) -> Self {
            let byte_count = events.iter().map(|(_, bytes)| bytes.len()).sum();
            let mut stream = Self {
                bytes: Vec::with_capacity(byte_count),
                output_types: Vec::with_capacity(byte_count),
                event_ordinals: Vec::with_capacity(byte_count),
            };
            for (ordinal, (output_type, bytes)) in events.iter().enumerate() {
                stream.bytes.extend_from_slice(bytes);
                stream
                    .output_types
                    .extend(std::iter::repeat_n(*output_type, bytes.len()));
                stream
                    .event_ordinals
                    .extend(std::iter::repeat_n(ordinal, bytes.len()));
            }
            stream
        }

        fn find(&self, needle: &str) -> Option<LocatedConsoleText> {
            let start = self
                .bytes
                .windows(needle.len())
                .position(|window| window == needle.as_bytes())?;
            let end = start + needle.len();
            Some(LocatedConsoleText {
                start,
                end,
                first_event: self.event_ordinals[start],
                last_event: self.event_ordinals[end - 1],
            })
        }

        fn assert_type(
            &self,
            needle: &str,
            expected_type: c_int,
        ) -> Result<LocatedConsoleText, Box<dyn std::error::Error>> {
            let located = self
                .find(needle)
                .ok_or_else(|| format!("missing WriteConsoleEx text {needle:?}"))?;
            if self.output_types[located.start..located.end]
                .iter()
                .any(|output_type| *output_type != expected_type)
            {
                return Err(format!(
                    "WriteConsoleEx text {needle:?} did not come entirely from output type {expected_type}"
                )
                .into());
            }
            Ok(located)
        }
    }

    fn assert_warning_event_order(
        transcript: &ChildTranscript,
        expect_task_callback: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let stream = ConsoleStream::new(&transcript.console);
        let print_body = stream.assert_type("ARF_WARNING_PRINT_BODY|", 0)?;
        let eval_warning = stream.assert_type("eval-warning", 1)?;
        let print_warning = stream.assert_type("print-warning", 1)?;
        if !(print_body.start < eval_warning.start && eval_warning.start < print_warning.start) {
            return Err(format!(
                "WriteConsoleEx order was not stdout print body < stderr eval warning < stderr print warning for {} child",
                transcript.mode
            )
            .into());
        }
        if expect_task_callback {
            let callback = stream.assert_type(
                "ARF_TASK|kind=warning|ast_match=FALSE|source_match=TRUE|",
                0,
            )?;
            if print_warning.end > callback.start {
                return Err(format!(
                    "native task callback preceded the print warning output (events {}..{} vs {})",
                    print_warning.first_event, print_warning.last_event, callback.first_event
                )
                .into());
            }
        }
        Ok(())
    }

    fn print_console_summary(mode: &str, events: &[(c_int, Vec<u8>)]) {
        let stdout_events = events.iter().filter(|(kind, _)| *kind == 0).count();
        let stderr_events = events.iter().filter(|(kind, _)| *kind != 0).count();
        let mut transcript = String::new();
        for (_, bytes) in events {
            transcript.push_str(&String::from_utf8_lossy(bytes));
        }
        let callback_suffix = if mode == "native" {
            " < task callback(stdout)"
        } else {
            " (no task callback)"
        };
        println!(
            "{mode} WriteConsoleEx events: {stdout_events} stdout, {stderr_events} stderr; verified print body(stdout) < eval warning(stderr) < print warning(stderr){callback_suffix}; captured {} bytes",
            transcript.len()
        );
    }

    fn extract_tag<'a>(text: &'a str, tag: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
        let start = text
            .find(tag)
            .ok_or_else(|| format!("missing observation {tag:?}"))?
            + tag.len();
        let rest = &text[start..];
        let value = rest
            .split(['\r', '\n'])
            .next()
            .ok_or("observation marker was empty")?
            .trim();
        Ok(value)
    }

    fn task_callback_mentions(text: &str, fragment: &str) -> bool {
        text.lines()
            .any(|line| line.contains("ARF_TASK|") && line.contains(fragment))
    }

    fn child_main(mode: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mode_value = match mode {
            "--child-native" => 1,
            "--child-candidate" => 2,
            _ => return Err(format!("unsupported child mode {mode:?}").into()),
        };
        unsafe {
            std::env::set_var("R_DEFAULT_PACKAGES", "NULL");
            arf_libr::initialize_r_with_args(&[
                "--vanilla",
                "--no-save",
                "--quiet",
                "--interactive",
            ])?;
        }
        let version_check = eval_string(
            r#"
if (!identical(as.character(getRversion()), "4.5.2")) {
    stop("this diagnostic is limited to R 4.5.2")
}
"#,
        )?;
        drop(version_check);

        let api = r_library()?;
        let library = unsafe { Library::new(arf_libr::find_r_library()?)? };
        let unwind_protect = unsafe { *library.get::<RUnwindProtect>(b"R_UnwindProtect\0")? };
        let setup = eval_string(
            r#"
.arf_last_tag <- function() {
    if (!exists(".Last.value", envir = .GlobalEnv, inherits = TRUE)) return("UNBOUND")
    value <- get(".Last.value", envir = .GlobalEnv, inherits = TRUE)
    if (inherits(value, "arf_repl_last_probe")) return("S3")
    if (is.null(value)) return("NULL")
    if (is.atomic(value) && length(value) == 1L) return(paste0(typeof(value), ":", as.character(value)))
    paste0(typeof(value), ":", length(value))
}
print.arf_repl_last_probe <- function(x, ...) {
    cat("ARF_LAST_VALUE|", .arf_last_tag(), "\n", sep = "")
    invisible(x)
}
print.arf_repl_warning_probe <- function(x, ...) {
    cat("ARF_WARNING_PRINT_BODY|entered\n")
    warning("print-warning")
    invisible(x)
}
print.arf_repl_print_stop_probe <- function(x, ...) {
    stop("intentional print-method stop")
}
.arf_expected_scalar_expr <- quote(41L)
.arf_expected_s3_expr <- quote(structure(7L, class = "arf_repl_last_probe"))
.arf_expected_eval_abort_expr <- quote(stop("unhandled-evaluation-stop"))
.arf_process_source_line <- function(source) {
    input <- textConnection(source, open = "r")
    on.exit(close(input))
    results <- list()
    repeat {
        parsed <- parse(input, n = 1L)
        if (length(parsed) == 0L) return(invisible(results))
        current <- parsed[[1L]]
        results[[length(results) + 1L]] <- withVisible(eval(current, envir = .GlobalEnv))
    }
}
.arf_task_log <- list()
.arf_task_observer <- function(expr, value, ok, visible) {
    expression_text <- paste(deparse(expr), collapse = "")
    scalar_match <- identical(expr, .arf_expected_scalar_expr)
    s3_match <- identical(expr, .arf_expected_s3_expr)
    warning_source_match <- grepl("eval-warning", expression_text, fixed = TRUE) &&
        grepl("arf_repl_warning_probe", expression_text, fixed = TRUE)
    eval_abort_match <- identical(expr, .arf_expected_eval_abort_expr)
    kind <- if (scalar_match) {
        "scalar"
    } else if (s3_match) {
        "s3"
    } else if (warning_source_match) {
        "warning"
    } else if (eval_abort_match) {
        "eval-abort"
    } else {
        "other"
    }
    ast_match <- scalar_match || s3_match || eval_abort_match
    value_text <- if (is.null(value)) {
        "NULL"
    } else if (is.atomic(value) && length(value) == 1L) {
        paste0(typeof(value), ":", as.character(value))
    } else {
        paste0(typeof(value), ":", length(value))
    }
    item <- list(kind = kind, ast_match = ast_match,
                 source_match = warning_source_match, expr = expression_text,
                 value = value_text, ok = ok, visible = visible)
    .arf_task_log[[length(.arf_task_log) + 1L]] <<- item
    cat("ARF_TASK|kind=", kind,
        "|ast_match=", ast_match,
        "|source_match=", warning_source_match,
        "|expr=", expression_text,
        "|value=", value_text,
        "|ok=", ok,
        "|visible=", visible,
        "\n", sep = "")
    TRUE
}
addTaskCallback(.arf_task_observer, name = "arf-repl-semantics-observer")
"#,
        )?;
        drop(setup);

        let probe_api = ProbeApi {
            global_env: unsafe { *api.r_globalenv },
            nil_value: unsafe { *api.r_nilvalue },
            eval: api.rf_eval,
            protect: api.rf_protect,
            unprotect: api.rf_unprotect,
            mk_string: api.rf_mkstring,
            lang2: unsafe { *library.get::<Lang2>(b"Rf_lang2\0")? },
            install: api.rf_install,
            find_var: api.rf_findvar,
            vector_elt: api.vector_elt,
            logical: api.logical,
            print_value: api.rf_printvalue,
            length: api.rf_length,
            unbound_value: unsafe { *api.r_unboundvalue },
            unwind_protect,
        };
        unsafe {
            ptr::write((*PROBE_API.0.get()).as_mut_ptr(), probe_api);
            ptr::write(
                ptr::addr_of_mut!(PROBE_STATE),
                ProbeState {
                    mode: mode_value,
                    ..ProbeState::new()
                },
            );
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

    unsafe extern "C" fn write_console_ex(buffer: *const c_char, len: c_int, output_type: c_int) {
        if buffer.is_null() || len <= 0 {
            return;
        }
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let count = ptr::read_volatile(ptr::addr_of!((*state).event_count));
            if count >= CONSOLE_EVENT_CAPACITY || len as usize > CONSOLE_BYTES {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                return;
            }
            let event_ptr = ptr::addr_of_mut!((*state).events)
                .cast::<ProbeEvent>()
                .add(count);
            ptr::write_volatile(ptr::addr_of_mut!((*event_ptr).kind), 1);
            ptr::write_volatile(ptr::addr_of_mut!((*event_ptr).output_type), output_type);
            ptr::write_volatile(ptr::addr_of_mut!((*event_ptr).len), len as usize);
            let destination = ptr::addr_of_mut!((*event_ptr).bytes).cast::<u8>();
            ptr::copy_nonoverlapping(buffer.cast::<u8>(), destination, len as usize);
            ptr::write_volatile(ptr::addr_of_mut!((*state).event_count), count + 1);
        }
    }

    unsafe extern "C" fn read_console(
        prompt: *const c_char,
        buffer: *mut c_char,
        buffer_len: c_int,
        _history: c_int,
    ) -> c_int {
        unsafe {
            flush_pending_events();
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).callback_active)) != 0 {
                emit_simple_marker(b"FAIL:READ_REENTRY\n");
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                return 0;
            }
            if !is_top_level_prompt(prompt) {
                emit_simple_marker(b"FAIL:UNEXPECTED_PROMPT\n");
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                return 0;
            }
            let fixture_index = ptr::read_volatile(ptr::addr_of!((*state).next_fixture));
            emit_index_marker(b"READ:", fixture_index + 1, b"\n");
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                emit_simple_marker(b"FAIL:CALLBACK_STATE\n");
                return 0;
            }
            if fixture_index == FIXTURES.len() {
                emit_simple_marker(b"EOF\n");
                return 0;
            }
            if buffer.is_null() || buffer_len < 2 {
                emit_simple_marker(b"FAIL:INVALID_INPUT_BUFFER\n");
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                return 0;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).next_fixture), fixture_index + 1);
            if ptr::read_volatile(ptr::addr_of!((*state).mode)) == 1 {
                return return_fixture_line(buffer, buffer_len, fixture_index);
            }
            run_candidate_fixture(buffer, buffer_len, fixture_index)
        }
    }

    unsafe fn return_fixture_line(buffer: *mut c_char, buffer_len: c_int, index: usize) -> c_int {
        unsafe {
            let source = FIXTURES[index].as_bytes();
            if source.len() + 2 > buffer_len as usize {
                emit_simple_marker(b"FAIL:FIXTURE_TOO_LONG\n");
                ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                return 0;
            }
            ptr::copy_nonoverlapping(source.as_ptr().cast::<c_char>(), buffer, source.len());
            buffer.add(source.len()).write(b'\n' as c_char);
            buffer.add(source.len() + 1).write(0);
            1
        }
    }

    unsafe fn run_candidate_fixture(
        buffer: *mut c_char,
        buffer_len: c_int,
        fixture_index: usize,
    ) -> c_int {
        unsafe {
            let source = FIXTURES[fixture_index].as_bytes();
            if buffer_len < 2 || source.len() >= CONSOLE_BYTES {
                emit_simple_marker(b"FAIL:CANDIDATE_FIXTURE_TOO_LONG\n");
                ptr::write_volatile(ptr::addr_of_mut!(PROBE_STATE.failed), 1);
                return 0;
            }
            let api = ptr::read((*PROBE_API.0.get()).as_ptr());
            let mut call_data = CandidateCall {
                api,
                source: source.as_ptr(),
                source_len: source.len(),
            };
            let state = ptr::addr_of_mut!(PROBE_STATE);
            ptr::write_volatile(ptr::addr_of_mut!((*state).armed), 1);
            ptr::write_volatile(ptr::addr_of_mut!((*state).body_entered), 0);
            ptr::write_volatile(ptr::addr_of_mut!((*state).callback_active), 1);
            (api.unwind_protect)(
                Some(candidate_body),
                ptr::addr_of_mut!(call_data).cast::<c_void>(),
                Some(candidate_cleanup),
                ptr::addr_of_mut!(call_data).cast::<c_void>(),
                ptr::null_mut(),
            );
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                return 0;
            }
            buffer.write(b'\n' as c_char);
            buffer.add(1).write(0);
            1
        }
    }

    unsafe extern "C" fn candidate_body(data: *mut c_void) -> SEXP {
        unsafe {
            let payload = data.cast::<CandidateCall>();
            let api = (*payload).api;
            let state = ptr::addr_of_mut!(PROBE_STATE);
            if ptr::read_volatile(ptr::addr_of!((*state).armed)) != 1
                || ptr::read_volatile(ptr::addr_of!((*state).body_entered)) != 0
            {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                append_cleanup_event(4, ptr::read_volatile(ptr::addr_of!((*state).next_fixture)));
                return api.nil_value;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).body_entered), 1);

            if (*payload).source_len >= CONSOLE_BYTES {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                append_cleanup_event(4, ptr::read_volatile(ptr::addr_of!((*state).next_fixture)));
                return api.nil_value;
            }
            let mut source_text = [0 as c_char; CONSOLE_BYTES];
            ptr::copy_nonoverlapping(
                (*payload).source.cast::<c_char>(),
                source_text.as_mut_ptr(),
                (*payload).source_len,
            );
            let source_value = (api.mk_string)(source_text.as_ptr());
            (api.protect)(source_value);

            let function_symbol = (api.install)(c".arf_process_source_line".as_ptr());
            let function = (api.find_var)(function_symbol, api.global_env);
            if function.is_null() || function == api.unbound_value {
                (api.unprotect)(1);
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                append_cleanup_event(4, ptr::read_volatile(ptr::addr_of!((*state).next_fixture)));
                return api.nil_value;
            }
            let call = (api.lang2)(function, source_value);
            (api.protect)(call);
            let results = (api.eval)(call, api.global_env);
            (api.protect)(results);

            let result_count = (api.length)(results);
            let mut result_index = 0;
            while result_index < result_count {
                let visible_result = (api.vector_elt)(results, result_index as isize);
                if visible_result.is_null() || (api.length)(visible_result) < 2 {
                    ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                    append_cleanup_event(
                        4,
                        ptr::read_volatile(ptr::addr_of!((*state).next_fixture)),
                    );
                    (api.unprotect)(3);
                    return api.nil_value;
                }
                let value = (api.vector_elt)(visible_result, 0);
                let visible = (api.vector_elt)(visible_result, 1);
                let visible_flag = (api.logical)(visible);
                if visible_flag.is_null() {
                    ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                    append_cleanup_event(
                        4,
                        ptr::read_volatile(ptr::addr_of!((*state).next_fixture)),
                    );
                    (api.unprotect)(3);
                    return api.nil_value;
                }
                if ptr::read(visible_flag) != 0 {
                    (api.print_value)(value);
                }
                result_index += 1;
            }
            (api.unprotect)(3);
            api.nil_value
        }
    }

    unsafe extern "C" fn candidate_cleanup(data: *mut c_void, jump: c_int) {
        unsafe {
            let payload = data.cast::<CandidateCall>();
            let state = ptr::addr_of_mut!(PROBE_STATE);
            ptr::write_volatile(ptr::addr_of_mut!((*state).callback_active), 0);
            let armed = ptr::read_volatile(ptr::addr_of!((*state).armed));
            let body_entered = ptr::read_volatile(ptr::addr_of!((*state).body_entered));
            if armed != 1 || body_entered != 1 {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                append_cleanup_event(4, ptr::read_volatile(ptr::addr_of!((*state).next_fixture)));
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).armed), 0);
            let cleanup_number = ptr::read_volatile(ptr::addr_of!((*state).cleanup_count)) + 1;
            ptr::write_volatile(ptr::addr_of_mut!((*state).cleanup_count), cleanup_number);
            if jump == 0 {
                append_cleanup_event(2, cleanup_number);
            } else {
                ptr::write_volatile(
                    ptr::addr_of_mut!((*state).cleanup_jumps),
                    ptr::read_volatile(ptr::addr_of!((*state).cleanup_jumps)) + 1,
                );
                append_cleanup_event(3, cleanup_number);
            }
            let _ = (*payload).source;
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

    unsafe fn append_cleanup_event(kind: c_int, cleanup_number: usize) {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let count = ptr::read_volatile(ptr::addr_of!((*state).event_count));
            if count >= CONSOLE_EVENT_CAPACITY {
                ptr::write_volatile(ptr::addr_of_mut!((*state).failed), 1);
                return;
            }
            let event_ptr = ptr::addr_of_mut!((*state).events)
                .cast::<ProbeEvent>()
                .add(count);
            ptr::write_volatile(ptr::addr_of_mut!((*event_ptr).kind), kind);
            ptr::write_volatile(ptr::addr_of_mut!((*event_ptr).detail), cleanup_number);
            ptr::write_volatile(ptr::addr_of_mut!((*state).event_count), count + 1);
        }
    }

    unsafe fn flush_pending_events() {
        unsafe {
            let state = ptr::addr_of_mut!(PROBE_STATE);
            let count = ptr::read_volatile(ptr::addr_of!((*state).event_count));
            let events = ptr::addr_of_mut!((*state).events).cast::<ProbeEvent>();
            let mut index = 0;
            while index < count {
                let event = events.add(index);
                let kind = ptr::read_volatile(ptr::addr_of!((*event).kind));
                if kind == 1 {
                    let output_type = ptr::read_volatile(ptr::addr_of!((*event).output_type));
                    let len = ptr::read_volatile(ptr::addr_of!((*event).len));
                    emit_bytes(1, MARKER_PREFIX);
                    emit_bytes(1, b"CONSOLE:");
                    emit_number(1, output_type as usize);
                    emit_bytes(1, b":");
                    let mut byte_index = 0;
                    let bytes = ptr::addr_of!((*event).bytes).cast::<u8>();
                    while byte_index < len {
                        let byte = ptr::read_volatile(bytes.add(byte_index));
                        emit_hex_byte(byte);
                        byte_index += 1;
                    }
                    emit_bytes(1, b"\n");
                } else if kind == 2 {
                    let number = ptr::read_volatile(ptr::addr_of!((*event).detail));
                    emit_index_marker(b"CLEANUP:NORMAL:", number, b"\n");
                } else if kind == 3 {
                    let number = ptr::read_volatile(ptr::addr_of!((*event).detail));
                    emit_index_marker(b"CLEANUP:JUMP:", number, b"\n");
                } else if kind == 4 {
                    let number = ptr::read_volatile(ptr::addr_of!((*event).detail));
                    emit_index_marker(b"FAIL:PRE_BODY_BOUNDARY:", number, b"\n");
                }
                index += 1;
            }
            ptr::write_volatile(ptr::addr_of_mut!((*state).event_count), 0);
            if ptr::read_volatile(ptr::addr_of!((*state).failed)) != 0 {
                emit_simple_marker(b"FAIL:EVENT_LOG_OVERFLOW\n");
            }
        }
    }

    unsafe fn emit_simple_marker(suffix: &[u8]) {
        unsafe {
            emit_bytes(1, MARKER_PREFIX);
            emit_bytes(1, suffix);
        }
    }

    unsafe fn emit_index_marker(prefix: &[u8], number: usize, suffix: &[u8]) {
        unsafe {
            emit_bytes(1, MARKER_PREFIX);
            emit_bytes(1, prefix);
            emit_number(1, number);
            emit_bytes(1, suffix);
        }
    }

    unsafe fn emit_hex_byte(byte: u8) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let pair = [HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]];
        unsafe { emit_bytes(1, &pair) };
    }

    unsafe fn emit_number(fd: c_int, mut number: usize) {
        let mut digits = [0u8; 24];
        let mut len = 0;
        loop {
            digits[len] = b'0' + (number % 10) as u8;
            len += 1;
            number /= 10;
            if number == 0 || len == digits.len() {
                break;
            }
        }
        unsafe {
            while len > 0 {
                len -= 1;
                emit_bytes(fd, &digits[len..len + 1]);
            }
        }
    }

    unsafe fn emit_bytes(fd: c_int, mut bytes: &[u8]) {
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
        eprintln!("REPL semantics spike failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("REPL semantics spike is supported only on Linux");
    std::process::exit(2);
}
