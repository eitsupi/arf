use super::*;

fn completion_suggestions(
    completer: &mut MetaCommandCompleter,
    line: &str,
    pos: usize,
) -> Vec<Suggestion> {
    completer.complete(line, pos).suggestions().to_vec()
}

#[test]
fn test_meta_command_completer_lists_all_commands() {
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":", 1);
    // With no partial input there is nothing to highlight.
    for suggestion in &suggestions {
        assert_eq!(
            suggestion.match_indices, None,
            "`:` with no partial input should have match_indices: None"
        );
    }
    let listing: Vec<String> = suggestions
        .iter()
        .map(|s| {
            format!(
                "{} append_whitespace={} description={:?}",
                s.value,
                s.append_whitespace,
                s.description.as_deref().unwrap_or_default()
            )
        })
        .collect();
    insta::assert_snapshot!(listing.join("\n"), @r#"
    h append_whitespace=false description="Search R help (alias)"
    r append_whitespace=false description="Return to R mode"
    cd append_whitespace=true description="Change working directory"
    ipc append_whitespace=true description="IPC server and send-policy management"
    help append_whitespace=false description="Search R help"
    info append_whitespace=false description="Show session information"
    cmds append_whitespace=false description="Show available commands (alias)"
    popd append_whitespace=false description="Pop directory from stack"
    quit append_whitespace=false description="Quit arf"
    exit append_whitespace=false description="Quit arf"
    shell append_whitespace=false description="Enter shell mode"
    pushd append_whitespace=true description="Push directory and change to it"
    system append_whitespace=true description="Execute system command"
    reprex append_whitespace=true description="Set reprex mode (on, off, or format)"
    switch append_whitespace=true description="Restart with different R version (requires rig)"
    session append_whitespace=false description="Show session information (alias)"
    restart append_whitespace=false description="Restart R session"
    switch! append_whitespace=true description="Restart with different R version, skip confirmation (requires rig)"
    history append_whitespace=true description="Manage command history"
    commands append_whitespace=false description="Show available commands"
    restart! append_whitespace=false description="Restart R session (skip confirmation)"
    changelog append_whitespace=false description="Show arf changelog"
    "#);
}

#[test]
fn test_meta_command_completer_partial_command() {
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":rep", 4);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].value, "reprex");
    // The matched prefix is highlighted.
    assert_eq!(suggestions[0].match_indices, Some(vec![0, 1, 2]));
}

#[test]
fn test_meta_command_completer_no_subcommands() {
    let mut completer = MetaCommandCompleter::new();
    // Reprex accepts the format subcommand.
    let suggestions = completion_suggestions(&mut completer, ":reprex ", 8);
    assert_eq!(
        suggestions
            .iter()
            .map(|s| s.value.as_str())
            .collect::<Vec<_>>(),
        ["on", "off", "format"]
    );
    let suggestions = completion_suggestions(&mut completer, ":commands ", 10);
    assert!(suggestions.is_empty());
}

#[test]
fn test_meta_command_completer_not_meta_command() {
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, "print(x)", 8);
    assert!(suggestions.is_empty());
}

#[test]
fn test_meta_command_completer_excludes_r_command() {
    // In R mode, `:r` should be excluded from completion
    let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
    let suggestions = completion_suggestions(&mut completer, ":", 1);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(!values.contains(&"r"), "`:r` should be excluded in R mode");
    assert!(
        values.contains(&"shell"),
        "`:shell` should still be present"
    );
}

