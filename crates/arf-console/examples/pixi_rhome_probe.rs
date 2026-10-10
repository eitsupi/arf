//! Standalone bounded probe prototype for the unmerged embedding experiment.
//! This is not wired into the production provider or discovery chain.

use anyhow::{Context, Result, bail};
use serde_json::json;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const OUTPUT_LIMIT: usize = 64 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > OUTPUT_LIMIT {
            bail!("output_limit");
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let milliseconds = args.next().context("timeout milliseconds or 'default'")?;
    let timeout = if milliseconds == "default" {
        PROBE_TIMEOUT
    } else {
        Duration::from_millis(milliseconds.to_string_lossy().parse()?)
    };
    let executable = args.next().context("R executable")?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_remove("R_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let start = Instant::now();
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().context("stdout pipe")?;
    let stderr = child.stderr.take().context("stderr pipe")?;
    // One deadline covers process completion AND both pipes reaching EOF.
    // Cancellation drops the pipes rather than joining blocking reader threads.
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            child.wait().await.map_err(anyhow::Error::from)
        })
    })
    .await;
    let output = match result {
        Ok(Ok((stdout, stderr, status))) => {
            let home = arf_libr::r_home_from_rhome_output(&String::from_utf8_lossy(&stdout));
            json!({
                "status": if status.success() { "ok" } else { "process_failure" },
                "r_home": home,
                "stdout_bytes": stdout.len(), "stderr_bytes": stderr.len()
            })
        }
        rejection => {
            let reason = match rejection {
                Err(_) => "timeout".to_owned(),
                Ok(Err(error)) => error.to_string(),
                Ok(Ok(_)) => unreachable!(),
            };
            let _ = child.start_kill();
            let cleanup_finished = tokio::time::timeout(CLEANUP_TIMEOUT, child.wait())
                .await
                .is_ok();
            json!({"status": reason, "cleanup_finished": cleanup_finished})
        }
    };
    let mut output = output;
    output["elapsed_ms"] = json!(start.elapsed().as_millis());
    println!("{output}");
    Ok(())
}
