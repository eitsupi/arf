use super::*;

#[test]
fn test_parse_var_from_wrapper_script() {
    // Standard R wrapper format (unquoted)
    let script = r#"
#!/bin/bash
R_HOME_DIR=/usr/lib64/R
R_SHARE_DIR=/usr/share/R
export R_SHARE_DIR
R_INCLUDE_DIR=/usr/include/R
export R_INCLUDE_DIR
R_DOC_DIR=/usr/share/doc/R
export R_DOC_DIR
"#;
    let r_home = Path::new("/opt/R");
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", r_home),
        Some(OsString::from("/usr/share/doc/R"))
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_SHARE_DIR", r_home),
        Some(OsString::from("/usr/share/R"))
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_INCLUDE_DIR", r_home),
        Some(OsString::from("/usr/include/R"))
    );

    // Variable not present
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_MISSING_VAR", r_home),
        None
    );
}

#[test]
fn r_home_from_library_path_inverts_r_library_path() {
    let r_home = Path::new("/opt/R/4.5.2");
    assert_eq!(
        r_home_from_library_path(&r_library_path(r_home)),
        Some(r_home.to_path_buf())
    );
}

#[test]
fn r_home_from_library_path_rejects_other_files() {
    assert_eq!(
        r_home_from_library_path(Path::new("/opt/R/4.5.2/lib/R/not-libR")),
        None
    );
}

#[test]
fn r_home_from_rhome_output_extracts_path() {
    for (name, output) in [
        (
            "after warning",
            "WARNING: ignoring environment value of R_HOME\n/opt/R/4.0.5/lib/R\n",
        ),
        ("normal output", "/opt/R/4.0.5/lib/R\n"),
        ("missing trailing newline", "/opt/R/4.0.5/lib/R"),
    ] {
        assert_eq!(
            r_home_from_rhome_output(output),
            Some("/opt/R/4.0.5/lib/R".to_string()),
            "{name}"
        );
    }
}

#[test]
fn r_home_from_rhome_output_rejects_empty_output() {
    assert_eq!(r_home_from_rhome_output(""), None);
    assert_eq!(
        r_home_from_rhome_output(
            r" 
	 "
        ),
        None
    );
    assert_eq!(
        r_home_from_rhome_output(
            r"WARNING: ignoring environment value of R_HOME
"
        ),
        None
    );
}

#[test]
fn test_parse_var_from_wrapper_script_quoted() {
    for (style, script) in [
        (
            "single-quoted",
            "R_DOC_DIR='/usr/share/doc/R'\nexport R_DOC_DIR\n",
        ),
        (
            "double-quoted",
            "R_DOC_DIR=\"/usr/share/doc/R\"\nexport R_DOC_DIR\n",
        ),
    ] {
        assert_eq!(
            parse_var_from_wrapper_script(script, "R_DOC_DIR", Path::new("/opt/R")),
            Some(OsString::from("/usr/share/doc/R")),
            "{style}"
        );
    }
}

#[test]
fn parse_wrapper_expands_only_known_r_home_references() {
    let r_home = Path::new("/opt/R/4.5.2/lib/R");
    let script = r#"
R_DOC_DIR=${R_HOME}/doc
R_SHARE_DIR="$R_HOME/share"
R_INCLUDE_DIR='$R_HOME/include'
"#;

    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", r_home),
        Some(OsString::from("/opt/R/4.5.2/lib/R/doc"))
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_SHARE_DIR", r_home),
        Some(OsString::from("/opt/R/4.5.2/lib/R/share"))
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_INCLUDE_DIR", r_home),
        Some(OsString::from("$R_HOME/include"))
    );

    let script = r#"R_DOC_DIR=$R_HOME_SUFFIX/doc
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", r_home),
        Some(OsString::from("$R_HOME_SUFFIX/doc"))
    );

    let script = r#"R_DOC_DIR=$(printf '%s/doc' "$R_HOME")
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", r_home),
        Some(OsString::from(r#"$(printf '%s/doc' "$R_HOME")"#))
    );

    let script = r#"R_DOC_DIR="\${R_HOME}/doc"
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", r_home),
        Some(OsString::from(r#"\${R_HOME}/doc"#))
    );
}

#[cfg(unix)]
#[test]
fn parse_wrapper_expands_non_utf8_r_home_without_loss() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let r_home = Path::new(std::ffi::OsStr::from_bytes(b"/opt/R-\xff"));
    assert_eq!(
        parse_var_from_wrapper_script("R_DOC_DIR=${R_HOME}/doc\n", "R_DOC_DIR", r_home),
        Some(OsString::from_vec(b"/opt/R-\xff/doc".to_vec()))
    );
}

#[test]
fn test_parse_var_from_wrapper_script_empty_value() {
    let script = r#"R_DOC_DIR=
export R_DOC_DIR
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", Path::new("/opt/R")),
        None
    );
}

#[test]
fn test_parse_var_from_wrapper_script_no_partial_prefix_match() {
    // R_DOC_DIR_EXTRA should NOT match when looking for R_DOC_DIR
    let script = r#"R_DOC_DIR_EXTRA=/some/path
R_DOC_DIR=/usr/share/doc/R
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", Path::new("/opt/R")),
        Some(OsString::from("/usr/share/doc/R"))
    );

    // If only the longer name exists, R_DOC_DIR should not match
    let script = r#"R_DOC_DIR_EXTRA=/some/path
"#;
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR", Path::new("/opt/R")),
        None
    );
}

#[test]
fn test_set_r_path_vars_from_wrapper_skips_existing_env() {
    let _env_guard = crate::sys::test_utils::RDocDirGuard::new("/custom/doc");

    // Create a temp dir with a fake wrapper script (auto-cleaned on drop)
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("R"),
        "R_DOC_DIR=/should/not/override\nexport R_DOC_DIR\n",
    )
    .unwrap();

    set_r_path_vars_from_wrapper(tmp.path());

    // R_DOC_DIR should NOT be overwritten
    assert_eq!(std::env::var("R_DOC_DIR").unwrap(), "/custom/doc");
}

#[cfg(unix)]
#[test]
fn set_r_path_vars_from_wrapper_preserves_non_utf8_existing_env() {
    use std::os::unix::ffi::OsStringExt;

    let _env_guard = crate::sys::test_utils::RDocDirGuard::new("temporary");
    let existing = OsString::from_vec(b"/custom/doc-\xff".to_vec());
    // SAFETY: RDocDirGuard holds the environment test mutex and restores the original value.
    unsafe { std::env::set_var("R_DOC_DIR", &existing) };

    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("R"),
        "R_DOC_DIR=${R_HOME}/doc\nexport R_DOC_DIR\n",
    )
    .unwrap();

    set_r_path_vars_from_wrapper(tmp.path());

    assert_eq!(std::env::var_os("R_DOC_DIR"), Some(existing));
}
