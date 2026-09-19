//! Protected API probe for R-owned incremental parsing, evaluation, and printing.
//!
//! Parsing uses a persistent R-level `textConnection` and `parse(n = 1)`;
//! evaluation uses `R_tryEval(withVisible(expr), .GlobalEnv)`; autoprint uses
//! `Rf_PrintValue` inside `R_ToplevelExec`. This does not reproduce R's private
//! `Rf_ReplIteration`. In particular, it does not establish the B approach based
//! on native `run_Rmainloop`/`ReadConsole` integration and error-propagation cleanup.
//! The outcome reports only whether each protected API call returned normally;
//! it does not distinguish interrupts or restart reasons. It does not verify
//! factory-construction cleanup, nested native `R_tryEval`, task callbacks,
//! `.Last.value`, `options(error)`, browser, or recover behavior.

use arf_harp::eval_string;
use arf_libr::{ParseStatus, RLibrary, SEXP, SexpType, r_global_env, r_library};
use libloading::Library;
use std::error::Error;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};

type Lang2 = unsafe extern "C" fn(SEXP, SEXP) -> SEXP;

#[derive(Debug, PartialEq, Eq)]
enum CommandOutcome {
    Empty,
    Completed { parsed: usize, printed: usize },
    ParseAborted { expression: usize },
    EvaluationAborted { expression: usize },
    PrintAborted { expression: usize },
    BoundaryError { expression: usize },
    SetupError,
}

#[derive(Debug)]
struct CommandReport {
    outcome: CommandOutcome,
    connection_closed: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    // This binary is an isolated experiment and does not enter arf's REPL path.
    // The scenarios only need base R; skip optional packages missing in lean R installs.
    unsafe { std::env::set_var("R_DEFAULT_PACKAGES", "NULL") };
    unsafe { arf_libr::initialize_r_with_args(&["--vanilla", "--quiet"])? };

    let api = r_library()?;
    let global = r_global_env()?;
    let base = unsafe { *api.r_baseenv };
    let nil = arf_libr::r_nil_value()?;

    // Rf_lang2 is exported by R but is not part of arf-libr's current surface.
    // Keep its lookup local to this opt-in example instead of widening product APIs.
    let r_library_handle = unsafe { Library::new(arf_libr::find_r_library()?)? };
    let lang2 = unsafe { *r_library_handle.get::<Lang2>(b"Rf_lang2\0")? };

    let parser_factory = eval_string(
        r#"
.arf_spike_parser_factory <- function(text) {
    con <- base::textConnection(text, open = "r")
    list(
        next_expr = function() base::parse(con, n = 1L, keep.source = TRUE),
        close = function() base::close(con)
    )
}
"#,
    )?;
    let with_visible = lookup_in_environment(api, b"withVisible\0", base)?;

