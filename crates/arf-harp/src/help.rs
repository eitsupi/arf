//! R help system integration.
//!
//! This module provides access to installed package help indexes by reading
//! each package's help metadata files directly.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of searching the installed help database was learned from
//! felp's `fuzzyhelp()` implementation.

use crate::error::{HarpError, HarpResult};
use crate::lib_paths::{installed_package_dir, installed_package_dirs, lib_paths};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use rd_helpdb::{
    DemoIndex, HelpTopicEntry, HelpTopicIndex, PackageHelpDb, VignetteIndex, read_rds_file,
};
use std::ffi::CString;

/// A help topic from R's help database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpTopic {
    /// Package name containing this help topic.
    pub package: String,
    /// Topic name (the alias used to access the help).
    pub topic: String,
    /// All usable aliases that can be used to find this topic, including the
    /// display alias and any duplicates in the package metadata.
    pub aliases: Vec<String>,
    /// Compiled-help database key from `Meta/Rd.rds`, when available.
    pub help_key: Option<String>,
    /// Title/description of the help topic.
    pub title: String,
    /// Type of help entry (e.g., "help", "vignette", "demo").
    pub entry_type: String,
}

impl HelpTopic {
    /// Format the topic as "package::topic" for display.
    pub fn qualified_name(&self) -> String {
        format!("{}::{}", self.package, self.topic)
    }
}

/// Payload for R_ToplevelExec callback.
struct EvalPayload {
    expr: SEXP,
    env: SEXP,
    result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - evaluates the expression.
unsafe extern "C" fn eval_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut EvalPayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    let result = unsafe { (lib.rf_eval)(data.expr, data.env) };
    data.result = Some(result);
}

/// Get help, vignette, and demo topics from installed package metadata.
///
/// This function reads each installed package's Rd topic metadata, vignette
/// index, and demo index independently.
///
/// # Returns
///
/// A vector of `HelpTopic` structs containing package, topic, title, and type.
///
pub fn get_help_topics() -> HarpResult<Vec<HelpTopic>> {
    let mut topics = Vec::new();
    for (package, package_dir) in installed_package_dirs(&lib_paths()?) {
        topics.extend(read_package_topics(&package, &package_dir));
    }
    Ok(topics)
}

fn read_package_topics(package: &str, package_dir: &std::path::Path) -> Vec<HelpTopic> {
    let mut topics = Vec::new();

    // Each metadata source is independent of compiled help files and of the
    // other metadata indexes. A malformed source should not hide the rest.
    match HelpTopicIndex::read_installed(package_dir) {
        Ok(Some(index)) => topics.extend(
            index
                .entries()
                .filter_map(|entry| project_help_topic_entry(package, entry)),
        ),
        Ok(None) => {}
        Err(error) => log::debug!(
            "Skipping unreadable Rd help metadata in {}: {error}",
            package_dir.display()
        ),
    }

    if let Some(object) = read_metadata_index(package_dir.join("Meta/vignette.rds")) {
        match VignetteIndex::from_object(&object) {
            Ok(index) => topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.to_owned(),
                topic: vignette_topic(entry),
                aliases: Vec::new(),
                help_key: None,
                title: entry.title.clone(),
                entry_type: "vignette".to_string(),
            })),
            Err(error) => log::debug!(
                "Skipping malformed vignette metadata in {}: {error}",
                package_dir.display()
            ),
        }
    }

    if let Some(object) = read_metadata_index(package_dir.join("Meta/demo.rds")) {
        match DemoIndex::from_object(&object) {
            Ok(index) => topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.to_owned(),
                topic: entry.name.clone(),
                aliases: Vec::new(),
                help_key: None,
                title: entry.title.clone(),
                entry_type: "demo".to_string(),
            })),
            Err(error) => log::debug!(
                "Skipping malformed demo metadata in {}: {error}",
                package_dir.display()
            ),
        }
    }

    topics
}

