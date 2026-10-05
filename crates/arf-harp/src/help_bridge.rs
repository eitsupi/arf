//! Bridge normalized R `help()` results into prepared help requests.

use crate::error::{HarpError, HarpResult};
use crate::help::get_package_help_markdown_by_key_in_dir;
use arf_libr::{R_CallMethodDef, R_FALSE, R_TRUE, SEXP, SexpType, r_library};
use std::collections::VecDeque;
use std::ffi::{CStr, c_void};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// Maximum number of pending help requests awaiting the UI consumer.
pub const MAX_PENDING_HELP_REQUESTS: usize = 16;

/// One prepared help page with its exact installed package directory and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedHelpPage {
    pub package_dir: PathBuf,
    pub package: String,
    pub display_topic: String,
    pub help_key: String,
    pub markdown: String,
}

/// A request containing all pages resolved by one R `help()` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedHelpRequest {
    pub topic: String,
    pub pages: Vec<PreparedHelpPage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelpCandidate {
    package_dir: PathBuf,
    package: String,
    help_key: String,
}

struct HelpRequestQueue {
    requests: VecDeque<PreparedHelpRequest>,
}

impl HelpRequestQueue {
    const fn new() -> Self {
        Self {
            requests: VecDeque::new(),
        }
    }

    fn enqueue(&mut self, request: PreparedHelpRequest) -> bool {
        if self.requests.len() >= MAX_PENDING_HELP_REQUESTS {
            return false;
        }
        self.requests.push_back(request);
        true
    }

    fn take(&mut self) -> Option<PreparedHelpRequest> {
        self.requests.pop_front()
    }

    fn drain(&mut self) -> Vec<PreparedHelpRequest> {
        self.requests.drain(..).collect()
    }
}

static PENDING_HELP_REQUESTS: Mutex<HelpRequestQueue> = Mutex::new(HelpRequestQueue::new());

/// Result of attempting to install the interactive R help wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpSubmitInstallOutcome {
    /// The standard utils method was active and the wrapper was installed.
    Installed,
    /// A custom or already-installed method was active, so the registry was left alone.
    SkippedExistingMethod,
}

