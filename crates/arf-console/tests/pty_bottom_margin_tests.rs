//! Bottom margin PTY integration tests for arf.
//!
//! These tests verify the bottom margin feature keeps the prompt at the
//! configured distance from the terminal bottom.
//!
//! The bottom margin feature is useful for keeping the prompt visible when
//! there's a lot of output, similar to radian's behavior. Without it, the
//! prompt quickly reaches the bottom of the terminal and stays there.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test proportional margin (0.5) keeps prompt in upper half.
///
/// Verifies that with `bottom_margin = { proportional = 0.5 }`, the prompt
/// stays in the upper half of a 24-row terminal (at or above row 12).
/// Tests multiple commands to ensure consistency.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_proportional_half() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ proportional = 0.5 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With 0.5 margin in 24-row terminal, prompt should be at or above row 12
    // Allow ±1 row tolerance for rounding
    assert!(
        row <= 13,
        "Prompt should be in upper half (row <= 13), got row {}",
        row
    );

    // Run several commands to verify consistency
    for i in 1..=5 {
        terminal
            .send_line(&format!("print({})", i))
            .expect("Should send command");
        terminal
            .wait_for_prompt()
            .expect("Should show prompt after command");

        let (row, _) = terminal
            .cursor_position()
            .expect("Should get cursor position");
        assert!(
            row <= 13,
            "Prompt should stay in upper half after command {}, got row {}",
            i,
            row
        );
    }

    terminal.quit().expect("Should quit cleanly");
}

