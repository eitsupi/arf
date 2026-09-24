use arf_libr::{ReplFact, ReplOutcome, install_repl_driver};
use arf_libr::{
    ReplInputMode, ReplInputResult, ReplPromptClass, classify_repl_prompt, copy_repl_source,
};
use std::ffi::c_void;
use std::io::Read;
use std::os::raw::{c_char, c_int};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static INPUT_INDEX: AtomicUsize = AtomicUsize::new(0);
static OUTCOME_INDEX: AtomicUsize = AtomicUsize::new(0);
static FAILURES: AtomicUsize = AtomicUsize::new(0);
static NESTED_INPUT_PENDING: AtomicUsize = AtomicUsize::new(0);
static RLANG_AVAILABLE: AtomicUsize = AtomicUsize::new(0);

const INPUTS: [&[u8]; 11] = [
    b"42L",
    b".arf_incremental_side_effect <- 7L; 1 + * 2",
    b"stop('uncaught eval')",
    b".arf_repl_print_abort",
    b"tryCatch(stop('handled'), error = function(e) 42)",
    b"stopifnot(identical(.arf_incremental_side_effect, 7L)); 42L",
    b"stopifnot(identical(readline('> '), 'nested input')); 7L",
    b"Sys.sleep(0.01)",
    b"gctorture(100); local({ x <- lapply(seq_len(100), function(i) paste0('gc-', i)); x[[100L]] }); invisible(gctorture(FALSE))",
    b"NULL",
    b"invisible(987654321L)",
];

const EXPECTED: [ReplOutcome; 14] = [
    ReplOutcome {
        command_id: 100,
        expression_id: 1,
        fact: ReplFact::Unobserved,
    },
    ReplOutcome {
        command_id: 101,
        expression_id: 2,
        fact: ReplFact::AbortedParse,
    },
    ReplOutcome {
        command_id: 102,
        expression_id: 1,
        fact: ReplFact::AbortedEval,
    },
    ReplOutcome {
        command_id: 103,
        expression_id: 1,
        fact: ReplFact::AbortedPrint,
    },
    ReplOutcome {
        command_id: 104,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 105,
        expression_id: 2,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 106,
        expression_id: 2,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 107,
        expression_id: 1,
        fact: ReplFact::AbortedEval,
    },
    ReplOutcome {
        command_id: 108,
        expression_id: 3,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 109,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 110,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 112,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 114,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
    ReplOutcome {
        command_id: 115,
        expression_id: 1,
        fact: ReplFact::Completed,
    },
];

unsafe extern "C" {
    fn arf_repl_driver_test_skip_next_boundary();
}

unsafe extern "C" fn top_level_prompt(prompt: *const c_char, _context: *mut c_void) -> c_int {
    let class = classify_repl_prompt();
    if class != ReplPromptClass::TopLevel {
        return class as c_int;
    }
    if !prompt.is_null() && unsafe { std::ffi::CStr::from_ptr(prompt) }.to_bytes() == b"> " {
        ReplPromptClass::TopLevel as c_int
    } else {
        marker("ARF_PROMPT_CLASSIFICATION_FAILED");
        ReplPromptClass::Unobserved as c_int
    }
}

unsafe extern "C" fn input_callback(
    mode: c_int,
    prompt: *const c_char,
    buffer: *mut c_char,
    buffer_len: c_int,
    _history: c_int,
    full_source: *mut *mut c_char,
    command_id: *mut u64,
    _context: *mut c_void,
) -> c_int {
    if NESTED_INPUT_PENDING.load(Ordering::SeqCst) != 0
        && !prompt.is_null()
        && unsafe { std::ffi::CStr::from_ptr(prompt) }.to_bytes() == b"> "
    {
        let input = b"nested input";
        if mode != ReplInputMode::Nested as c_int
            || buffer.is_null()
            || command_id.is_null()
            || full_source.is_null()
            || buffer_len as usize <= input.len() + 1
        {
            FAILURES.fetch_add(1, Ordering::SeqCst);
            return 0;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr().cast::<c_char>(), buffer, input.len());
            buffer.add(input.len()).write(b'\n' as c_char);
            buffer.add(input.len() + 1).write(0);
            *command_id = 0;
        }
        NESTED_INPUT_PENDING.store(0, Ordering::SeqCst);
        marker("ARF_NESTED_INPUT");
        return ReplInputResult::Text as c_int;
    }

    let index = INPUT_INDEX.fetch_add(1, Ordering::SeqCst);
    let (input, selected_command_id) = if let Some(input) = INPUTS.get(index) {
        (input.to_vec(), 100 + index as u64)
    } else if index == INPUTS.len() {
        (
            format!("{}43L", "# source padding beyond R buffer\n".repeat(600)).into_bytes(),
            112,
        )
    } else if index == INPUTS.len() + 1 {
        (b"# comment-only source\n".to_vec(), 114_u64)
    } else if index == INPUTS.len() + 2 {
        (b"   \n\t".to_vec(), 115_u64)
    } else if index == INPUTS.len() + 3 && RLANG_AVAILABLE.load(Ordering::SeqCst) != 0 {
        (b"rlang::abort('uncaught rlang abort')".to_vec(), 111_u64)
    } else if index == INPUTS.len() + 3 {
        (Vec::new(), 113_u64)
    } else if index == INPUTS.len() + 4 && RLANG_AVAILABLE.load(Ordering::SeqCst) != 0 {
        (Vec::new(), 113_u64)
    } else {
        let cancel_index =
            INPUTS.len() + 4 + usize::from(RLANG_AVAILABLE.load(Ordering::SeqCst) != 0);
        if index == cancel_index {
            marker("ARF_DRIVER_CANCELLED");
            return ReplInputResult::Cancelled as c_int;
        }
        marker(if FAILURES.load(Ordering::SeqCst) == 0 {
            "ARF_DRIVER_OK"
        } else {
            "ARF_DRIVER_FAILURES"
        });
        marker("ARF_DRIVER_EOF");
        return ReplInputResult::Eof as c_int;
    };
    if command_id.is_null() || full_source.is_null() || mode != ReplInputMode::TopLevel as c_int {
        FAILURES.fetch_add(1, Ordering::SeqCst);
        return ReplInputResult::Eof as c_int;
    }
    unsafe {
        *full_source = copy_repl_source(std::str::from_utf8(&input).unwrap())
            .unwrap_or_else(|| std::ptr::null_mut());
        if (*full_source).is_null() {
            FAILURES.fetch_add(1, Ordering::SeqCst);
            return ReplInputResult::Eof as c_int;
        }
        *command_id = selected_command_id;
    }
    if index == 6 {
        NESTED_INPUT_PENDING.store(1, Ordering::SeqCst);
    }
    if index == 7 {
        arf_libr::set_r_interrupt_pending();
    }
    ReplInputResult::Text as c_int
}