fn read_metadata_index(path: std::path::PathBuf) -> Option<rd_rds::RObject> {
    match read_rds_file(&path) {
        Ok(object) => Some(object),
        Err(rd_helpdb::Error::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(error) => {
            log::debug!(
                "Skipping unreadable help metadata {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn project_help_topic_entry(package: &str, entry: &HelpTopicEntry) -> Option<HelpTopic> {
    let aliases: Vec<String> = entry.aliases.iter().flatten().cloned().collect();
    let topic = entry
        .name
        .as_str()
        .filter(|name| aliases.iter().any(|alias| alias == name))
        .or_else(|| entry.aliases.first().and_then(Option::as_deref))?;

    Some(HelpTopic {
        package: package.to_owned(),
        topic: topic.to_owned(),
        aliases,
        help_key: entry
            .topic_key()
            .filter(|key| !key.is_empty())
            .map(str::to_owned),
        title: entry.title.as_str().unwrap_or("").to_owned(),
        entry_type: "help".to_string(),
    })
}

// Mirror R's vignette-topic resolution from the index's filename fields.
fn vignette_topic(entry: &rd_helpdb::VignetteEntry) -> String {
    let (filename, from_file) = if !entry.r.is_empty() {
        (&entry.r, false)
    } else if !entry.pdf.is_empty() {
        (&entry.pdf, false)
    } else {
        (&entry.file, true)
    };
    let filename = if from_file {
        filename.rsplit(['/', '\\']).next().unwrap_or(filename)
    } else {
        filename
    };
    filename
        .rsplit_once('.')
        .map_or_else(|| filename.to_owned(), |(stem, _)| stem.to_owned())
}

#[cfg(test)]
mod help_metadata_tests {
    use super::{
        get_package_help_markdown_in_dir, package_help_markdown_by_key_in_dir,
        project_help_topic_entry, read_package_topics, vignette_topic,
    };
    use rd_helpdb::VignetteEntry;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/help_metadata")
            .join(name)
    }

    fn entry(file: &str, pdf: &str, r: &str) -> VignetteEntry {
        VignetteEntry {
            file: file.to_owned(),
            title: String::new(),
            pdf: pdf.to_owned(),
            r: r.to_owned(),
            depends: Vec::new(),
            keywords: Vec::new(),
        }
    }

    #[test]
    fn resolves_vignette_topic_from_r_pdf_or_file() {
        assert_eq!(
            vignette_topic(&entry("ignored.Rmd", "ignored.pdf", "guide.R")),
            "guide"
        );
        assert_eq!(
            vignette_topic(&entry("ignored.Rmd", "guide.pdf", "")),
            "guide"
        );
        assert_eq!(
            vignette_topic(&entry("vignettes/guide.Rmd", "", "")),
            "guide"
        );
        assert_eq!(vignette_topic(&entry("guide", "", "")), "guide");
    }

    fn topic_index() -> rd_helpdb::HelpTopicIndex {
        let object = rd_helpdb::read_rds_file(fixture_path("help_topics_metadata_v3.rds"))
            .expect("metadata fixture should decode");
        rd_helpdb::HelpTopicIndex::from_object(&object).expect("metadata fixture should validate")
    }

    fn topic_index_with_aliases(
        row: usize,
        aliases: Vec<rd_rds::RStr>,
    ) -> rd_helpdb::HelpTopicIndex {
        let object = rd_helpdb::read_rds_file(fixture_path("help_topics_metadata_v3.rds")).unwrap();
        let (rd_rds::RValue::List(mut columns), attributes) = object.into_parts() else {
            panic!("metadata fixture root should be a list");
        };
        let names = object_names(&attributes);
        let alias_column = names
            .iter()
            .position(|name| name.as_str().unwrap().unwrap() == "Aliases")
            .unwrap();
        let rd_rds::RValue::List(mut alias_rows) = columns[alias_column].value().clone() else {
            panic!("Aliases should be a list column");
        };
        alias_rows[row] = rd_rds::RObject::from_parts(
            rd_rds::RValue::Character(aliases),
            rd_rds::Attributes::default(),
        );
        columns[alias_column] = rd_rds::RObject::from_parts(
            rd_rds::RValue::List(alias_rows),
            rd_rds::Attributes::default(),
        );
        let object = rd_rds::RObject::from_parts(rd_rds::RValue::List(columns), attributes);
        rd_helpdb::HelpTopicIndex::from_object(&object).unwrap()
    }

    fn compiled_help_package(with_aliases: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("fixturepkg");
        let help_dir = package_dir.join("help");
        std::fs::create_dir_all(&help_dir).unwrap();
        for extension in ["rdx", "rdb"] {
            std::fs::copy(
                fixture_path(&format!("help_topics.{extension}")),
                help_dir.join(format!("fixturepkg.{extension}")),
            )
            .unwrap();
        }
        if with_aliases {
            std::fs::copy(
                fixture_path("aliases_vector_dup_v3.rds"),
                help_dir.join("aliases.rds"),
            )
            .unwrap();
        }
        (temp, package_dir)
    }

    #[test]
    fn help_entry_projection_prefers_matching_name_and_keeps_all_aliases() {
        let index = topic_index();
        let topic = project_help_topic_entry("fixture", index.entries().next().unwrap())
            .expect("first metadata row should be visible");

        assert_eq!(topic.topic, "first");
        assert_eq!(topic.aliases, ["shared", "first", "first"]);
        assert_eq!(topic.help_key.as_deref(), Some("first-topic"));
        assert_eq!(topic.title, "First topic title");
    }

    #[test]
    fn help_entry_projection_uses_only_first_alias_when_name_is_not_an_alias() {
        let index = topic_index_with_aliases(
            2,
            vec![
                rd_rds::RStr::new(
                    b"first-alias",
                    rd_rds::REncoding::Utf8,
                    rd_rds::NativeEncodingSource::Unknown,
                ),
                rd_rds::RStr::new(
                    b"later-alias",
                    rd_rds::REncoding::Utf8,
                    rd_rds::NativeEncodingSource::Unknown,
                ),
            ],
        );
        let entry = index.entries().nth(2).unwrap();
        let topic = project_help_topic_entry("fixture", entry).unwrap();
        assert_eq!(topic.topic, "first-alias");
        assert_eq!(topic.aliases, ["first-alias", "later-alias"]);
    }

    #[test]
    fn help_entry_projection_skips_first_na_even_when_a_later_alias_is_usable() {
        let index = topic_index_with_aliases(
            0,
            vec![
                rd_rds::RStr::Na,
                rd_rds::RStr::new(
                    b"later",
                    rd_rds::REncoding::Utf8,
                    rd_rds::NativeEncodingSource::Unknown,
                ),
            ],
        );

        assert!(project_help_topic_entry("fixture", index.entries().next().unwrap()).is_none());
    }

    #[test]
    fn help_entry_projection_uses_empty_title_for_missing_or_na_and_skips_empty_alias_groups() {
        let index = topic_index();
        let entries: Vec<_> = index.entries().collect();
        let topic = project_help_topic_entry("fixture", entries[1]).unwrap();
        assert_eq!(topic.title, "");
        assert_eq!(project_help_topic_entry("fixture", entries[3]), None);

        let missing_title =
            rd_helpdb::read_rds_file(fixture_path("help_topics_aliases_only_v3.rds")).unwrap();
        let missing_index = rd_helpdb::HelpTopicIndex::from_object(&missing_title).unwrap();
        assert_eq!(
            project_help_topic_entry("fixture", missing_index.entries().next().unwrap())
                .unwrap()
                .title,
            ""
        );
    }

    #[test]
    fn metadata_sources_are_independent_of_compiled_help_and_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let metadata = temp.path().join("Meta");
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::copy(
            fixture_path("help_topics_metadata_v3.rds"),
            metadata.join("Rd.rds"),
        )
        .unwrap();
        std::fs::write(metadata.join("vignette.rds"), b"broken RDS").unwrap();
        std::fs::copy(fixture_path("demo_valid_v3.rds"), metadata.join("demo.rds")).unwrap();

        let topics = read_package_topics("fixture", temp.path());
        assert!(topics.iter().any(|topic| topic.entry_type == "help"));
        assert!(topics.iter().any(|topic| topic.entry_type == "demo"));
        assert!(!topics.iter().any(|topic| topic.entry_type == "vignette"));
        assert!(!temp.path().join("help").exists());

        std::fs::copy(
            fixture_path("vignette_reordered_v3.rds"),
            metadata.join("vignette.rds"),
        )
        .unwrap();
        std::fs::write(metadata.join("demo.rds"), b"broken RDS").unwrap();
        let topics = read_package_topics("fixture", temp.path());
        assert!(topics.iter().any(|topic| topic.entry_type == "help"));
        assert!(topics.iter().any(|topic| topic.entry_type == "vignette"));
        assert!(!topics.iter().any(|topic| topic.entry_type == "demo"));

        let compiled_help = temp.path().join("help");
        std::fs::create_dir(&compiled_help).unwrap();
        std::fs::write(compiled_help.join("aliases.rds"), b"broken RDS").unwrap();
        std::fs::write(compiled_help.join("fixture.rdx"), b"broken index").unwrap();
        std::fs::write(compiled_help.join("fixture.rdb"), b"broken database").unwrap();
        let topics = read_package_topics("fixture", temp.path());
        assert!(topics.iter().any(|topic| topic.entry_type == "help"));

        std::fs::write(metadata.join("Rd.rds"), b"broken RDS").unwrap();
        std::fs::copy(fixture_path("demo_valid_v3.rds"), metadata.join("demo.rds")).unwrap();
        let topics = read_package_topics("fixture", temp.path());
        assert!(!topics.iter().any(|topic| topic.entry_type == "help"));
        assert!(topics.iter().any(|topic| topic.entry_type == "vignette"));
        assert!(topics.iter().any(|topic| topic.entry_type == "demo"));
    }

    #[test]
    fn known_help_key_reads_compiled_topic_without_aliases_file() {
        let (_temp, package_dir) = compiled_help_package(false);
        assert!(!package_dir.join("help/aliases.rds").exists());

        let markdown = package_help_markdown_by_key_in_dir(
            "display-only-name",
            "first-topic",
            "fixturepkg",
            &package_dir,
        )
        .expect("direct known-key lookup should not need aliases.rds");

        assert!(!markdown.is_empty());
    }

    #[test]
    fn generic_lookup_keeps_alias_first_and_exact_key_fallback_semantics() {
        let (_temp, package_dir) = compiled_help_package(true);

        let alias_error = get_package_help_markdown_in_dir("shared", "fixturepkg", &package_dir)
            .expect_err("alias lookup should take precedence over treating input as a key");
        assert!(matches!(
            alias_error,
            crate::error::HarpError::HelpDatabase { key, source, .. }
                if key == "second-topic"
                    && matches!(*source, rd_helpdb::Error::UnknownTopic { ref topic } if topic == "second-topic")
        ));

        let exact_result =
            get_package_help_markdown_in_dir("first-topic", "fixturepkg", &package_dir)
                .expect("non-alias input should fall back to its exact key");
        let direct_exact = package_help_markdown_by_key_in_dir(
            "first-topic",
            "first-topic",
            "fixturepkg",
            &package_dir,
        )
        .expect("exact key should load directly");
        assert_eq!(exact_result, direct_exact);
    }

    fn object_names(attributes: &rd_rds::Attributes) -> Vec<rd_rds::RStr> {
        attributes
            .get("names")
            .and_then(|names| match names.value() {
                rd_rds::RValue::Character(names) => Some(names.clone()),
                _ => None,
            })
            .expect("metadata names")
    }
}

/// Evaluate R code and return the result as an optional String.
///
/// This shared helper handles the common pattern of:
/// parsing R code, evaluating it via `R_ToplevelExec`, and extracting
/// a character string result.
///
/// Returns `Ok(Some(text))` if evaluation produces a character result,
/// `Ok(None)` if the result is `NULL`, or `Err` on failure.
///
/// # Safety
///
/// R must already be initialized and the caller must ensure that evaluation
/// happens on R's thread.
pub unsafe fn eval_r_to_string(code: &str) -> HarpResult<Option<String>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
        expected: "string without interior NUL bytes".to_string(),
        actual: "string containing interior NUL byte(s)".to_string(),
    })?;

    unsafe {
        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Failed to parse R code".to_string(),
            )));
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Empty R expression".to_string(),
            )));
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let base_env = *lib.r_baseenv;

        let mut payload = EvalPayload {
            expr,
            env: base_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "R evaluation failed".to_string(),
            )));
        }

        let Some(result) = payload.result else {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "No result from R evaluation".to_string(),
            )));
        };

        if result == r_nil_value()? {
            return Ok(None);
        }

        // Check if it's a character vector (STRSXP = 16)
        let sexp_type = (lib.rf_typeof)(result);
        if sexp_type != 16 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Unexpected result type from R".to_string(),
            )));
        }

        let len = (lib.rf_length)(result);
        if len == 0 {
            return Ok(None);
        }

        let str_elt = (lib.string_elt)(result, 0);
        let char_ptr = (lib.r_charsxp)(str_elt);
        if char_ptr.is_null() {
            return Ok(None);
        }

        let c_str = std::ffi::CStr::from_ptr(char_ptr);
        let text = c_str.to_string_lossy().into_owned();
        Ok(Some(text))
    }
}