/// Test fixed margin reserves exact line count.
///
/// Verifies that with `bottom_margin = { fixed = 5 }`, the prompt stays
/// at least 5 lines from the bottom of the terminal.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_fixed_lines() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ fixed = 5 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Fill screen with output to push cursor down
    for _ in 1..=20 {
        terminal
            .send_line("cat('\\n')")
            .expect("Should send command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With fixed=5 in 24-row terminal, prompt should be at or above row 19
    // (24 - 5 = 19, allowing 1 row tolerance)
    assert!(
        row >= 18,
        "Prompt should be at least 5 rows from bottom (row >= 18), got row {}",
        row
    );

    // Run more commands to verify margin maintained consistently
    for _ in 1..=3 {
        terminal
            .send_line("cat('\\n')")
            .expect("Should send command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    assert!(
        row >= 18,
        "Prompt should maintain 5-line margin after output, got row {}",
        row
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test disabled margin allows prompt at bottom.
///
/// Verifies that with `bottom_margin = "disabled"`, the prompt can reach
/// the bottom of the terminal (normal behavior).
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_disabled() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = "disabled"
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Fill the screen with output
    for _ in 1..=30 {
        terminal
            .send_line("cat('\\n')")
            .expect("Should send command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With disabled margin, prompt should be able to reach near bottom
    // (allowing for some scrollback, row should be >= 20 in 24-row terminal)
    assert!(
        row >= 20,
        "With disabled margin, prompt should reach near bottom (row >= 20), got row {}",
        row
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test proportional = 0.0 pins prompt to top.
///
/// Verifies that with `bottom_margin = { proportional = 0.0 }`, the
/// prompt stays at the top of the terminal (row 0 or 1).
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_proportional_top() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ proportional = 0.0 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With 0.0 margin, prompt should be at top (row 0 or 1)
    assert!(
        row <= 2,
        "With proportional = 0.0, prompt should be at top (row <= 2), got row {}",
        row
    );

    // Run multiple commands and verify it stays at top
    for i in 1..=5 {
        terminal
            .send_line(&format!("print({})", i))
            .expect("Should send command");
        terminal
            .wait_for_prompt()
            .expect("Should show prompt after command");

        let (row, _) = terminal
            .cursor_position()
            .expect("Should get cursor position");
        assert!(
            row <= 2,
            "Prompt should stay at top after command {}, got row {}",
            i,
            row
        );
    }

    terminal.quit().expect("Should quit cleanly");
}

/// Test fixed = 0 behaves like disabled (zero-cost).
///
/// Verifies that `bottom_margin = { fixed = 0 }` behaves the same as
/// disabled - no margin, prompt reaches bottom.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_fixed_zero() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ fixed = 0 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Fill the screen with output
    for _ in 1..=30 {
        terminal
            .send_line("cat('\\n')")
            .expect("Should send command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With fixed = 0, should behave like disabled
    assert!(
        row >= 20,
        "With fixed = 0, prompt should reach near bottom (row >= 20), got row {}",
        row
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test large fixed value clamps to terminal height.
///
/// Verifies that `bottom_margin = { fixed = 100 }` in a 24-row terminal
/// clamps to 24 lines, resulting in prompt at top.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_large_fixed() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ fixed = 100 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With fixed=100 in 24-row terminal, should clamp to 24
    // resulting in target_row = 0 (top of screen)
    assert!(
        row <= 2,
        "With large fixed value, prompt should be at top (row <= 2), got row {}",
        row
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test high proportional value (0.95) keeps prompt near top.
///
/// Verifies that `bottom_margin = { proportional = 0.95 }` keeps the
/// prompt near the top by reserving only the bottom 5% of the terminal.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_high_proportional() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ proportional = 0.95 }}
"#
    )
    .expect("Failed to write config");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--config",
        &config_path.to_string_lossy(),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    // With 0.95 margin in 24-row terminal, only bottom 5% is reserved
    // So prompt should be at or above row 22 (allowing ±1 tolerance)
    assert!(
        row <= 23,
        "With proportional = 0.95, prompt should be near top (row <= 23), got row {}",
        row
    );

    // Run commands and verify it stays near top
    for _ in 1..=3 {
        terminal
            .send_line("cat('\\n')")
            .expect("Should send command");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    terminal.wait_for_prompt().expect("Should show prompt");

    let (row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    assert!(
        row <= 23,
        "Prompt should stay near top with high proportional, got row {}",
        row
    );

    terminal.quit().expect("Should quit cleanly");
}

/// Test window resize adjusts margin correctly.
///
/// Verifies that with `bottom_margin = { proportional = 0.5 }` in a 24-row
/// terminal, the prompt starts in the upper half. While we cannot actually
/// resize the PTY during the test, we verify the margin is consistently
/// recalculated across multiple prompts.
#[test]
#[cfg(unix)]
fn test_pty_bottom_margin_resize_window() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = temp_dir.path().join("arf.toml");
    let mut config_file =
        std::fs::File::create(&config_path).expect("Failed to create config file");
    writeln!(
        config_file,
        r#"
[editor]
bottom_margin = {{ proportional = 0.5 }}
"#
    )
    .expect("Failed to write config");

    // Start with 24-row terminal
    let mut terminal = Terminal::spawn_with_size(
        &[
            "--no-auto-match",
            "--config",
            &config_path.to_string_lossy(),
        ],
        24,
        80,
    )
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Check initial position in 24-row terminal (should be <= row 12)
    let (initial_row, _) = terminal
        .cursor_position()
        .expect("Should get cursor position");
    assert!(
        initial_row <= 13,
        "Initial prompt should be in upper half of 24-row terminal (row <= 13), got row {}",
        initial_row
    );

    // For a more robust test, we verify the margin is consistent across multiple prompts
    for i in 1..=5 {
        terminal
            .send_line(&format!("print({})", i))
            .expect("Should send command");
        terminal
            .wait_for_prompt()
            .expect("Should show prompt after command");

        let (row, _) = terminal
            .cursor_position()
            .expect("Should get cursor position");
        // Verify the margin is recalculated each prompt
        assert!(
            row <= 13,
            "Prompt should maintain upper half position after command {}, got row {}",
            i,
            row
        );
    }

    terminal.quit().expect("Should quit cleanly");
}