/// Take the oldest prepared help request, if one is pending.
pub fn take_prepared_help_request() -> Option<PreparedHelpRequest> {
    PENDING_HELP_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Drain all pending prepared requests in FIFO order.
pub fn drain_prepared_help_requests() -> Vec<PreparedHelpRequest> {
    PENDING_HELP_REQUESTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
}

/// Provide the help callback definition to the complete embedding registry.
pub(crate) fn help_submit_definition() -> R_CallMethodDef {
    R_CallMethodDef {
        name: c"arf_submit_help_request".as_ptr(),
        // SAFETY: R stores all native entries as the generic DL_FUNC ABI,
        // while `.Call` invokes this callback with the registered arity.
        fun: Some(unsafe {
            std::mem::transmute::<
                unsafe extern "C" fn(SEXP, SEXP, SEXP, SEXP) -> SEXP,
                unsafe extern "C" fn() -> *mut c_void,
            >(arf_submit_help_request)
        }),
        num_args: 4,
    }
}

/// Install the `.Call` routine and, only when utils' registered S3 method is
/// identical to its standard method, wrap that method to submit prepared help.
///
/// This is intended for an initialized interactive embedded-R session. It does
/// not replace custom S3 methods and is safe to call repeatedly.
pub fn install_help_submit_wrapper() -> HarpResult<HelpSubmitInstallOutcome> {
    let eligibility = crate::eval_string_in_base(HELP_SUBMIT_ELIGIBILITY_R)?;
    match read_scalar_status(&eligibility)? {
        1 => {}
        2 => return Ok(HelpSubmitInstallOutcome::SkippedExistingMethod),
        status => {
            return Err(HarpError::TypeMismatch {
                expected: "scalar installer status 1 or 2".to_string(),
                actual: format!("installer status {status}"),
            });
        }
    }

    // Register before mutating the S3 registry so a missing DllInfo or native
    // registration failure leaves standard R help behavior untouched.
    crate::routines::register_embedding_routines()?;

    let installed = crate::eval_string_in_base(HELP_SUBMIT_INSTALL_R)?;
    match read_scalar_status(&installed)? {
        1 => Ok(HelpSubmitInstallOutcome::Installed),
        2 => Ok(HelpSubmitInstallOutcome::SkippedExistingMethod),
        status => Err(HarpError::TypeMismatch {
            expected: "scalar installer status 1 or 2".to_string(),
            actual: format!("installer status {status}"),
        }),
    }
}

fn read_scalar_status(result: &crate::RObject) -> HarpResult<i32> {
    if result.sexp_type()? != SexpType::IntSxp {
        return Err(HarpError::TypeMismatch {
            expected: "one non-NA integer installer status".to_string(),
            actual: format!("{:?}", result.sexp_type()?),
        });
    }
    let lib = r_library()?;
    if unsafe { (lib.rf_length)(result.sexp()) } != 1 {
        return Err(HarpError::TypeMismatch {
            expected: "one non-NA integer installer status".to_string(),
            actual: "integer vector with length other than one".to_string(),
        });
    }
    let value = unsafe { (lib.integer)(result.sexp()) };
    if value.is_null() {
        return Err(HarpError::TypeMismatch {
            expected: "one non-NA integer installer status".to_string(),
            actual: "null integer storage".to_string(),
        });
    }
    let status = unsafe { *value };
    if status == i32::MIN {
        return Err(HarpError::TypeMismatch {
            expected: "one non-NA integer installer status".to_string(),
            actual: "NA integer installer status".to_string(),
        });
    }
    Ok(status)
}

const HELP_SUBMIT_ELIGIBILITY_R: &str = r#"
invisible((function() {
  ns <- getNamespace("utils")
  standard <- get("print.help_files_with_topic", envir = ns, inherits = FALSE)
  get_method <- get("getS3method", envir = ns, inherits = FALSE)
  registered <- get_method("print", "help_files_with_topic", optional = TRUE)
  if (identical(registered, standard)) 1L else 2L
})())
"#;

const HELP_SUBMIT_INSTALL_R: &str = r#"
invisible((function() {
  ns <- getNamespace("utils")
  standard <- get("print.help_files_with_topic", envir = ns, inherits = FALSE)
  get_method <- get("getS3method", envir = ns, inherits = FALSE)
  registered <- get_method("print", "help_files_with_topic", optional = TRUE)
  if (!identical(registered, standard)) return(2L)

  fallback <- standard
  wrapper <- function(x, ...) {
    accepted <- tryCatch({
      paths <- enc2utf8(as.character(x))
      topic <- enc2utf8(attr(x, "topic"))
      help_type <- enc2utf8(attr(x, "type"))
      tried_all_packages <- attr(x, "tried_all_packages")
      isTRUE(.Call(
        "arf_submit_help_request", paths, topic, help_type,
        tried_all_packages, PACKAGE = "(embedding)"
      ))
    }, error = function(error) FALSE)

    if (isTRUE(accepted)) invisible(x) else fallback(x, ...)
  }
  registerS3method(
    "print", "help_files_with_topic", wrapper, envir = ns
  )
  1L
})())
"#;

unsafe extern "C" fn arf_submit_help_request(
    paths_utf8: SEXP,
    topic_utf8: SEXP,
    help_type: SEXP,
    tried_all_packages: SEXP,
) -> SEXP {
    // Keep every Rust operation, including queue locking and R scalar creation,
    // inside the unwind boundary. R's C API itself must not unwind through Rust.
    contain_callback_panic(
        || unsafe {
            submit_and_return_logical(paths_utf8, topic_utf8, help_type, tried_all_packages)
        },
        rejected_logical_or_null,
    )
}

fn contain_callback_panic<T>(operation: impl FnOnce() -> T, fallback: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(_) => fallback(),
    }
}

fn rejected_logical_or_null() -> SEXP {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(lib) = r_library() else {
            return std::ptr::null_mut();
        };
        unsafe { (lib.rf_scalarlogical)(R_FALSE) }
    }))
    .unwrap_or(std::ptr::null_mut())
}

unsafe fn submit_and_return_logical(
    paths_utf8: SEXP,
    topic_utf8: SEXP,
    help_type: SEXP,
    tried_all_packages: SEXP,
) -> SEXP {
    let Ok(lib) = r_library() else {
        return std::ptr::null_mut();
    };

    let prepared =
        unsafe { prepare_request_from_r(paths_utf8, topic_utf8, help_type, tried_all_packages) };
    let accepted = match prepared {
        Some(request) => PENDING_HELP_REQUESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .enqueue(request),
        None => false,
    };

    // This is the final operation after enqueueing, so no Rust panic can make
    // the callback report rejection after changing the queue.
    unsafe { (lib.rf_scalarlogical)(if accepted { R_TRUE } else { R_FALSE }) }
}