/// Get help text for a specific topic.
///
/// This retrieves the help content as plain text using `tools::Rd2txt()`,
/// bypassing R's pager system. This is important on Windows where R's
/// help() function may try to open a GUI window.
///
/// The approach is inspired by the felp package's `get_help()` function.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
///
/// # Returns
///
/// The help text as a String, or an error if the topic is not found.
pub fn get_help_text(topic: &str, package: Option<&str>) -> HarpResult<String> {
    let code = if let Some(pkg) = package {
        format!(
            r#"local({{
    x <- utils::help("{topic}", package = "{pkg}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    pkgname <- basename(dirname(dirname(file)))
    paste(utils::capture.output(
        tools::Rd2txt(utils:::.getHelpFile(file), package = pkgname)
    ), collapse = "\n")
}})"#,
            topic = escape_r_string(topic),
            pkg = escape_r_string(pkg)
        )
    } else {
        format!(
            r#"local({{
    x <- utils::help("{topic}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    pkgname <- basename(dirname(dirname(file)))
    paste(utils::capture.output(
        tools::Rd2txt(utils:::.getHelpFile(file), package = pkgname)
    ), collapse = "\n")
}})"#,
            topic = escape_r_string(topic)
        )
    };

    unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "No help found for topic '{}'",
                topic
            )))
        })
    }
}

