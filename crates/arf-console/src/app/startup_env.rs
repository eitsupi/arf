//! Startup environment capture and runtime R_HOME resolution.

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::OnceLock;

const STARTUP_ENV_VARS: &[&str] = &[
    "R_HOME",
    "LD_LIBRARY_PATH",
    "R_LIBS_USER",
    "R_LIBS_SITE",
    "R_LIBS",
    "R_DOC_DIR",
    "R_SHARE_DIR",
    "R_INCLUDE_DIR",
    "R_SYSTEM_ABI",
];
/// Carries the startup snapshot across a restart or the `ensure_ld_library_path` re-exec.
///
/// The leading underscore marks it as arf's own and keeps it clear of ordinary
/// names; it does not stop anyone from setting it. What guards the snapshot is
/// that the value is validated against a fixed set of names and a schema
/// version, and that arf drops the variable from its environment as soon as it
/// has read it. Supplying one buys nothing anyway: the variables it carries can
/// be set directly.
pub(crate) const STARTUP_ENV_CARRIER: &str = "_ARF_INTERNAL_STARTUP_ENV";
const STARTUP_ENV_CARRIER_VERSION: u32 = 1;
static STARTUP_ENV: OnceLock<HashMap<String, OsString>> = OnceLock::new();

/// Read the startup environment snapshot before R initialization can add any
/// R-specific variables, using the carrier forwarded through a prior re-exec
/// when present and otherwise capturing the current environment.
pub(crate) fn capture_startup_env() {
    crate::pid_file::capture_restart_context();
    // SAFETY: run() calls this at the start of process initialization, before
    // arf starts any threads or initializes R, so changing the process
    // environment is safe here.
    let carrier = unsafe {
        let carrier = std::env::var_os(STARTUP_ENV_CARRIER);
        std::env::remove_var(STARTUP_ENV_CARRIER);
        carrier
    };

    let snapshot = match carrier {
        Some(carrier) => match carrier.to_str() {
            Some(serialized) => match deserialize_startup_env(serialized) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    eprintln!(
                        "Warning: Ignoring invalid {STARTUP_ENV_CARRIER} carrier: {error}; capturing the current environment."
                    );
                    capture_current_startup_env()
                }
            },
            None => {
                eprintln!(
                    "Warning: Ignoring invalid {STARTUP_ENV_CARRIER} carrier: it is not valid UTF-8; capturing the current environment."
                );
                capture_current_startup_env()
            }
        },
        None => capture_current_startup_env(),
    };
    let _ = STARTUP_ENV.set(snapshot);
}

fn capture_current_startup_env() -> HashMap<String, OsString> {
    STARTUP_ENV_VARS
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_string(), value)))
        .collect()
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartupEnvCarrier {
    version: u32,
    variables: BTreeMap<String, String>,
}

/// Serialize the startup environment using a versioned, platform-safe schema.
fn serialize_startup_env(
    snapshot: &HashMap<String, OsString>,
) -> Result<String, serde_json::Error> {
    let variables = STARTUP_ENV_VARS
        .iter()
        .filter_map(|name| {
            snapshot
                .get(*name)
                .map(|value| ((*name).to_string(), encode_os_string(value)))
        })
        .collect();

    serde_json::to_string(&StartupEnvCarrier {
        version: STARTUP_ENV_CARRIER_VERSION,
        variables,
    })
}

/// Deserialize a startup environment carrier, ignoring variables outside the whitelist.
fn deserialize_startup_env(serialized: &str) -> Result<HashMap<String, OsString>, String> {
    let carrier: StartupEnvCarrier =
        serde_json::from_str(serialized).map_err(|error| format!("invalid JSON: {error}"))?;
    if carrier.version != STARTUP_ENV_CARRIER_VERSION {
        return Err(format!("unsupported version {}", carrier.version));
    }

    carrier
        .variables
        .into_iter()
        .filter(|(name, _)| STARTUP_ENV_VARS.contains(&name.as_str()))
        .map(|(name, value)| {
            let decoded = decode_os_string(&value)
                .map_err(|error| format!("invalid value for {name}: {error}"))?;
            Ok((name, decoded))
        })
        .collect()
}

fn encode_os_string(value: &OsStr) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().to_vec()
    };

    #[cfg(windows)]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        value
            .encode_wide()
            .flat_map(u16::to_ne_bytes)
            .collect::<Vec<_>>()
    };

    #[cfg(not(any(unix, windows)))]
    let bytes = value.to_string_lossy().as_bytes().to_vec();

    general_purpose::STANDARD.encode(bytes)
}

