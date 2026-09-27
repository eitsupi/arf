use super::*;
use crate::config::StatusSymbol;
use reedline::Prompt;

fn create_test_config(mode: ReprexMode) -> PromptRuntimeConfig {
    create_test_config_with_indicators(mode, Indicators::default())
}

fn create_test_config_with_indicators(
    _mode: ReprexMode,
    indicators: Indicators,
) -> PromptRuntimeConfig {
    PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "[bash] $ ")
        .indicators(indicators)
        .build()
}

#[test]
fn test_prompt_runtime_config_build_main_prompt() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let config = create_test_config(ReprexMode::Off);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_prompt_runtime_config_reprex_mode_indicator() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let config = create_test_config(ReprexMode::On);
    let prompt = config.build_main_prompt(ReprexMode::On);
    assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
}

#[test]
fn test_prompt_runtime_config_set_reprex_mode() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut runtime =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);

    assert!(!runtime.is_enabled());
    runtime.set_mode(ReprexMode::On);
    assert!(runtime.is_enabled());
    runtime.set_mode(ReprexMode::Off);
    assert!(!runtime.is_enabled());
}

#[test]
fn test_prompt_runtime_config_set_reprex_with_comment() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut runtime =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);

    runtime.set_mode(ReprexMode::On);
    assert!(runtime.is_enabled());
}

#[test]
fn test_prompt_runtime_config_shell_mode_prompt() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_config(ReprexMode::Off);
    let mut runtime =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);

    // Initially R mode prompt
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    // Enable shell mode - uses shell_format as prompt
    config.set_shell(true);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[bash] $ ");

    // Shell mode prompt ignores reprex mode
    runtime.set_mode(ReprexMode::On);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[bash] $ ");

    // Disable shell mode, reprex shows
    config.set_shell(false);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
}

#[test]
fn test_prompt_runtime_config_format_mode_indicator() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let config = create_test_config(ReprexMode::Off);
    let mut runtime =
        ReprexRuntime::new(ReprexMode::Off, "#> ", crate::config::FormatterBackend::Air);

    // Initially no indicator
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    // Enable reprex mode - shows reprex indicator
    runtime.set_mode(ReprexMode::On);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");

    // Enable format mode - now shows the format indicator instead
    runtime.set_mode(ReprexMode::Format);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[format] r> ");

    // Disable reprex - no mode indicator.
    runtime.set_mode(ReprexMode::Off);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    // Re-enable format mode - the format indicator shows again.
    runtime.set_mode(ReprexMode::Format);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[format] r> ");

    // Switch back to reprex mode
    runtime.set_mode(ReprexMode::On);
    let prompt = config.build_main_prompt(runtime.mode);
    assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
}

#[test]
fn test_prompt_runtime_config_custom_format_indicator() {
    // create_test_config_with_indicators reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let indicators = Indicators {
        reprex_format: "[AIR] ".to_string(),
        ..Indicators::default()
    };

    let config = create_test_config_with_indicators(ReprexMode::Format, indicators);

    // Shows the custom format indicator.
    let prompt = config.build_main_prompt(ReprexMode::Format);
    assert_eq!(prompt.render_prompt_left(), "[AIR] r> ");
}

#[test]
fn test_prompt_runtime_config_cwd_placeholder_expansion() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    // Test that {cwd} and {cwd_short} placeholders are expanded dynamically
    let config = PromptRuntimeConfig::builder(
        PromptFormatter::default(),
        "{cwd_short}> ",
        "+  ",
        "[{shell}] $ ",
    )
    .build();

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left().to_string();

    // The cwd_short should be expanded to the current directory's basename
    // It should NOT contain the literal "{cwd_short}" placeholder
    assert!(
        !rendered.contains("{cwd_short}"),
        "Placeholder should be expanded, got: {}",
        rendered
    );
    assert!(
        rendered.ends_with("> "),
        "Prompt should end with '> ', got: {}",
        rendered
    );
}

#[test]
fn test_prompt_runtime_config_dynamic_cwd_update() {
    // PromptFormatter::new reads SHELL; the test also changes the cwd.
    let _guard = crate::test_utils::lock_env_and_cwd();

    // Test that build_main_prompt() returns updated cwd after directory change
    let config = PromptRuntimeConfig::builder(PromptFormatter::default(), "{cwd}> ", "+  ", "$ ")
        .mode_indicator_position(ModeIndicatorPosition::None)
        .build();

    // Get the current directory
    let original_cwd = std::env::current_dir().unwrap();
    let prompt1 = config.build_main_prompt(ReprexMode::Off);
    let rendered1 = prompt1.render_prompt_left().to_string();

    // Change to a temporary directory
    let temp_dir = std::env::temp_dir();
    std::env::set_current_dir(&temp_dir).unwrap();

    // Build prompt again - should reflect the new directory
    let prompt2 = config.build_main_prompt(ReprexMode::Off);
    let rendered2 = prompt2.render_prompt_left().to_string();

    // The two prompts should be different if cwd changed
    // (unless original_cwd == temp_dir, which is unlikely)
    if original_cwd != temp_dir {
        assert_ne!(
            rendered1, rendered2,
            "Prompt should update when cwd changes.\nBefore: {}\nAfter: {}",
            rendered1, rendered2
        );
    }

    // Verify the prompt contains the temp directory path
    // Some systems resolve symlinks differently, so we also accept absolute paths
    #[cfg(unix)]
    let is_absolute_path = rendered2.starts_with("/");
    #[cfg(windows)]
    let is_absolute_path = rendered2.len() >= 3 && rendered2.chars().nth(1) == Some(':');

    assert!(
        rendered2.contains(&temp_dir.to_string_lossy().to_string()) || is_absolute_path,
        "Prompt should contain temp dir path, got: {}",
        rendered2
    );
}