/// Get help content as Markdown for a specific topic.
///
/// When `package` is known, this reads the installed package's compiled help
/// database directly. Without a package, it retains the R-evaluation-based
/// resolution needed for attached-package and search-path semantics.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
///
/// # Returns
///
/// The help content as a Markdown string, or an error if the topic is not found.
pub fn get_help_markdown(topic: &str, package: Option<&str>) -> HarpResult<String> {
    match package {
        Some(package) => get_package_help_markdown(topic, package),
        None => get_help_markdown_via_r(topic),
    }
}

fn rd_convert_options() -> rd2qmd_core::RdConvertOptions {
    let mut options = rd2qmd_core::RdConvertOptions::default();
    options.code.quarto_code_blocks = false;
    options.arguments_format = rd2qmd_core::ArgumentsFormat::List;
    options.describe_format = rd2qmd_core::DescribeFormat::Headings;
    options
}

/// Get package help as Markdown without evaluating R for the help database.
///
/// The package directory is selected from the startup-cached library paths,
/// refreshed as needed by [`crate::lib_paths::lib_paths`].
///
/// `topic` is treated as an alias-or-exact-key input: aliases are resolved
/// first using the compiled help database's last-wins alias index, then the
/// input is used as an exact key if it is not an alias.
pub fn get_package_help_markdown(topic: &str, package: &str) -> HarpResult<String> {
    let package_dir = installed_package_dir(&lib_paths()?, package).ok_or_else(|| {
        HarpError::PackageNotFound {
            package: package.to_string(),
        }
    })?;
    get_package_help_markdown_in_dir(topic, package, &package_dir)
}