unsafe extern "C" fn outcome_callback(
    command_id: u64,
    expression_id: u32,
    fact: u8,
    _context: *mut c_void,
) {
    let index = OUTCOME_INDEX.fetch_add(1, Ordering::SeqCst);
    let rlang_expected = ReplOutcome {
        command_id: 111,
        expression_id: 1,
        fact: ReplFact::AbortedEval,
    };
    let expected = EXPECTED.get(index).or_else(|| {
        if index != EXPECTED.len() {
            None
        } else if RLANG_AVAILABLE.load(Ordering::SeqCst) != 0 {
            Some(&rlang_expected)
        } else {
            None
        }
    });
    let Some(expected) = expected else {
        FAILURES.fetch_add(1, Ordering::SeqCst);
        return;
    };
    if ReplOutcome::from_native(command_id, expression_id, fact) != Some(*expected) {
        eprintln!(
            "unexpected repl outcome: command={command_id} expression={expression_id} fact={fact}; expected={expected:?}"
        );
        FAILURES.fetch_add(1, Ordering::SeqCst);
    }
    let marker_text = match fact {
        0 => "unobserved",
        1 => "completed",
        2 => "parse",
        3 => "eval",
        4 => "print",
        _ => "unknown",
    };
    marker(&format!(
        "ARF_OUTCOME:{command_id}:{expression_id}:{marker_text}"
    ));
}

fn marker(text: &str) {
    use std::io::Write;
    println!("{text}");
    let _ = std::io::stdout().flush();
}