unsafe fn prepare_request_from_r(
    paths_utf8: SEXP,
    topic_utf8: SEXP,
    help_type: SEXP,
    tried_all_packages: SEXP,
) -> Option<PreparedHelpRequest> {
    let lib = r_library().ok()?;
    let paths = unsafe { copy_character_vector(lib, paths_utf8)? };
    if paths.is_empty() || paths.iter().any(String::is_empty) {
        return None;
    }

    let topic = unsafe { copy_scalar_character(lib, topic_utf8)? };
    if topic.is_empty() {
        return None;
    }
    let help_type = unsafe { copy_scalar_character(lib, help_type)? };
    if help_type != "text" {
        return None;
    }
    if unsafe { !is_scalar_false_logical(lib, tried_all_packages) } {
        return None;
    }

    let candidates = paths
        .iter()
        .map(|path| candidate_from_path(path))
        .collect::<Option<Vec<_>>>()?;
    prepare_request_with(topic, candidates, |candidate, display_topic| {
        get_package_help_markdown_by_key_in_dir(
            &candidate.package_dir,
            display_topic,
            &candidate.help_key,
            &candidate.package,
        )
        .ok()
    })
}

fn candidate_from_path(path: &str) -> Option<HelpCandidate> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return None;
    }

    let help_dir = path.parent()?;
    if help_dir.file_name()? != "help" {
        return None;
    }
    let package_dir = help_dir.parent()?.to_path_buf();
    let package = package_dir.file_name()?.to_str()?.to_owned();
    let help_key = path.file_name()?.to_str()?.to_owned();
    if package.is_empty() || help_key.is_empty() {
        return None;
    }

    Some(HelpCandidate {
        package_dir,
        package,
        help_key,
    })
}

fn prepare_request_with<F>(
    topic: String,
    candidates: Vec<HelpCandidate>,
    mut load_markdown: F,
) -> Option<PreparedHelpRequest>
where
    F: FnMut(&HelpCandidate, &str) -> Option<String>,
{
    if candidates.is_empty() {
        return None;
    }

    let mut pages = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let markdown = load_markdown(&candidate, &topic)?;
        pages.push(PreparedHelpPage {
            package_dir: candidate.package_dir,
            package: candidate.package,
            display_topic: topic.clone(),
            help_key: candidate.help_key,
            markdown,
        });
    }

    Some(PreparedHelpRequest { topic, pages })
}

unsafe fn copy_character_vector(lib: &arf_libr::RLibrary, value: SEXP) -> Option<Vec<String>> {
    if value.is_null() || unsafe { (lib.rf_typeof)(value) } != SexpType::StrSxp as i32 {
        return None;
    }
    let length = unsafe { (lib.rf_length)(value) };
    if length <= 0 {
        return None;
    }

    let na_string = unsafe { *lib.r_na_string };
    let mut strings = Vec::with_capacity(length as usize);
    for index in 0..length as isize {
        let element = unsafe { (lib.string_elt)(value, index) };
        if element.is_null() || element == na_string {
            return None;
        }
        strings.push(unsafe { copy_utf8_chars(lib, element)? });
    }
    Some(strings)
}

unsafe fn copy_scalar_character(lib: &arf_libr::RLibrary, value: SEXP) -> Option<String> {
    if value.is_null()
        || unsafe { (lib.rf_typeof)(value) } != SexpType::StrSxp as i32
        || unsafe { (lib.rf_length)(value) } != 1
    {
        return None;
    }
    let element = unsafe { (lib.string_elt)(value, 0) };
    if element.is_null() || element == unsafe { *lib.r_na_string } {
        return None;
    }
    unsafe { copy_utf8_chars(lib, element) }
}