    let syntax = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        "1 + * 2",
    )?;
    assert_eq!(
        syntax.outcome,
        CommandOutcome::ParseAborted { expression: 1 }
    );
    assert!(syntax.connection_closed);

    let invisible = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        "invisible(42)",
    )?;
    assert_eq!(
        invisible.outcome,
        CommandOutcome::Completed {
            parsed: 1,
            printed: 0
        }
    );
    assert!(invisible.connection_closed);

    for source in ["", "# comment-only input"] {
        let empty = execute_command(
            api,
            lang2,
            parser_factory.sexp(),
            with_visible,
            global,
            nil,
            source,
        )?;
        assert_eq!(empty.outcome, CommandOutcome::Empty);
        assert!(empty.connection_closed);
    }

    let stop = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        r#"stop("spike stop"); .arf_spike_after_eval_abort <- 1L"#,
    )?;
    assert_eq!(
        stop.outcome,
        CommandOutcome::EvaluationAborted { expression: 1 }
    );
    assert!(
        stop.connection_closed,
        "the stop input connection must close"
    );
    assert!(!binding_exists(api, ".arf_spike_after_eval_abort", global)?);

    let after_stop = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        "40 + 2",
    )?;
    assert_eq!(
        after_stop.outcome,
        CommandOutcome::Completed {
            parsed: 1,
            printed: 1
        }
    );
    assert!(after_stop.connection_closed);

    let caught = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        r#"tryCatch(stop("caught"), error = function(cnd) 42)"#,
    )?;
    assert_eq!(
        caught.outcome,
        CommandOutcome::Completed {
            parsed: 1,
            printed: 1
        }
    );
    assert!(caught.connection_closed);

    let signaled = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        r#"signalCondition(errorCondition("nonfatal", class = "error")); 42"#,
    )?;
    assert_eq!(
        signaled.outcome,
        CommandOutcome::Completed {
            parsed: 2,
            printed: 2
        }
    );
    assert!(signaled.connection_closed);

    let direct_try_eval_caught_error = direct_try_eval_stop(api, global, nil)?;
    assert!(
        direct_try_eval_caught_error,
        "R_tryEval should return an error flag for an uncaught stop()"
    );

    let helper_setup = eval_string(
        r#"
.arf_spike_print_value <- structure(1L, class = "arf_spike_print_error")
print.arf_spike_print_error <- function(x, ...) stop("autoprint failed")
"#,
    )?;
    drop(helper_setup);
    let autoprint = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        ".arf_spike_print_value; .arf_spike_after_print_abort <- 1L",
    )?;
    assert_eq!(
        autoprint.outcome,
        CommandOutcome::PrintAborted { expression: 1 }
    );
    assert!(autoprint.connection_closed);
    assert!(!binding_exists(
        api,
        ".arf_spike_after_print_abort",
        global
    )?);
    let after_autoprint = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        "7 * 6",
    )?;
    assert_eq!(
        after_autoprint.outcome,
        CommandOutcome::Completed {
            parsed: 1,
            printed: 1
        }
    );
    assert!(after_autoprint.connection_closed);

    let sequential_name = ".arf_spike_sequential_side_effect";
    let sequential_source = format!("{sequential_name} <- 1L; 1 + * 2");
    let sequential = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        &sequential_source,
    )?;
    assert_eq!(
        sequential.outcome,
        CommandOutcome::ParseAborted { expression: 2 }
    );
    assert!(sequential.connection_closed);
    assert!(binding_is_integer_one(api, sequential_name, global)?);

    let full_parse_name = ".arf_spike_full_parse_side_effect";
    let full_parse_source = format!("{full_parse_name} <- 1L; 1 + * 2");
    let full_parse_status = parse_full_input(api, nil, &full_parse_source)?;
    assert_eq!(full_parse_status, ParseStatus::Error);
    assert!(!binding_exists(api, full_parse_name, global)?);

    let after_parse_error = execute_command(
        api,
        lang2,
        parser_factory.sexp(),
        with_visible,
        global,
        nil,
        "6 * 7",
    )?;
    assert_eq!(
        after_parse_error.outcome,
        CommandOutcome::Completed {
            parsed: 1,
            printed: 1
        }
    );
    assert!(after_parse_error.connection_closed);

    println!("protected API probe assertions passed");
    println!("R-level incremental parser and per-expression protected API outcomes checked");
    println!("full-input R_ParseVector negative baseline checked");
    Ok(())
}

fn execute_command(
    api: &RLibrary,
    lang2: Lang2,
    parser_factory: SEXP,
    with_visible: SEXP,
    global: SEXP,
    nil: SEXP,
    source: &str,
) -> Result<CommandReport, Box<dyn Error>> {
    let mut command_protect = arf_harp::RProtect::new();
    let source_value = make_r_string(api, source, &mut command_protect)?;
    let Some(factory_call) = make_call(
        api,
        lang2,
        parser_factory,
        source_value,
        true,
        nil,
        &mut command_protect,
    ) else {
        return Ok(CommandReport {
            outcome: CommandOutcome::SetupError,
            connection_closed: false,
        });
    };

    let mut setup_error = 0;
    let parser_parts = unsafe { (api.r_tryeval)(factory_call, global, &mut setup_error) };
    if setup_error != 0 || parser_parts.is_null() {
        return Ok(CommandReport {
            outcome: CommandOutcome::SetupError,
            connection_closed: false,
        });
    }
    let parser_parts = unsafe { command_protect.protect(parser_parts) };
    let next_expression = unsafe { (api.vector_elt)(parser_parts, 0) };
    let close_connection = unsafe { (api.vector_elt)(parser_parts, 1) };

    let mut parsed = 0;
    let mut printed = 0;

    let outcome = loop {
        let mut expression_protect = arf_harp::RProtect::new();
        let Some(next_call) = make_call(
            api,
            lang2,
            next_expression,
            nil,
            false,
            nil,
            &mut expression_protect,
        ) else {
            break CommandOutcome::BoundaryError {
                expression: parsed + 1,
            };
        };

        let mut parse_error = 0;
        let expression_vector = unsafe { (api.r_tryeval)(next_call, global, &mut parse_error) };
        if parse_error != 0 {
            break CommandOutcome::ParseAborted {
                expression: parsed + 1,
            };
        }
        let expression_vector = unsafe { expression_protect.protect(expression_vector) };
        let expression_count = unsafe { (api.rf_length)(expression_vector) };
        if expression_count == 0 {
            break if parsed == 0 {
                CommandOutcome::Empty
            } else {
                CommandOutcome::Completed { parsed, printed }
            };
        }

        parsed += 1;
        let expression = unsafe { (api.vector_elt)(expression_vector, 0) };
        let Some(visible_call) = make_call(
            api,
            lang2,
            with_visible,
            expression,
            true,
            nil,
            &mut expression_protect,
        ) else {
            break CommandOutcome::BoundaryError { expression: parsed };
        };

        let mut eval_error = 0;
        let visible_result = unsafe { (api.r_tryeval)(visible_call, global, &mut eval_error) };
        if eval_error != 0 {
            break CommandOutcome::EvaluationAborted { expression: parsed };
        }
        let visible_result = unsafe { expression_protect.protect(visible_result) };
        let value = unsafe { (api.vector_elt)(visible_result, 0) };
        let visible_flag = unsafe { (api.vector_elt)(visible_result, 1) };
        let visible = unsafe { *(api.logical)(visible_flag) != 0 };
        if visible {
            if print_value(api, value) {
                printed += 1;
            } else {
                break CommandOutcome::PrintAborted { expression: parsed };
            }
        }
    };

    let mut close_error = 0;
    let closed = if let Some(close_call) = make_call(
        api,
        lang2,
        close_connection,
        nil,
        false,
        nil,
        &mut command_protect,
    ) {
        unsafe { (api.r_tryeval)(close_call, global, &mut close_error) };
        close_error == 0
    } else {
        false
    };

    Ok(CommandReport {
        outcome,
        connection_closed: closed,
    })
}

