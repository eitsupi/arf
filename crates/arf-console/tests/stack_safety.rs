use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_TIMEOUT: Duration = Duration::from_secs(45);

#[test]
fn product_process_has_finite_r_stack_and_recovers_from_overflow() {
    let r_code = r#"
stack_info <- Cstack_info()
stack_size <- stack_info[["size"]]
stack_current <- stack_info[["current"]]
stopifnot(is.finite(stack_size), stack_size > 0)
stopifnot(is.finite(stack_current), stack_current > 0, stack_current < stack_size)
cat("finite-stack:", stack_size, "current:", stack_current, "\n")
options(expressions = 50000L)
compiler::enableJIT(0L)
recurse <- function() recurse()
overflow_message <- tryCatch(recurse(), CStackOverflowError = function(e) conditionMessage(e))
stopifnot(is.character(overflow_message), length(overflow_message) == 1L)
stopifnot(grepl("C stack usage", overflow_message, fixed = TRUE))
cat("overflow-caught-and-continued\n")
"#;

    let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
    command
        .args(["--quiet", "--vanilla", "--eval", r_code])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        const STACK_LIMIT_CAP: libc::rlim_t = 8 * 1024 * 1024;

        // Read the inherited limit before spawning so the product process gets
        // a predictable native stack even when the host limit is unlimited.
        let mut inherited_stack_limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let get_stack_limit_result =
            unsafe { libc::getrlimit(libc::RLIMIT_STACK, &mut inherited_stack_limit) };
        assert_eq!(
            get_stack_limit_result,
            0,
            "read parent RLIMIT_STACK for product R process: {}",
            std::io::Error::last_os_error()
        );

        let capped_stack_soft_limit =
            (inherited_stack_limit.rlim_cur > STACK_LIMIT_CAP).then(|| {
                // Preserve the hard limit and never request a soft limit above it.
                std::cmp::min(STACK_LIMIT_CAP, inherited_stack_limit.rlim_max)
            });

        // Bound runaway recursion and startup failures even if the test's
        // wall-clock watchdog cannot run because the child consumes a core.
        unsafe {
            command.pre_exec(move || {
                if let Some(soft_limit) = capped_stack_soft_limit {
                    let stack_limit = libc::rlimit {
                        rlim_cur: soft_limit,
                        rlim_max: inherited_stack_limit.rlim_max,
                    };
                    if libc::setrlimit(libc::RLIMIT_STACK, &stack_limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }

                let cpu_limit = libc::rlimit {
                    rlim_cur: 15,
                    rlim_max: 15,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }

    let mut child = command.spawn().expect("start product R process");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("poll product R process").is_some() {
            break;
        }
        if started.elapsed() >= CHILD_TIMEOUT {
            child.kill().expect("kill timed-out product R process");
            let output = child
                .wait_with_output()
                .expect("collect timed-out product R process output");
            panic!(
                "product R process exceeded {CHILD_TIMEOUT:?}; stdout={}, stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = child
        .wait_with_output()
        .expect("collect product R process output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "product R process failed: stdout={stdout}, stderr={stderr}"
    );
    assert!(
        stdout.contains("finite-stack:"),
        "finite C stack size was not reported: stdout={stdout}, stderr={stderr}"
    );
    assert!(
        stdout.contains("overflow-caught-and-continued"),
        "R did not continue after catching CStackOverflowError: stdout={stdout}, stderr={stderr}"
    );
}