fn get_package_help_markdown_in_dir(
    topic: &str,
    package: &str,
    package_dir: &std::path::Path,
) -> HarpResult<String> {
    let db = open_package_help_db(package_dir, package, topic, topic)?;
    let resolved = db
        .resolve_alias(topic)
        .map_err(|source| HarpError::HelpDatabase {
            package: package.to_string(),
            topic: topic.to_string(),
            key: topic.to_string(),
            source: Box::new(source),
        })?;
    let key = resolved.unwrap_or(topic).to_string();
    package_help_markdown_from_db(&db, topic, &key, package)
}

/// Get package help as Markdown using a known compiled-help key directly.
///
/// Unlike [`get_package_help_markdown`], this path never reads
/// `help/aliases.rds`. It is intended for indexed help rows carrying a key
/// from `Meta/Rd.rds`.
pub fn get_package_help_markdown_by_key(
    display_topic: &str,
    help_key: &str,
    package: &str,
) -> HarpResult<String> {
    let package_dir = installed_package_dir(&lib_paths()?, package).ok_or_else(|| {
        HarpError::PackageNotFound {
            package: package.to_string(),
        }
    })?;
    package_help_markdown_by_key_in_dir(display_topic, help_key, package, &package_dir)
}

fn package_help_markdown_by_key_in_dir(
    display_topic: &str,
    help_key: &str,
    package: &str,
    package_dir: &std::path::Path,
) -> HarpResult<String> {
    let db = open_package_help_db(package_dir, package, display_topic, help_key)?;
    package_help_markdown_from_db(&db, display_topic, help_key, package)
}

