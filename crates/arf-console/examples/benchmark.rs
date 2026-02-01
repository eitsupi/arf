//! Benchmark script comparing arf, vanilla R, and radian performance.
//!
//! Usage:
//!     cargo build --release
//!     uv tool install radian
//!     cargo run --release --example benchmark

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Result of a benchmark run
struct BenchmarkResult {
    times: Vec<f64>,
    avg: f64,
    min: f64,
    max: f64,
}

impl BenchmarkResult {
    fn new(times: Vec<f64>) -> Self {
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        let min = times.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = times.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Self {
            times,
            avg,
            min,
            max,
        }
    }
}

/// Measure startup time for a command
fn measure_startup(cmd: &str, args: &[&str], input: &str, runs: usize) -> Option<BenchmarkResult> {
    let mut times = Vec::with_capacity(runs);

    for _ in 0..runs {
        let start = Instant::now();
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }

        let _ = child.wait();
        let elapsed = start.elapsed().as_secs_f64();
        times.push(elapsed);
    }

    Some(BenchmarkResult::new(times))
}

/// Print benchmark result
fn print_result(name: &str, result: &BenchmarkResult) {
    let times_str: Vec<String> = result.times.iter().map(|t| format!("{:.3}s", t)).collect();
    println!("{}:", name);
    println!("  Runs: [{}]", times_str.join(", "));
    println!("  Average: {:.3}s", result.avg);
    println!("  Min: {:.3}s, Max: {:.3}s", result.min, result.max);
}

/// Measure startup time for all tools
fn measure_startup_all(
    runs: usize,
) -> (
    Option<BenchmarkResult>,
    Option<BenchmarkResult>,
    Option<BenchmarkResult>,
) {
    println!("=== Startup Time Benchmark ===\n");

    let arf_result = measure_startup("./target/release/arf", &[], "q()\n", runs);
    if let Some(ref r) = arf_result {
        print_result("arf", r);
    } else {
        println!("arf: not found (run `cargo build --release` first)");
    }
    println!();

    let r_result = measure_startup("R", &["--vanilla", "-q"], "q()\n", runs);
    if let Some(ref r) = r_result {
        print_result("R (vanilla)", r);
    } else {
        println!("R (vanilla): not found");
    }
    println!();

    let radian_result = measure_startup("radian", &[], "q()\n", runs);
    if let Some(ref r) = radian_result {
        print_result("radian", r);
    } else {
        println!("radian: not found (run `uv tool install radian` to install)");
    }
    println!();

    // Print comparison
    println!("=== Comparison ===");
    let avgs: Vec<f64> = [&arf_result, &r_result, &radian_result]
        .iter()
        .filter_map(|r| r.as_ref().map(|x| x.avg))
        .collect();

    if let Some(fastest) = avgs.iter().cloned().reduce(f64::min) {
        if let Some(ref r) = arf_result {
            println!("arf:     {:.3}s ({:.1}x)", r.avg, r.avg / fastest);
        }
        if let Some(ref r) = r_result {
            println!("R:       {:.3}s ({:.1}x)", r.avg, r.avg / fastest);
        }
        if let Some(ref r) = radian_result {
            println!("radian:  {:.3}s ({:.1}x)", r.avg, r.avg / fastest);
        }
    }
    println!();

    (arf_result, r_result, radian_result)
}

/// Get RSS memory of process tree in MB using ps command
fn get_memory_tree(pid: u32) -> Option<f64> {
    let output = Command::new("ps")
        .args([
            "--no-headers",
            "-o",
            "rss",
            "--ppid",
            &pid.to_string(),
            "-p",
            &pid.to_string(),
        ])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let total_kb: u64 = stdout
        .split_whitespace()
        .filter_map(|s| s.parse::<u64>().ok())
        .sum();

    Some(total_kb as f64 / 1024.0)
}

/// Memory measurement result
struct MemoryResult {
    memory_mb: f64,
}

