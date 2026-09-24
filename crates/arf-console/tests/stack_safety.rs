use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn run_arf_eval(code: &str, timeout: Duration) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
    command
        .args(["--vanilla", "--quiet", "-e", code])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Bound CPU use in case the tested stack guard fails to stop recursion.
        unsafe {
            command.pre_exec(|| {
                let limit = libc::rlimit {
                    rlim_cur: 5,
                    rlim_max: 5,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = command.spawn().expect("arf child should start");
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .expect("arf child should remain waitable")
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("arf child exceeded the {timeout:?} timeout");
        }
        thread::sleep(Duration::from_millis(20));
    }
    child
        .wait_with_output()
        .expect("arf child output should be available")
}

fn finite_stack_info(stdout: &str) -> Result<(), String> {
    let marker = stdout
        .lines()
        .find_map(|line| line.split_once("ARF_CSTACK_INFO:"))
        .map(|(_, values)| values.trim())
        .ok_or_else(|| format!("child did not print Cstack_info marker: {stdout}"))?;
    let (size, current) = marker
        .split_once(':')
        .ok_or_else(|| format!("Cstack_info marker lacks fields: {stdout}"))?;
    let size = size.trim().parse::<u64>().map_err(|e| e.to_string())?;
    let current = current.trim().parse::<u64>().map_err(|e| e.to_string())?;
    if size == 0 || size == u64::MAX || current >= size {
        return Err(format!("invalid C stack bounds: {stdout}"));
    }
    Ok(())
}

fn probe_finite_stack_limit() -> Result<(), String> {
    let output = run_arf_eval(
        "info <- Cstack_info(); cat('ARF_CSTACK_INFO:', info[['size']], ':', info[['current']], '\\n')",
        Duration::from_secs(10),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("arf child failed: {stderr}\n{stdout}"));
    }
    finite_stack_info(&stdout)
}

/// Verify the product executable leaves R's detected C stack limit enabled.
/// This deliberately inspects the limit before any recursive R evaluation.
#[test]
fn product_preserves_finite_r_c_stack_limit() {
    probe_finite_stack_limit().unwrap_or_else(|error| panic!("{error}"));
}

/// Verify an R stack condition is distinct from process failure and that R
/// can continue after it. The finite-limit probe runs first; this child has a
/// wall-clock timeout and a Unix CPU limit as fail-safes.
#[test]
fn product_recovers_after_r_c_stack_condition() {
    if let Err(error) = probe_finite_stack_limit() {
        eprintln!("Skipping unsafe recursion because stack probe failed: {error}");
        return;
    }

    let output = run_arf_eval(
        concat!(
            "stack_info <- Cstack_info(); ",
            "stopifnot(is.finite(stack_info[['size']]), stack_info[['size']] > 0, ",
            "stack_info[['current']] < stack_info[['size']]); ",
            "options(expressions = 50000L); ",
            "compiler::enableJIT(0L); ",
            "recurse <- function() recurse(); ",
            "tryCatch(recurse(), error = function(e) { ",
            "cat('ARF_R_STACK_CLASS:', paste(class(e), collapse = '/'), '\\n'); ",
            "cat('ARF_R_STACK_CONDITION:', conditionMessage(e), '\\n') ",
            "}); ",
            "cat('ARF_R_STACK_RECOVERED:', 1 + 1, '\\n')"
        ),
        Duration::from_secs(10),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "arf child failed: {stderr}\n{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("ARF_R_STACK_CLASS:")
                && line.contains("CStackOverflowError")),
        "recursion should raise CStackOverflowError: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("ARF_R_STACK_CONDITION:") && stdout.contains("C stack usage"),
        "recursion should raise R's stack condition: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("ARF_R_STACK_RECOVERED: 2"),
        "R should recover after the stack condition: {stdout}\n{stderr}"
    );
}