fn open_package_help_db(
    package_dir: &std::path::Path,
    package: &str,
    display_topic: &str,
    lookup_key: &str,
) -> HarpResult<PackageHelpDb> {
    PackageHelpDb::open(package_dir).map_err(|source| HarpError::HelpDatabase {
        package: package.to_string(),
        topic: display_topic.to_string(),
        key: lookup_key.to_string(),
        source: Box::new(source),
    })
}

fn package_help_markdown_from_db(
    db: &PackageHelpDb,
    display_topic: &str,
    lookup_key: &str,
    package: &str,
) -> HarpResult<String> {
    let robj = db
        .raw_topic(lookup_key)
        .map_err(|source| HarpError::HelpDatabase {
            package: package.to_string(),
            topic: display_topic.to_string(),
            key: lookup_key.to_string(),
            source: Box::new(source),
        })?;
    let doc = rd_ast::lower_r_object(&robj).map_err(|source| HarpError::HelpLowering {
        package: package.to_string(),
        topic: display_topic.to_string(),
        key: lookup_key.to_string(),
        source: Box::new(source),
    })?;
    let options = rd_convert_options();
    Ok(rd2qmd_core::convert_rd_document(&doc, &options))
}

fn get_help_markdown_via_r(topic: &str) -> HarpResult<String> {
    let code = format!(
        r#"local({{
    x <- utils::help("{topic}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    rd <- utils:::.getHelpFile(file)
    paste0(as.character(rd, deparse = TRUE), collapse = "")
}})"#,
        topic = escape_r_string(topic)
    );

    let rd_content = unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "No help found for topic '{}'",
                topic
            )))
        })?
    };

    let parsed = rd_source::parse(rd_content.as_bytes()).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to parse Rd for Markdown conversion: {}",
            e
        )))
    })?;
    let options = rd_convert_options();
    Ok(rd2qmd_core::convert_rd_document(
        parsed.document(),
        &options,
    ))
}

/// Sentinel value returned by R when a vignette is in PDF format.
const PDF_VIGNETTE_SENTINEL: &str = "__PDF_VIGNETTE__";