#[test]
fn test_meta_command_completer_excludes_shell_mode_commands() {
    // In Shell mode, R-specific commands should be excluded from completion
    let mut completer =
        MetaCommandCompleter::with_exclusions(MetaCommandCompleter::shell_mode_exclusions());
    let suggestions = completion_suggestions(&mut completer, ":", 1);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();

    // These should be excluded in Shell mode
    assert!(!values.contains(&"shell"), "`:shell` should be excluded");
    assert!(!values.contains(&"system"), "`:system` should be excluded");
    assert!(
        !values.contains(&"restart"),
        "`:restart` should be excluded"
    );
    assert!(!values.contains(&"reprex"), "`:reprex` should be excluded");
    assert!(!values.contains(&"switch"), "`:switch` should be excluded");
    assert!(!values.contains(&"h"), "`:h` should be excluded");
    assert!(!values.contains(&"help"), "`:help` should be excluded");

    // These should still be present in Shell mode
    assert!(values.contains(&"r"), "`:r` should be present");
    assert!(
        values.contains(&"commands"),
        "`:commands` should be present"
    );
    assert!(values.contains(&"quit"), "`:quit` should be present");
    assert!(values.contains(&"exit"), "`:exit` should be present");
}

#[test]
fn test_meta_command_completer_exclusion_affects_partial_match() {
    // Even with partial match, excluded commands should not appear
    let mut completer = MetaCommandCompleter::with_exclusions(vec!["r"]);
    // Typing ":r" should not match excluded "r" command
    let suggestions = completion_suggestions(&mut completer, ":r", 2);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(
        !values.contains(&"r"),
        "`:r` should not appear even with partial match"
    );
    // But "restart" and "reprex" should still appear
    assert!(values.contains(&"restart"));
    assert!(values.contains(&"reprex"));
}

#[test]
fn test_meta_command_append_whitespace_for_commands_with_arguments() {
    // Commands that take arguments should have append_whitespace: true
    let mut completer = MetaCommandCompleter::new();

    // :switch takes an argument (R version)
    let suggestions = completion_suggestions(&mut completer, ":sw", 3);
    let switch = suggestions.iter().find(|s| s.value == "switch").unwrap();
    assert!(
        switch.append_whitespace,
        "`:switch` should append whitespace because it takes an argument"
    );

    // :system takes an argument (shell command)
    let suggestions = completion_suggestions(&mut completer, ":sys", 4);
    let system = suggestions.iter().find(|s| s.value == "system").unwrap();
    assert!(
        system.append_whitespace,
        "`:system` should append whitespace because it takes an argument"
    );
}

#[test]
fn test_meta_command_match_indices_single_char() {
    // Single character fuzzy match - 'r' matches at different positions in different commands
    let mut completer = MetaCommandCompleter::new();

    let suggestions = completion_suggestions(&mut completer, ":r", 2);
    // With fuzzy matching, `:r` matches any command containing 'r':
    // - "r" at position 0
    // - "reprex" at position 0
    // - "restart" at position 0
    // Formatter aliases are not completion candidates.
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(values.contains(&"r"));
    assert!(values.contains(&"reprex"));
    assert!(values.contains(&"restart"));

    // Verify match_indices are correct for each
    for suggestion in &suggestions {
        let expected_pos = suggestion
            .value
            .find('r')
            .or_else(|| suggestion.value.find('R'));
        assert_eq!(
            suggestion.match_indices,
            expected_pos.map(|p| vec![p]),
            "`:r` should highlight first 'r' position in `{}`",
            suggestion.value
        );
    }
}

// --- Fuzzy matching integration tests ---
// Note: Direct fuzzy_match tests are in fuzzy.rs module

#[test]
fn test_meta_command_fuzzy_matching() {
    let mut completer = MetaCommandCompleter::new();

    // ":rst" should fuzzy match "restart"
    let suggestions = completion_suggestions(&mut completer, ":rst", 4);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(
        values.contains(&"restart"),
        "`:rst` should fuzzy match `restart`, got: {:?}",
        values
    );

    // Check match_indices for fuzzy match
    let restart = suggestions.iter().find(|s| s.value == "restart").unwrap();
    assert_eq!(
        restart.match_indices,
        Some(vec![0, 2, 3]),
        "`:rst` should highlight positions 0, 2, 3 in `restart`"
    );
}