#[test]
#[ignore = "runs an incremental command driver in the native R mainloop"]
fn c_repl_driver_recovers_failures_interrupts_gc_and_visibility() {
    const TEST_NAME: &str =
        "repl_tests::c_repl_driver_recovers_failures_interrupts_gc_and_visibility";
    const CHILD_ENV: &str = "ARF_C_REPL_DRIVER_CHILD";
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        unsafe {
            INPUT_INDEX.store(0, Ordering::SeqCst);
            OUTCOME_INDEX.store(0, Ordering::SeqCst);
            FAILURES.store(0, Ordering::SeqCst);
            NESTED_INPUT_PENDING.store(0, Ordering::SeqCst);
            arf_libr::initialize_r_with_args(&[
                "--vanilla",
                "--no-save",
                "--quiet",
                "--interactive",
            ])
            .expect("R should initialize in the dedicated mainloop child");
            let rlang_available = crate::eval_string("requireNamespace('rlang', quietly = TRUE)")
                .map(|result| {
                    let lib = arf_libr::r_library().expect("R library should stay available");
                    *(lib.logical)(result.sexp()) != 0
                })
                .expect("rlang availability probe should evaluate");
            RLANG_AVAILABLE.store(usize::from(rlang_available), Ordering::SeqCst);
            marker(if rlang_available {
                "ARF_RLANG_PRESENT"
            } else {
                "ARF_RLANG_ABSENT"
            });

            let lib = arf_libr::r_library().expect("R library should stay available");
            #[cfg(unix)]
            let previous_read_console = *lib.ptr_r_readconsole;
            let rejected = install_repl_driver(
                std::ptr::null_mut(),
                top_level_prompt,
                input_callback,
                outcome_callback,
                std::ptr::null_mut(),
            );
            assert!(
                matches!(
                    &rejected,
                    Err(arf_libr::RError::EvalError(message))
                        if message.contains("failed to install native REPL driver")
                ),
                "null parser factory should produce an explicit install error: {rejected:?}"
            );
            #[cfg(unix)]
            {
                let callback_unchanged = match (*lib.ptr_r_readconsole, previous_read_console) {
                    (Some(current), Some(previous)) => std::ptr::fn_addr_eq(current, previous),
                    (None, None) => true,
                    _ => false,
                };
                assert!(
                    callback_unchanged,
                    "failed installation must not replace ReadConsole"
                );
            }
            marker("ARF_NULL_INSTALL_REJECTED");

            let parser_factory =
                crate::initialize_repl_engine().expect("R parser factory should initialize");
            assert_eq!(crate::repl_engine_state(), crate::ReplEngineState::Ready);
            drop(parser_factory);
            let parser_factory = crate::eval_string_with_visibility(
                r#"
function(text) {
    connection <- base::textConnection(text, open = "r")
    list(
        function() base::parse(connection, n = 1L),
        function() {
            base::close(connection)
            if (grepl("uncaught eval", text, fixed = TRUE)) {
                cat("ARF_CLOSE_FAILURE_INJECTED\n")
                stop("injected close failure")
            }
            invisible(NULL)
        }
    )
}
"#,
            )
            .expect("test parser factory should evaluate")
            .value;
            crate::eval_string(
                ".arf_repl_print_abort <- structure(1L, class = 'arf_repl_print_abort'); print.arf_repl_print_abort <- function(x, ...) stop('uncaught print')",
            )
            .expect("print failure method should install");
            install_repl_driver(
                parser_factory.sexp(),
                top_level_prompt,
                input_callback,
                outcome_callback,
                std::ptr::null_mut(),
            )
            .expect("native C driver should install");
            arf_repl_driver_test_skip_next_boundary();
            marker("ARF_DRIVER_READY");
            arf_libr::run_r_mainloop();
            unreachable!("R exits its process after ReadConsole returns EOF");
        }
    }

    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            TEST_NAME,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env("R_DEFAULT_PACKAGES", "NULL")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mainloop child should start");
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let output = child
        .wait_with_output()
        .expect("mainloop child should exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mainloop child failed: {stderr}\n{stdout}"
    );
    let expected_markers = [
        "ARF_NULL_INSTALL_REJECTED",
        "ARF_DRIVER_READY",
        "ARF_OUTCOME:100:1:unobserved",
        "ARF_OUTCOME:101:2:parse",
        "ARF_CLOSE_FAILURE_INJECTED",
        "ARF_OUTCOME:102:1:eval",
        "ARF_OUTCOME:103:1:print",
        "ARF_OUTCOME:104:1:completed",
        "ARF_OUTCOME:105:2:completed",
        "ARF_NESTED_INPUT",
        "ARF_OUTCOME:106:2:completed",
        "ARF_OUTCOME:107:1:eval",
        "ARF_OUTCOME:108:3:completed",
        "ARF_OUTCOME:109:1:completed",
        "ARF_OUTCOME:110:1:completed",
        "ARF_OUTCOME:112:1:completed",
        "ARF_OUTCOME:114:1:completed",
        "ARF_OUTCOME:115:1:completed",
        "ARF_DRIVER_CANCELLED",
        "ARF_DRIVER_OK",
        "ARF_DRIVER_EOF",
    ];
    let mut expected_markers = expected_markers.to_vec();
    if stdout.contains("ARF_RLANG_PRESENT") {
        let eof_index = expected_markers
            .iter()
            .position(|marker| *marker == "ARF_DRIVER_OK")
            .expect("expected successful driver marker");
        expected_markers.insert(eof_index, "ARF_OUTCOME:111:1:eval");
    } else {
        assert!(
            stdout.contains("ARF_RLANG_ABSENT"),
            "child did not report rlang availability: {stdout}"
        );
    }
    let actual = stdout
        .split_whitespace()
        .filter(|token| {
            token.starts_with("ARF_DRIVER_")
                || token.starts_with("ARF_OUTCOME:")
                || token.starts_with("ARF_NESTED_")
                || token.starts_with("ARF_CLOSE_")
                || token.starts_with("ARF_NULL_INSTALL_")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected_markers,
        "child stdout: {stdout}\nchild stderr: {stderr}"
    );
    assert!(
        stdout.contains("NULL"),
        "visible NULL was not printed: {stdout}"
    );
    assert!(
        !stdout.contains("987654321"),
        "invisible result was printed: {stdout}"
    );
}