#[test]
fn test_status_indicator_with_error_symbol() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let status_config = StatusConfig {
        symbol: StatusSymbol {
            success: "".to_string(),
            error: "✗ ".to_string(),
        },
        override_prompt_color: false,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .status(status_config, StatusColorConfig::default())
            .build();

    // Initially no error - empty status symbol
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    // After command failure - shows error symbol (with color)
    config.set_last_command_failed(true);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Symbol should contain "✗ " (possibly with ANSI color codes)
    assert!(
        rendered.contains("✗"),
        "Should contain error symbol, got: {}",
        rendered
    );
    assert!(
        rendered.ends_with("r> "),
        "Should end with prompt, got: {}",
        rendered
    );

    // After successful command - back to empty
    config.set_last_command_failed(false);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_status_indicator_with_empty_symbols() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    // Both symbols empty - equivalent to old mode=None
    let status_config = StatusConfig {
        symbol: StatusSymbol {
            success: "".to_string(),
            error: "".to_string(),
        },
        override_prompt_color: false,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .status(status_config, StatusColorConfig::default())
            .build();

    // With empty symbols, status placeholder should expand to empty string
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    // Even after failure, still empty
    config.set_last_command_failed(true);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_status_without_placeholder() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    // Test that status config has no effect when {status} placeholder is absent
    let status_config = StatusConfig {
        symbol: StatusSymbol {
            success: "✓ ".to_string(),
            error: "✗ ".to_string(),
        },
        override_prompt_color: false,
    };
    let mut config = PromptRuntimeConfig::builder(
        PromptFormatter::default(),
        "r> ", // No {status} placeholder
        "+  ",
        "$ ",
    )
    .mode_indicator_position(ModeIndicatorPosition::None)
    .status(status_config, StatusColorConfig::default())
    .build();

    // Prompt stays the same regardless of status
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");

    config.set_last_command_failed(true);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_status_override_prompt_color() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let status_config = StatusConfig {
        symbol: StatusSymbol {
            success: "".to_string(),
            error: "✗ ".to_string(),
        },
        override_prompt_color: true, // Enable prompt color override
    };
    let status_colors = StatusColorConfig {
        success: Color::Green,
        error: Color::Red,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{status}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .main_color(Color::LightGreen) // Normal main color
            .status(status_config, status_colors)
            .build();

    // On success, prompt should use success color (Green)
    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // The prompt text "r> " should be colored with Green
    assert!(
        rendered.contains("r> "),
        "Should contain prompt text, got: {}",
        rendered
    );

    // On failure, prompt should use error color (Red)
    config.set_last_command_failed(true);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Should contain both the error symbol and prompt
    assert!(
        rendered.contains("✗") && rendered.contains("r>"),
        "Should contain error symbol and prompt, got: {}",
        rendered
    );

    config.set_last_command_outcome_unknown();
    assert_eq!(config.get_status_prompt_color(), Color::LightGreen);
    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    assert!(
        !rendered.contains("✗") && rendered.contains("r>"),
        "Unknown status should clear the symbol and retain the main prompt: {rendered}"
    );
}

#[test]
fn test_spinner_not_started_in_shell_mode() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    let mut config = create_test_config(ReprexMode::Off);
    config.set_shell(true);
    // In shell mode, start_spinner should be a no-op (no panic, etc.)
    config.start_spinner();
    // Verify shell mode is still enabled
    assert!(config.is_shell_enabled());
}

#[test]
fn test_spinner_not_started_with_empty_frames() {
    // create_test_config reads SHELL through PromptFormatter::new.
    let _guard = crate::test_utils::lock_env();
    // Create config with empty spinner frames (disabled by default)
    let config = create_test_config(ReprexMode::Off);
    // Should not panic when spinner is disabled
    config.start_spinner();
}

#[test]
fn test_render_time_seconds_only() {
    assert_eq!(render_time(Duration::from_secs(5)), "5s");
    assert_eq!(render_time(Duration::from_secs(59)), "59s");
}

#[test]
fn test_render_time_minutes_and_seconds() {
    assert_eq!(render_time(Duration::from_secs(60)), "1m0s");
    assert_eq!(render_time(Duration::from_secs(90)), "1m30s");
    assert_eq!(render_time(Duration::from_secs(3599)), "59m59s");
}

#[test]
fn test_render_time_hours() {
    assert_eq!(render_time(Duration::from_secs(3600)), "1h0m0s");
    assert_eq!(render_time(Duration::from_secs(3661)), "1h1m1s");
    assert_eq!(render_time(Duration::from_secs(7200)), "2h0m0s");
}

#[test]
fn test_render_time_days() {
    assert_eq!(render_time(Duration::from_secs(86400)), "1d0h0m0s");
    assert_eq!(render_time(Duration::from_secs(90061)), "1d1h1m1s");
}

#[test]
fn test_render_time_subsecond_shows_milliseconds() {
    // Sub-second durations show milliseconds
    assert_eq!(render_time(Duration::from_millis(0)), "0ms");
    assert_eq!(render_time(Duration::from_millis(500)), "500ms");
    assert_eq!(render_time(Duration::from_millis(800)), "800ms");
    assert_eq!(render_time(Duration::from_millis(999)), "999ms");
    // Once >= 1s, subsecond precision is truncated to whole seconds
    assert_eq!(render_time(Duration::from_millis(2500)), "2s");
}

#[test]
fn test_duration_placeholder_below_threshold() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .build();
    // Simulate a fast command (below default 2000ms threshold)
    config.last_command_duration = Some(Duration::from_millis(500));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    // Below threshold -> {duration} should be empty
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_duration_placeholder_above_threshold() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .build();
    config.last_command_duration = Some(Duration::from_secs(5));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Above threshold -> should contain "5s" with default format "{value} "
    assert!(
        rendered.contains("5s"),
        "Should contain duration time, got: {}",
        rendered
    );
    assert!(
        rendered.ends_with("r> "),
        "Should end with prompt, got: {}",
        rendered
    );
}