#[test]
fn test_meta_command_fuzzy_matching_cms_cmds() {
    let mut completer = MetaCommandCompleter::new();

    // ":cms" should match "cmds" - c=0, m=1, d=2, s=3
    let suggestions = completion_suggestions(&mut completer, ":cms", 4);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(values.contains(&"cmds"), "`:cms` should fuzzy match `cmds`");

    let cmds = suggestions.iter().find(|s| s.value == "cmds").unwrap();
    assert_eq!(
        cmds.match_indices,
        Some(vec![0, 1, 3]),
        "`:cms` should highlight positions 0, 1, 3 in `cmds`"
    );
}

#[test]
fn test_meta_command_fuzzy_no_match() {
    let mut completer = MetaCommandCompleter::new();

    // ":xyz" should not match any command
    let suggestions = completion_suggestions(&mut completer, ":xyz", 4);
    assert!(
        suggestions.is_empty(),
        "`:xyz` should not match any command"
    );
}

// --- cd/pushd/popd completion tests ---

#[test]
fn test_meta_cd_completes_directories() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();
    std::fs::File::create(tmp.path().join("file.txt")).unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":cd ", 4);

    // Should only contain directories (directories_only: true)
    assert!(!suggestions.is_empty(), "Should have directory completions");
    for s in &suggestions {
        assert!(
            s.value.ends_with('/'),
            "cd completion should only show directories, got: {}",
            s.value
        );
    }
    assert!(
        suggestions.iter().any(|s| s.value == "subdir/"),
        "Should contain subdir/"
    );
    assert!(
        !suggestions.iter().any(|s| s.value == "file.txt"),
        "Should not contain files"
    );
}

#[test]
fn test_meta_cd_partial_path() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":cd su", 6);

    assert!(
        suggestions.iter().any(|s| s.value == "subdir/"),
        "Should fuzzy-match subdir/, got: {:?}",
        suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
}

#[test]
fn test_meta_pushd_completes_directories() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("mydir")).unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":pushd ", 7);

    assert!(
        suggestions.iter().any(|s| s.value == "mydir/"),
        "Should contain mydir/"
    );
}

#[test]
fn test_meta_popd_no_completion() {
    // ":popd " should NOT show path completions (takes no argument)
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":popd ", 6);
    assert!(
        suggestions.is_empty(),
        "popd should not have completions, got: {:?}",
        suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
}

#[test]
fn test_meta_cd_nested_path() {
    let _guard = crate::test_utils::lock_cwd();
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    std::fs::create_dir(src.join("inner")).unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":cd src/", 8);

    assert!(
        suggestions.iter().any(|s| s.value == "src/inner/"),
        "Should list src/inner/, got: {:?}",
        suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
}

// --- :switch! / :restart! completion tests ---

#[test]
fn test_meta_command_switch_bang_appears_in_completions() {
    let mut completer = MetaCommandCompleter::new();
    // ":sw" should fuzzy match both "switch" and "switch!"
    let suggestions = completion_suggestions(&mut completer, ":sw", 3);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(values.contains(&"switch"), "should match :switch");
    assert!(values.contains(&"switch!"), "should match :switch!");

    // Both should append whitespace (they take arguments)
    let switch_bang = suggestions.iter().find(|s| s.value == "switch!").unwrap();
    assert!(
        switch_bang.append_whitespace,
        ":switch! should append whitespace because it takes an argument"
    );
}

#[test]
fn test_meta_command_restart_bang_appears_in_completions() {
    let mut completer = MetaCommandCompleter::new();
    // ":restart" should fuzzy match both "restart" and "restart!"
    let suggestions = completion_suggestions(&mut completer, ":restart", 8);
    let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
    assert!(values.contains(&"restart"), "should match :restart");
    assert!(values.contains(&"restart!"), "should match :restart!");

    // restart! should NOT append whitespace (it takes no argument)
    let restart_bang = suggestions.iter().find(|s| s.value == "restart!").unwrap();
    assert!(
        !restart_bang.append_whitespace,
        ":restart! should NOT append whitespace"
    );
}

