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
use crate::lib_paths::installed_package_dirs;
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use rd_helpdb::{
    DemoIndex, HelpTopicEntry, HelpTopicIndex, PackageHelpDb, VignetteIndex, read_rds_file,
};
use std::ffi::CString;

mod target;
pub use target::HelpTarget;
mod resolver;
pub use resolver::{HelpResolution, HelpTargetResolver};

/// A help topic from R's help database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpTopic {
    /// Package name containing this help topic.
    pub package: String,
    /// Installed package directory that supplied this topic's metadata.
    pub package_dir: std::path::PathBuf,
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

/// Get help, vignette, and demo topics from explicit library paths.
///
/// This function only reads the supplied filesystem paths and metadata; it
/// does not evaluate R. The paths are searched in order, with the first
/// installed copy of a package taking precedence. Missing or unreadable
/// metadata sources are skipped.
///
/// Rd topic metadata, vignette indexes, and demo indexes are read independently.
pub fn get_help_topics_from_paths(paths: &[String]) -> Vec<HelpTopic> {
    let mut topics = Vec::new();
    for (package, package_dir) in installed_package_dirs(paths) {
        topics.extend(read_package_topics(&package, &package_dir));
    }
    topics
}

fn read_package_topics(package: &str, package_dir: &std::path::Path) -> Vec<HelpTopic> {
    let mut topics = Vec::new();

    // Each metadata source is independent of compiled help files and of the
    // other metadata indexes. A malformed source should not hide the rest.
    match HelpTopicIndex::read_installed(package_dir) {
        Ok(Some(index)) => topics.extend(
            index
                .entries()
                .filter_map(|entry| project_help_topic_entry(package, package_dir, entry)),
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
                package_dir: package_dir.to_owned(),
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
                package_dir: package_dir.to_owned(),
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

fn project_help_topic_entry(
    package: &str,
    package_dir: &std::path::Path,
    entry: &HelpTopicEntry,
) -> Option<HelpTopic> {
    let aliases: Vec<String> = entry.aliases.iter().flatten().cloned().collect();
    let topic = entry
        .name
        .as_str()
        .filter(|name| aliases.iter().any(|alias| alias == name))
        .or_else(|| entry.aliases.first().and_then(Option::as_deref))?;

    Some(HelpTopic {
        package: package.to_owned(),
        package_dir: package_dir.to_owned(),
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
        HarpError, get_help_topics_from_paths, get_package_help_markdown_by_key_in_dir,
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

    fn compiled_help_package() -> (tempfile::TempDir, std::path::PathBuf) {
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
        (temp, package_dir)
    }

    fn install_metadata_fixture(library: &std::path::Path, package: &str) -> std::path::PathBuf {
        let package_dir = library.join(package);
        let metadata = package_dir.join("Meta");
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::write(metadata.join("package.rds"), []).unwrap();
        std::fs::copy(
            fixture_path("help_topics_metadata_v3.rds"),
            metadata.join("Rd.rds"),
        )
        .unwrap();
        package_dir
    }

    #[test]
    fn explicit_paths_list_topics_without_r() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let package_dir = install_metadata_fixture(&library, "fixturepkg");

        let paths = vec![library.to_string_lossy().into_owned()];
        let topics = get_help_topics_from_paths(&paths);
        assert!(topics.iter().any(|topic| {
            topic.package == "fixturepkg"
                && topic.package_dir == package_dir
                && topic.topic == "first"
        }));
    }

    #[test]
    fn explicit_help_paths_keep_first_installed_copy_order() {
        let temp = tempfile::tempdir().unwrap();
        let first_library = temp.path().join("first");
        let second_library = temp.path().join("second");
        let first_package = install_metadata_fixture(&first_library, "fixturepkg");
        let second_package = install_metadata_fixture(&second_library, "fixturepkg");
        let paths = vec![
            first_library.to_string_lossy().into_owned(),
            second_library.to_string_lossy().into_owned(),
        ];

        let topics = get_help_topics_from_paths(&paths);
        assert!(!topics.is_empty(), "fixture should contain help metadata");
        assert!(
            topics
                .iter()
                .all(|topic| topic.package_dir == first_package)
        );

        let reversed_topics =
            get_help_topics_from_paths(&paths.into_iter().rev().collect::<Vec<_>>());
        assert!(
            !reversed_topics.is_empty(),
            "fixture should contain help metadata"
        );
        assert!(
            reversed_topics
                .iter()
                .all(|topic| topic.package_dir == second_package)
        );
    }

    #[test]
    fn help_entry_projection_prefers_matching_name_and_keeps_all_aliases() {
        let index = topic_index();
        let topic = project_help_topic_entry(
            "fixture",
            std::path::Path::new("/fixture"),
            index.entries().next().unwrap(),
        )
        .expect("first metadata row should be visible");

        assert_eq!(topic.topic, "first");
        assert_eq!(topic.aliases, ["shared", "first", "first"]);
        assert_eq!(topic.help_key.as_deref(), Some("first-topic"));
        assert_eq!(topic.title, "First topic title");
        assert_eq!(topic.package_dir, std::path::Path::new("/fixture"));
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
        let topic =
            project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entry).unwrap();
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

        assert!(
            project_help_topic_entry(
                "fixture",
                std::path::Path::new("/fixture"),
                index.entries().next().unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn help_entry_projection_uses_empty_title_for_missing_or_na_and_skips_empty_alias_groups() {
        let index = topic_index();
        let entries: Vec<_> = index.entries().collect();
        let topic =
            project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entries[1])
                .unwrap();
        assert_eq!(topic.title, "");
        assert_eq!(
            project_help_topic_entry("fixture", std::path::Path::new("/fixture"), entries[3]),
            None
        );

        let missing_title =
            rd_helpdb::read_rds_file(fixture_path("help_topics_aliases_only_v3.rds")).unwrap();
        let missing_index = rd_helpdb::HelpTopicIndex::from_object(&missing_title).unwrap();
        assert_eq!(
            project_help_topic_entry(
                "fixture",
                std::path::Path::new("/fixture"),
                missing_index.entries().next().unwrap()
            )
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
        let (_temp, package_dir) = compiled_help_package();
        assert!(!package_dir.join("help/aliases.rds").exists());

        let markdown = get_package_help_markdown_by_key_in_dir(
            &package_dir,
            "display-only-name",
            "first-topic",
            "fixturepkg",
        )
        .expect("direct known-key lookup should not need aliases.rds");

        assert!(!markdown.is_empty());
    }

    #[test]
    fn known_help_key_missing_from_compiled_help_reports_unknown_topic() {
        let (_temp, package_dir) = compiled_help_package();

        let error = get_package_help_markdown_by_key_in_dir(
            &package_dir,
            "display-only-name",
            "second-topic",
            "fixturepkg",
        )
        .expect_err("a key absent from the compiled help database should fail");

        assert!(matches!(
            error,
            HarpError::HelpDatabase { key, source, .. }
                if key == "second-topic"
                    && matches!(*source, rd_helpdb::Error::UnknownTopic { ref topic } if topic == "second-topic")
        ));
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

fn rd_convert_options() -> rd2qmd_core::RdConvertOptions {
    let mut options = rd2qmd_core::RdConvertOptions::default();
    options.code.quarto_code_blocks = false;
    options.arguments_format = rd2qmd_core::ArgumentsFormat::List;
    options.describe_format = rd2qmd_core::DescribeFormat::Headings;
    options.links = target::rd_link_options();
    options
}

/// Convert Rd source to terminal help Markdown without evaluating R.
///
/// Cross-references use arf's internal help URI format, interpreted by
/// [`HelpTarget::from_uri`]. Rust consumers of Rd source use the same conversion
/// policy as the R fallback help path.
pub fn rd_source_to_markdown(rd_content: &str) -> HarpResult<String> {
    let parsed = rd_source::parse(rd_content.as_bytes()).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to parse Rd for Markdown conversion: {}",
            e
        )))
    })?;
    Ok(rd2qmd_core::convert_rd_document(
        parsed.document(),
        &rd_convert_options(),
    ))
}

/// Get package help as Markdown using a known compiled-help key in a specific
/// installed package directory.
///
/// This function never reads `help/aliases.rds`. Callers that already have the
/// package directory avoid selecting another installed copy of the same
/// package.
pub fn get_package_help_markdown_by_key_in_dir(
    package_dir: &std::path::Path,
    display_topic: &str,
    help_key: &str,
    package: &str,
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