/// Get vignette content as Markdown text.
///
/// This retrieves a vignette's HTML content via `utils::vignette()` and
/// converts it to Markdown using htmd. PDF vignettes cannot be displayed
/// in the terminal and will return an error with a descriptive message.
///
/// # Arguments
///
/// * `topic` - The vignette topic name
/// * `package` - The package name containing the vignette
///
/// # Returns
///
/// The vignette content as Markdown text, or an error if unavailable.
pub fn get_vignette_text(topic: &str, package: &str) -> HarpResult<String> {
    let code = format!(
        r#"local({{
    v <- tryCatch(
        utils::vignette("{topic}", package = "{pkg}"),
        error = function(e) NULL
    )
    if (is.null(v)) return(NULL)
    if (nchar(v$PDF) == 0) return(NULL)
    file <- file.path(v$Dir, "doc", v$PDF)
    if (!file.exists(file)) return(NULL)
    ext <- tolower(tools::file_ext(file))
    if (ext == "pdf") return("{sentinel}")
    paste(readLines(file, warn = FALSE), collapse = "\n")
}})"#,
        topic = escape_r_string(topic),
        pkg = escape_r_string(package),
        sentinel = escape_r_string(PDF_VIGNETTE_SENTINEL),
    );

    let html = unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "Vignette '{}' not found in package '{}'",
                topic, package
            )))
        })?
    };

    if html == PDF_VIGNETTE_SENTINEL {
        return Err(HarpError::RError(arf_libr::RError::EvalError(format!(
            r#"Vignette '{topic}' in package '{package}' is a PDF and cannot be displayed in the terminal.
Run in R: vignette("{topic}", package = "{package}")"#,
        ))));
    }

    r_vignette_to_md::convert(&html).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to convert vignette HTML: {}",
            e
        )))
    })
}

/// Show help for a specific topic (legacy function).
///
/// This calls `get_help_text()` and prints the result to stdout.
/// For better control, use `get_help_text()` directly.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
pub fn show_help(topic: &str, package: Option<&str>) -> HarpResult<()> {
    let text = get_help_text(topic, package)?;
    println!("{}", text);
    Ok(())
}

/// Escape a string for use in R code.
fn escape_r_string(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_topic_qualified_name() {
        let topic = HelpTopic {
            package: "base".to_string(),
            topic: "print".to_string(),
            aliases: vec![],
            help_key: None,
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        };

        assert_eq!(topic.qualified_name(), "base::print");
    }

    #[test]
    fn test_escape_r_string() {
        assert_eq!(escape_r_string("hello"), "hello");
        assert_eq!(escape_r_string(r#"he"llo"#), r#"he\"llo"#);
        assert_eq!(escape_r_string("he\\llo"), "he\\\\llo");
    }

    #[test]
    fn test_rd_conversion_strips_if_html_content() {
        // Regression test: `\if{html}{\out{...}}` blocks (e.g. asciicast
        // recordings) must not leak raw HTML into terminal help output.
        // Snapshotting the full output (rather than asserting individual
        // substrings) ensures any leaked tag, attribute, or inner text
        // shows up as a diff.
        let rd_content = r#"
\name{hello}
\title{Hello World}
\description{A simple function.}
\details{
Some details.
\if{html}{\out{<div class="asciicast"><span style="color: red;">colored</span></div>}}
More text after.
}
"#;

        let parsed = rd_source::parse(rd_content.as_bytes()).unwrap();
        let options = rd_convert_options();
        let qmd = rd2qmd_core::convert_rd_document(parsed.document(), &options);

        insta::assert_snapshot!("rd_conversion_strips_if_html_content", qmd);
    }

    #[test]
    fn test_rd_conversion_arguments_preserves_blocks() {
        let rd_content = r#"
\name{arguments}
\title{Arguments}
\arguments{
    \item{value}{
        Description before the list.
        \itemize{
            \item First nested item.
            \item Second nested item.
        }

        A second paragraph after the list.
    }
}
"#;

        let parsed = rd_source::parse(rd_content.as_bytes()).unwrap();
        let options = rd_convert_options();
        let qmd = rd2qmd_core::convert_rd_document(parsed.document(), &options);

        insta::assert_snapshot!("rd_conversion_arguments_preserves_blocks", qmd);
    }
}