#[test]
fn test_meta_command_switch_bang_subcommand_completion() {
    // ":switch! " should enter the version completion branch (same as ":switch ")
    // Without rig installed, both return empty, but the key thing is they take
    // the same code path (not falling through to the default empty vec).
    let mut completer = MetaCommandCompleter::new();
    let switch_suggestions = completion_suggestions(&mut completer, ":switch ", 8);
    let switch_bang_suggestions = completion_suggestions(&mut completer, ":switch! ", 9);
    assert_eq!(
        switch_suggestions.len(),
        switch_bang_suggestions.len(),
        ":switch! should produce the same version completions as :switch"
    );

    // Partial version input should also work
    let switch_partial = completion_suggestions(&mut completer, ":switch 4", 9);
    let switch_bang_partial = completion_suggestions(&mut completer, ":switch! 4", 10);
    assert_eq!(
        switch_partial.len(),
        switch_bang_partial.len(),
        ":switch! with partial version should match :switch"
    );
}

#[test]
fn test_meta_command_reprex_format_subcommand() {
    // ":reprex " offers all supported subcommands.
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":reprex ", 8);
    assert_eq!(
        suggestions
            .iter()
            .map(|s| s.value.as_str())
            .collect::<Vec<_>>(),
        ["on", "off", "format"]
    );
}

#[test]
fn test_meta_command_history_bang_no_subcommands() {
    // ":history! " should NOT offer history subcommands (history! is not a valid command)
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":history! ", 10);
    assert!(
        suggestions.is_empty(),
        ":history! should not offer subcommands, got: {:?}",
        suggestions.iter().map(|s| &s.value).collect::<Vec<_>>()
    );
}

// --- nested subcommand and target completion tests ---

fn values_with_whitespace(suggestions: &[Suggestion]) -> Vec<(&str, bool)> {
    suggestions
        .iter()
        .map(|s| (s.value.as_str(), s.append_whitespace))
        .collect()
}

#[test]
fn test_meta_command_history_subcommands_append_whitespace() {
    let mut completer = MetaCommandCompleter::new();
    let suggestions = completion_suggestions(&mut completer, ":history ", 9);
    assert_eq!(
        values_with_whitespace(&suggestions),
        [("browse", true), ("clear", true), ("schema", true)]
    );
    let partial = completion_suggestions(&mut completer, ":history cl", 11);
    assert_eq!(values_with_whitespace(&partial), [("clear", true)]);
    assert_eq!(partial[0].span.start, 9);
    assert_eq!(partial[0].span.end, 11);
}

#[test]
fn test_meta_command_history_targets() {
    let mut completer = MetaCommandCompleter::new();
    let clear = completion_suggestions(&mut completer, ":history clear ", 15);
    assert_eq!(
        values_with_whitespace(&clear),
        [("r", false), ("shell", false), ("all", false)]
    );
    let browse = completion_suggestions(&mut completer, ":history browse s", 17);
    assert_eq!(values_with_whitespace(&browse), [("shell", false)]);
    let schema = completion_suggestions(&mut completer, ":history schema ", 16);
    assert!(schema.is_empty());
}

#[test]
fn test_meta_command_ipc_subcommands_and_send_policy() {
    let mut completer = MetaCommandCompleter::new();
    let subcommands = completion_suggestions(&mut completer, ":ipc ", 5);
    assert_eq!(
        values_with_whitespace(&subcommands),
        [
            ("start", false),
            ("stop", false),
            ("status", false),
            ("revoke", false),
            ("send-policy", true)
        ]
    );
    let policies = completion_suggestions(&mut completer, ":ipc send-policy ", 17);
    assert_eq!(
        values_with_whitespace(&policies),
        [("prompt", false), ("allow", false)]
    );
    let partial = completion_suggestions(&mut completer, ":ipc send-policy a", 18);
    assert_eq!(values_with_whitespace(&partial), [("allow", false)]);
}