fn make_r_string(
    api: &RLibrary,
    text: &str,
    protect: &mut arf_harp::RProtect,
) -> Result<SEXP, Box<dyn Error>> {
    let text = CString::new(text)?;
    let mut payload = StringPayload {
        text: text.as_ptr(),
        value: std::ptr::null_mut(),
        make_string: api.rf_mkstring,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(make_string_callback),
            (&mut payload as *mut StringPayload).cast(),
        )
    };
    if success == 0 || payload.value.is_null() {
        return Err("R failed while creating the input string".into());
    }
    Ok(unsafe { protect.protect(payload.value) })
}

fn make_call(
    api: &RLibrary,
    lang2: Lang2,
    function: SEXP,
    argument: SEXP,
    has_argument: bool,
    nil: SEXP,
    protect: &mut arf_harp::RProtect,
) -> Option<SEXP> {
    let mut payload = CallPayload {
        function,
        argument,
        has_argument: c_int::from(has_argument),
        nil,
        call: std::ptr::null_mut(),
        lang2,
        lcons: api.rf_lcons,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(make_call_callback),
            (&mut payload as *mut CallPayload).cast(),
        )
    };
    if success == 0 || payload.call.is_null() {
        return None;
    }
    Some(unsafe { protect.protect(payload.call) })
}

fn lookup_in_environment(
    api: &RLibrary,
    name: &'static [u8],
    environment: SEXP,
) -> Result<SEXP, Box<dyn Error>> {
    let mut payload = LookupPayload {
        name: name.as_ptr().cast(),
        environment,
        value: std::ptr::null_mut(),
        install: api.rf_install,
        find_var: api.rf_findvar,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(lookup_callback),
            (&mut payload as *mut LookupPayload).cast(),
        )
    };
    if success == 0 || payload.value.is_null() {
        return Err("R failed while looking up a function".into());
    }
    Ok(payload.value)
}

fn print_value(api: &RLibrary, value: SEXP) -> bool {
    let payload = PrintPayload {
        value,
        print_value: api.rf_printvalue,
    };
    unsafe {
        (api.r_toplevelexec)(
            Some(print_value_callback),
            (&payload as *const PrintPayload as *mut PrintPayload).cast(),
        ) != 0
    }
}

fn direct_try_eval_stop(api: &RLibrary, global: SEXP, nil: SEXP) -> Result<bool, Box<dyn Error>> {
    let mut protect = arf_harp::RProtect::new();
    let source = make_r_string(api, r#"stop("direct R_tryEval")"#, &mut protect)?;
    let mut payload = ParsePayload {
        code: source,
        nil,
        status: ParseStatus::Null,
        expressions: std::ptr::null_mut(),
        parse_vector: api.r_parsevector,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(parse_callback),
            (&mut payload as *mut ParsePayload).cast(),
        )
    };
    if success == 0 || payload.status != ParseStatus::Ok || payload.expressions.is_null() {
        return Err("R failed while parsing the direct R_tryEval probe".into());
    }
    let expressions = unsafe { protect.protect(payload.expressions) };
    let expression = unsafe { (api.vector_elt)(expressions, 0) };
    let mut error = 0;
    unsafe { (api.r_tryeval)(expression, global, &mut error) };
    Ok(error != 0)
}