/// Measure memory for arf
fn measure_memory_arf() -> Option<MemoryResult> {
    let mut child = Command::new("./target/release/arf")
        .args(["-e", "Sys.sleep(5)"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    thread::sleep(Duration::from_secs(3));
    let memory_mb = get_memory_tree(child.id())?;
    let _ = child.wait();

    Some(MemoryResult { memory_mb })
}

/// Measure memory for R
fn measure_memory_r() -> Option<MemoryResult> {
    let mut child = Command::new("R")
        .args(["--vanilla", "-q", "-e", "Sys.sleep(5)"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    thread::sleep(Duration::from_secs(3));
    let memory_mb = get_memory_tree(child.id())?;
    let _ = child.wait();

    Some(MemoryResult { memory_mb })
}

/// Measure memory for radian (requires PTY)
fn measure_memory_radian() -> Option<MemoryResult> {
    // radian requires a PTY, so we use a simple approach with script command
    let mut child = Command::new("script")
        .args(["-q", "-c", "radian", "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    thread::sleep(Duration::from_secs(3));
    let memory_mb = get_memory_tree(child.id())?;

    // Send quit command
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"q()\n");
    }
    thread::sleep(Duration::from_millis(500));
    let _ = child.kill();

    Some(MemoryResult { memory_mb })
}

/// Measure memory usage for all tools
fn measure_memory_all() -> (
    Option<MemoryResult>,
    Option<MemoryResult>,
    Option<MemoryResult>,
) {
    println!("=== Memory Usage Benchmark ===\n");

    let arf_result = measure_memory_arf();
    if let Some(ref r) = arf_result {
        println!("arf:");
        println!("  Total RSS (process tree): {:.1} MB", r.memory_mb);
    } else {
        println!("arf: not found");
    }
    println!();

    let r_result = measure_memory_r();
    if let Some(ref r) = r_result {
        println!("R (vanilla):");
        println!("  Total RSS (process tree): {:.1} MB", r.memory_mb);
    } else {
        println!("R (vanilla): not found");
    }
    println!();

    let radian_result = measure_memory_radian();
    if let Some(ref r) = radian_result {
        println!("radian:");
        println!("  Total RSS (process tree): {:.1} MB", r.memory_mb);
    } else {
        println!("radian: not found");
    }
    println!();

    // Print comparison
    println!("=== Comparison ===");
    if let Some(ref arf) = arf_result {
        println!("arf:     {:.1} MB", arf.memory_mb);
    }
    if let Some(ref r) = r_result {
        println!("R:       {:.1} MB", r.memory_mb);
    }
    if let Some(ref radian) = radian_result {
        println!("radian:  {:.1} MB", radian.memory_mb);
    }

    if let (Some(arf), Some(radian)) = (&arf_result, &radian_result)
        && arf.memory_mb < radian.memory_mb
    {
        let diff = radian.memory_mb - arf.memory_mb;
        println!(
            "arf uses {:.1} MB ({:.0}%) less memory than radian",
            diff,
            diff / radian.memory_mb * 100.0
        );
    }
    println!();

    (arf_result, r_result, radian_result)
}

/// R commands to benchmark
const R_COMMANDS: &[(&str, &str)] = &[
    ("1+1", "Simple arithmetic"),
    ("sum(1:1000000)", "Sum 1M numbers"),
    (
        "x <- rnorm(100000); mean(x)",
        "Generate & mean 100K randoms",
    ),
];

/// Command execution result for multiple R commands
struct CommandResults {
    results: Vec<(String, f64)>, // (description, avg time)
}

/// Measure command execution time for arf
fn measure_commands_arf(runs: usize) -> Option<CommandResults> {
    let mut results = Vec::new();

    for (r_cmd, desc) in R_COMMANDS {
        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = Instant::now();
            let status = Command::new("./target/release/arf")
                .args(["-e", r_cmd])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status();

            if status.is_err() {
                return None;
            }
            times.push(start.elapsed().as_secs_f64());
        }
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        results.push((desc.to_string(), avg));
    }

    Some(CommandResults { results })
}

/// Measure command execution time for R
fn measure_commands_r(runs: usize) -> Option<CommandResults> {
    let mut results = Vec::new();

    for (r_cmd, desc) in R_COMMANDS {
        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = Instant::now();
            let status = Command::new("R")
                .args(["--vanilla", "-q", "-e", r_cmd])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status();

            if status.is_err() {
                return None;
            }
            times.push(start.elapsed().as_secs_f64());
        }
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        results.push((desc.to_string(), avg));
    }

    Some(CommandResults { results })
}

/// Measure command execution time for radian
fn measure_commands_radian(runs: usize) -> Option<CommandResults> {
    let mut results = Vec::new();

    for (r_cmd, desc) in R_COMMANDS {
        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let start = Instant::now();
            let mut child = Command::new("radian")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .ok()?;

            if let Some(mut stdin) = child.stdin.take() {
                let input = format!("{}\nq()\n", r_cmd);
                let _ = stdin.write_all(input.as_bytes());
            }

            let _ = child.wait();
            times.push(start.elapsed().as_secs_f64());
        }
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        results.push((desc.to_string(), avg));
    }

    Some(CommandResults { results })
}

/// Measure command execution for all tools
fn measure_commands_all(
    runs: usize,
) -> (
    Option<CommandResults>,
    Option<CommandResults>,
    Option<CommandResults>,
) {
    println!("=== R Command Execution Benchmark ===\n");
    println!("(Includes startup time + command execution)\n");

    let arf_result = measure_commands_arf(runs);
    if let Some(ref r) = arf_result {
        println!("arf:");
        for (desc, avg) in &r.results {
            println!("  {}: {:.3}s (avg of {} runs)", desc, avg, runs);
        }
    } else {
        println!("arf: not found");
    }
    println!();

    let r_result = measure_commands_r(runs);
    if let Some(ref r) = r_result {
        println!("R (vanilla):");
        for (desc, avg) in &r.results {
            println!("  {}: {:.3}s (avg of {} runs)", desc, avg, runs);
        }
    } else {
        println!("R (vanilla): not found");
    }
    println!();

    let radian_result = measure_commands_radian(runs);
    if let Some(ref r) = radian_result {
        println!("radian:");
        for (desc, avg) in &r.results {
            println!("  {}: {:.3}s (avg of {} runs)", desc, avg, runs);
        }
    } else {
        println!("radian: not found");
    }
    println!();

    (arf_result, r_result, radian_result)
}

fn main() {
    println!("{}", "=".repeat(60));
    println!("arf vs R vs radian Performance Benchmark");
    println!("{}", "=".repeat(60));
    println!();

    let (arf_startup, r_startup, radian_startup) = measure_startup_all(5);
    let (arf_memory, r_memory, radian_memory) = measure_memory_all();
    let (arf_commands, r_commands, radian_commands) = measure_commands_all(3);

    // Print summary table
    println!("{}", "=".repeat(60));
    println!("SUMMARY");
    println!("{}", "=".repeat(60));
    println!();
    println!("| Metric | arf | R | radian |");
    println!("|--------|-----|---|--------|");

    let arf_startup_str = arf_startup
        .as_ref()
        .map(|r| format!("{:.3}s", r.avg))
        .unwrap_or_else(|| "N/A".to_string());
    let r_startup_str = r_startup
        .as_ref()
        .map(|r| format!("{:.3}s", r.avg))
        .unwrap_or_else(|| "N/A".to_string());
    let radian_startup_str = radian_startup
        .as_ref()
        .map(|r| format!("{:.3}s", r.avg))
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "| Startup time | {} | {} | {} |",
        arf_startup_str, r_startup_str, radian_startup_str
    );

    let arf_memory_str = arf_memory
        .as_ref()
        .map(|r| format!("{:.1} MB", r.memory_mb))
        .unwrap_or_else(|| "N/A".to_string());
    let r_memory_str = r_memory
        .as_ref()
        .map(|r| format!("{:.1} MB", r.memory_mb))
        .unwrap_or_else(|| "N/A".to_string());
    let radian_memory_str = radian_memory
        .as_ref()
        .map(|r| format!("{:.1} MB", r.memory_mb))
        .unwrap_or_else(|| "N/A".to_string());
    println!(
        "| Memory (RSS) | {} | {} | {} |",
        arf_memory_str, r_memory_str, radian_memory_str
    );

    // Print command execution results
    for (i, (_, desc)) in R_COMMANDS.iter().enumerate() {
        let short_desc: String = desc.split_whitespace().next().unwrap_or(desc).to_string();
        let arf_cmd_str = arf_commands
            .as_ref()
            .and_then(|c| c.results.get(i))
            .map(|(_, avg)| format!("{:.3}s", avg))
            .unwrap_or_else(|| "N/A".to_string());
        let r_cmd_str = r_commands
            .as_ref()
            .and_then(|c| c.results.get(i))
            .map(|(_, avg)| format!("{:.3}s", avg))
            .unwrap_or_else(|| "N/A".to_string());
        let radian_cmd_str = radian_commands
            .as_ref()
            .and_then(|c| c.results.get(i))
            .map(|(_, avg)| format!("{:.3}s", avg))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "| {} | {} | {} | {} |",
            short_desc, arf_cmd_str, r_cmd_str, radian_cmd_str
        );
    }
}