fn decode_os_string(encoded: &str) -> Result<OsString, String> {
    let bytes = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes))
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err("decoded value has an odd number of bytes".to_string());
        }
        let wide = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_ne_bytes(*chunk))
            .collect::<Vec<_>>();
        Ok(OsString::from_wide(&wide))
    }

    #[cfg(not(any(unix, windows)))]
    Ok(OsString::from(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn startup_env_carrier() -> String {
    serialize_startup_env(
        STARTUP_ENV
            .get()
            .expect("startup environment must be captured before restarting"),
    )
    .expect("startup environment carrier serialization cannot fail")
}

/// Return the value of a variable from the environment inherited at startup.
pub(crate) fn startup_env_value(name: &str) -> Option<OsString> {
    STARTUP_ENV.get()?.get(name).cloned()
}

/// Capture the R_HOME reported by the initialized R runtime.
///
/// This is intentionally called only after successful R initialization. The
/// startup resolution report is a prediction, while `R.home()` is a fact from
/// the runtime that is actually in use.
///
/// `base_dir` is the working directory from before initialization, used to
/// resolve a relative R_HOME. See [`absolutize_runtime_r_home`].
pub(crate) fn capture_runtime_r_home(base_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    let r_home = match unsafe { arf_harp::eval_r_to_string(r#"base::R.home()"#) } {
        Ok(Some(r_home)) => r_home,
        Ok(None) => {
            log::warn!("R.home() returned NULL after R initialization");
            return None;
        }
        Err(error) => {
            log::warn!("Could not evaluate R.home() after R initialization: {error}");
            return None;
        }
    };

    absolutize_runtime_r_home(PathBuf::from(r_home), base_dir)
}

/// Resolve a relative R_HOME against the directory R started in.
///
/// R stores whatever R_HOME string it was given, so `R.home()` can be
/// relative. The base has to be the working directory from before
/// initialization: a `.Rprofile` runs during initialization on Unix and may
/// call `setwd()`, and resolving against the directory afterwards would name a
/// different location than the one R loaded from. Without a base, a relative
/// path is dropped rather than guessed at.
///
/// Paths are joined, never canonicalized: resolving symlinks would report an
/// R_HOME other than the one in use. `arf r resolve` joins for the same reason.
///
/// In practice R fails to initialize with a relative R_HOME, so this is
/// defensive. It is kept because arf does not reject relative input either: an
/// explicit `--r-home` is returned unchanged when the path has no `bin/R`.
fn absolutize_runtime_r_home(
    r_home: PathBuf,
    base_dir: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if r_home.is_absolute() {
        return Some(r_home);
    }
    match base_dir {
        Some(base) => Some(base.join(r_home)),
        None => {
            log::warn!(
                "Could not make R.home() absolute: the directory R started in is unknown ({})",
                r_home.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        STARTUP_ENV_CARRIER_VERSION, absolutize_runtime_r_home, deserialize_startup_env,
        serialize_startup_env,
    };
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    #[test]
    fn absolute_runtime_r_home_is_left_alone() {
        let r_home = PathBuf::from(if cfg!(windows) {
            r"C:\opt\R\lib\R"
        } else {
            r"/opt/R/lib/R"
        });
        assert_eq!(
            absolutize_runtime_r_home(r_home.clone(), Some(Path::new("/elsewhere"))),
            Some(r_home)
        );
    }

    #[test]
    fn relative_runtime_r_home_resolves_against_the_startup_directory() {
        assert_eq!(
            absolutize_runtime_r_home(PathBuf::from("lib/R"), Some(Path::new("/start"))),
            Some(PathBuf::from("/start/lib/R"))
        );
    }

    #[test]
    fn relative_runtime_r_home_without_a_base_is_dropped() {
        assert_eq!(
            absolutize_runtime_r_home(PathBuf::from("lib/R"), None),
            None
        );
    }

    #[test]
    fn startup_env_round_trip_preserves_presence_and_empty_values() {
        let snapshot = HashMap::from([
            ("R_LIBS_USER".to_string(), OsString::from("/user/library")),
            ("R_LIBS".to_string(), OsString::new()),
        ]);

        let serialized = serialize_startup_env(&snapshot).expect("serialization should succeed");
        let restored =
            deserialize_startup_env(&serialized).expect("deserialization should succeed");

        assert_eq!(
            restored.get("R_LIBS_USER"),
            Some(&OsString::from("/user/library"))
        );
        assert_eq!(restored.get("R_LIBS"), Some(&OsString::new()));
        assert!(!restored.contains_key("R_LIBS_SITE"));
    }

    #[cfg(unix)]
    #[test]
    fn startup_env_round_trip_preserves_non_utf8_values() {
        use std::os::unix::ffi::OsStringExt;

        let value = OsString::from_vec(vec![b'/', 0xff, b'R']);
        let snapshot = HashMap::from([("R_LIBS".to_string(), value.clone())]);

        let serialized = serialize_startup_env(&snapshot).expect("serialization should succeed");
        let restored =
            deserialize_startup_env(&serialized).expect("deserialization should succeed");

        assert_eq!(restored.get("R_LIBS"), Some(&value));
    }

    #[test]
    fn startup_env_deserialization_rejects_invalid_json() {
        assert!(deserialize_startup_env(r#"not JSON"#).is_err());
    }

    #[test]
    fn startup_env_deserialization_rejects_unknown_version() {
        let serialized = format!(
            r#"{{"version":{},"variables":{{}}}}"#,
            STARTUP_ENV_CARRIER_VERSION + 1
        );

        assert!(deserialize_startup_env(&serialized).is_err());
    }

    #[test]
    fn startup_env_deserialization_ignores_non_whitelisted_variables() {
        let serialized = r#"{"version":1,"variables":{"NOT_ALLOWED":"aGVsbG8=","R_LIBS":""}}"#;

        let restored = deserialize_startup_env(serialized).expect("unknown names are ignored");

        assert_eq!(restored.get("R_LIBS"), Some(&OsString::new()));
        assert!(!restored.contains_key("NOT_ALLOWED"));
    }
}