unsafe fn copy_utf8_chars(lib: &arf_libr::RLibrary, charsxp: SEXP) -> Option<String> {
    let chars = unsafe { (lib.r_charsxp)(charsxp) };
    if chars.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(chars) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

unsafe fn is_scalar_false_logical(lib: &arf_libr::RLibrary, value: SEXP) -> bool {
    if value.is_null()
        || unsafe { (lib.rf_typeof)(value) } != SexpType::LglSxp as i32
        || unsafe { (lib.rf_length)(value) } != 1
    {
        return false;
    }
    let logicals = unsafe { (lib.logical)(value) };
    !logicals.is_null() && unsafe { *logicals } == R_FALSE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(number: usize) -> PreparedHelpRequest {
        PreparedHelpRequest {
            topic: format!("topic-{number}"),
            pages: vec![PreparedHelpPage {
                package_dir: PathBuf::from("/library/pkg"),
                package: "pkg".to_string(),
                display_topic: format!("topic-{number}"),
                help_key: format!("key-{number}"),
                markdown: format!("page-{number}"),
            }],
        }
    }

    #[test]
    fn queue_takes_and_drains_in_fifo_order() {
        let mut queue = HelpRequestQueue::new();
        assert!(queue.enqueue(request(1)));
        assert!(queue.enqueue(request(2)));
        assert_eq!(queue.take(), Some(request(1)));
        assert_eq!(queue.drain(), vec![request(2)]);
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn full_queue_rejects_without_changing_queued_requests() {
        let mut queue = HelpRequestQueue::new();
        for number in 0..MAX_PENDING_HELP_REQUESTS {
            assert!(queue.enqueue(request(number)));
        }
        let before = queue.requests.iter().cloned().collect::<Vec<_>>();
        assert!(!queue.enqueue(request(MAX_PENDING_HELP_REQUESTS)));
        assert_eq!(queue.requests.iter().cloned().collect::<Vec<_>>(), before);
    }

    #[test]
    fn malformed_help_paths_are_rejected_lexically() {
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let package_dir = temp_dir.path().join("library").join("pkg");
        let valid_path = package_dir.join("help").join("mean");
        assert_eq!(
            candidate_from_path(valid_path.to_str().unwrap()),
            Some(HelpCandidate {
                package_dir: package_dir.clone(),
                package: "pkg".to_string(),
                help_key: "mean".to_string(),
            })
        );
        assert!(
            candidate_from_path(package_dir.join("doc").join("mean").to_str().unwrap()).is_none()
        );
        assert!(
            candidate_from_path(
                package_dir
                    .join("help")
                    .join("..")
                    .join("mean")
                    .to_str()
                    .unwrap()
            )
            .is_none()
        );
        assert!(candidate_from_path(package_dir.join("help").to_str().unwrap()).is_none());
        assert!(candidate_from_path("library/pkg/help/mean").is_none());
    }

    #[test]
    fn failed_candidate_preparation_produces_no_partial_request() {
        let mut queue = HelpRequestQueue::new();
        assert!(queue.enqueue(request(7)));
        let queued_before = queue.requests.iter().cloned().collect::<Vec<_>>();
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let package_dir = temp_dir.path().join("library").join("pkg");
        let candidates = vec![
            candidate_from_path(package_dir.join("help").join("mean").to_str().unwrap()).unwrap(),
            candidate_from_path(package_dir.join("help").join("missing").to_str().unwrap())
                .unwrap(),
        ];
        let mut attempts = 0;
        let prepared = prepare_request_with("mean".to_string(), candidates, |candidate, _| {
            attempts += 1;
            (candidate.help_key == "mean").then(|| "prepared page".to_string())
        });
        assert_eq!(attempts, 2);
        assert!(prepared.is_none());
        assert_eq!(
            queue.requests.iter().cloned().collect::<Vec<_>>(),
            queued_before
        );
    }

    #[test]
    fn panicking_candidate_preparation_is_contained_before_queue_mutation() {
        let queue = HelpRequestQueue::new();
        let temp_dir = tempfile::tempdir().expect("temporary directory should be created");
        let package_dir = temp_dir.path().join("library").join("pkg");
        let candidates = vec![
            candidate_from_path(package_dir.join("help").join("mean").to_str().unwrap()).unwrap(),
        ];
        let accepted = contain_callback_panic(
            || {
                let _prepared = prepare_request_with("mean".to_string(), candidates, |_, _| {
                    panic!("simulated help preparation panic")
                });
                true
            },
            || false,
        );
        assert!(!accepted);
        assert!(queue.requests.is_empty());
    }
}