#[test]
fn test_duration_placeholder_no_data() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .build();

    let prompt = config.build_main_prompt(ReprexMode::Off);
    // No duration data -> {duration} should be empty
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_clear_command_duration() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .build();
    // Set a duration above the default threshold
    config.last_command_duration = Some(Duration::from_secs(5));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    assert!(
        rendered.contains("5s"),
        "Should contain duration before clearing, got: {}",
        rendered
    );

    // Clear and verify duration is no longer rendered
    config.clear_command_duration();
    let prompt = config.build_main_prompt(ReprexMode::Off);
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_duration_placeholder_not_present() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let mut config = PromptRuntimeConfig::builder(PromptFormatter::default(), "r> ", "+  ", "$ ")
        .mode_indicator_position(ModeIndicatorPosition::None)
        .build();
    config.last_command_duration = Some(Duration::from_secs(5));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    // No {duration} in template -> prompt unchanged
    assert_eq!(prompt.render_prompt_left(), "r> ");
}

#[test]
fn test_duration_custom_threshold() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let duration_config = PromptDurationConfig {
        threshold_ms: 500,
        ..PromptDurationConfig::default()
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .duration(duration_config, Color::Default)
            .build();
    // 600ms > 500ms threshold, sub-second shows milliseconds
    config.last_command_duration = Some(Duration::from_millis(600));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    assert!(
        rendered.contains("600ms"),
        "Should contain duration time in milliseconds, got: {}",
        rendered
    );
}

#[test]
fn test_duration_custom_format() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let duration_config = PromptDurationConfig {
        format: "took {value} ".to_string(),
        threshold_ms: 2000,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .duration(duration_config, Color::Default)
            .build();
    config.last_command_duration = Some(Duration::from_secs(5));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Custom format "took {value} " should produce "took 5s "
    assert!(
        rendered.contains("took 5s"),
        "Should contain formatted duration, got: {}",
        rendered
    );
    assert!(
        rendered.ends_with("r> "),
        "Should end with prompt, got: {}",
        rendered
    );
}

#[test]
fn test_duration_format_with_brackets() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let duration_config = PromptDurationConfig {
        format: "({value}) ".to_string(),
        threshold_ms: 2000,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .duration(duration_config, Color::Default)
            .build();
    config.last_command_duration = Some(Duration::from_secs(90));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Custom format "({value}) " should produce "(1m30s) "
    assert!(
        rendered.contains("(1m30s)"),
        "Should contain bracketed duration, got: {}",
        rendered
    );
}

#[test]
fn test_duration_format_without_value_placeholder() {
    // PromptFormatter::new reads SHELL while this test builds the formatter.
    let _guard = crate::test_utils::lock_env();
    let duration_config = PromptDurationConfig {
        format: "slow! ".to_string(),
        threshold_ms: 2000,
    };
    let mut config =
        PromptRuntimeConfig::builder(PromptFormatter::default(), "{duration}r> ", "+  ", "$ ")
            .mode_indicator_position(ModeIndicatorPosition::None)
            .duration(duration_config, Color::Default)
            .build();
    config.last_command_duration = Some(Duration::from_secs(5));

    let prompt = config.build_main_prompt(ReprexMode::Off);
    let rendered = prompt.render_prompt_left();
    // Format without {value}: only static text "slow! " is shown
    assert!(
        rendered.contains("slow!"),
        "Should contain static text from format, got: {}",
        rendered
    );
    assert!(
        !rendered.contains("5s"),
        "Should not contain time value when {{value}} is absent, got: {}",
        rendered
    );
}