fn parse_full_input(
    api: &RLibrary,
    nil: SEXP,
    source: &str,
) -> Result<ParseStatus, Box<dyn Error>> {
    let mut protect = arf_harp::RProtect::new();
    let code = make_r_string(api, source, &mut protect)?;
    let mut payload = ParsePayload {
        code,
        nil,
        status: ParseStatus::Null,
        expressions: std::ptr::null_mut(),
        parse_vector: api.r_parsevector,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(parse_callback),
            (&mut payload as *mut ParsePayload).cast(),
        )
    };
    if success == 0 {
        return Err("R failed while parsing the full-input baseline".into());
    }
    Ok(payload.status)
}

fn binding_exists(api: &RLibrary, name: &str, environment: SEXP) -> Result<bool, Box<dyn Error>> {
    Ok(lookup_binding(api, name, environment)? != unsafe { *api.r_unboundvalue })
}

fn binding_is_integer_one(
    api: &RLibrary,
    name: &str,
    environment: SEXP,
) -> Result<bool, Box<dyn Error>> {
    let value = lookup_binding(api, name, environment)?;
    if value == unsafe { *api.r_unboundvalue }
        || unsafe { (api.rf_typeof)(value) } != SexpType::IntSxp as c_int
        || unsafe { (api.rf_length)(value) } != 1
    {
        return Ok(false);
    }

    let integer = unsafe { (api.integer)(value) };
    Ok(!integer.is_null() && unsafe { *integer == 1 })
}

fn lookup_binding(api: &RLibrary, name: &str, environment: SEXP) -> Result<SEXP, Box<dyn Error>> {
    let name = CString::new(name)?;
    let mut payload = LookupPayload {
        name: name.as_ptr(),
        environment,
        value: std::ptr::null_mut(),
        install: api.rf_install,
        find_var: api.rf_findvar,
    };
    let success = unsafe {
        (api.r_toplevelexec)(
            Some(lookup_callback),
            (&mut payload as *mut LookupPayload).cast(),
        )
    };
    if success == 0 || payload.value.is_null() {
        return Err("R failed while checking a workspace binding".into());
    }
    Ok(payload.value)
}

#[repr(C)]
struct StringPayload {
    text: *const c_char,
    value: SEXP,
    make_string: unsafe extern "C" fn(*const c_char) -> SEXP,
}

#[repr(C)]
struct CallPayload {
    function: SEXP,
    argument: SEXP,
    has_argument: c_int,
    nil: SEXP,
    call: SEXP,
    lang2: Lang2,
    lcons: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
}

#[repr(C)]
struct LookupPayload {
    name: *const c_char,
    environment: SEXP,
    value: SEXP,
    install: unsafe extern "C" fn(*const c_char) -> SEXP,
    find_var: unsafe extern "C" fn(SEXP, SEXP) -> SEXP,
}

#[repr(C)]
struct PrintPayload {
    value: SEXP,
    print_value: unsafe extern "C" fn(SEXP),
}

#[repr(C)]
struct ParsePayload {
    code: SEXP,
    nil: SEXP,
    status: ParseStatus,
    expressions: SEXP,
    parse_vector: unsafe extern "C" fn(SEXP, c_int, *mut ParseStatus, SEXP) -> SEXP,
}

/// # Safety
/// The context must point to a live `StringPayload`.
unsafe extern "C" fn make_string_callback(context: *mut c_void) {
    let payload = unsafe { &mut *context.cast::<StringPayload>() };
    payload.value = unsafe { (payload.make_string)(payload.text) };
}

/// # Safety
/// The context must point to a live `CallPayload`.
unsafe extern "C" fn make_call_callback(context: *mut c_void) {
    let payload = unsafe { &mut *context.cast::<CallPayload>() };
    payload.call = unsafe {
        if payload.has_argument != 0 {
            (payload.lang2)(payload.function, payload.argument)
        } else {
            (payload.lcons)(payload.function, payload.nil)
        }
    };
}

/// # Safety
/// The context must point to a live `LookupPayload`.
unsafe extern "C" fn lookup_callback(context: *mut c_void) {
    let payload = unsafe { &mut *context.cast::<LookupPayload>() };
    let symbol = unsafe { (payload.install)(payload.name) };
    payload.value = unsafe { (payload.find_var)(symbol, payload.environment) };
}

/// # Safety
/// The context must point to a live `PrintPayload`.
unsafe extern "C" fn print_value_callback(context: *mut c_void) {
    let payload = unsafe { &*context.cast::<PrintPayload>() };
    unsafe { (payload.print_value)(payload.value) };
}

/// # Safety
/// The context must point to a live `ParsePayload`.
unsafe extern "C" fn parse_callback(context: *mut c_void) {
    let payload = unsafe { &mut *context.cast::<ParsePayload>() };
    payload.expressions =
        unsafe { (payload.parse_vector)(payload.code, -1, &mut payload.status, payload.nil) };
}
