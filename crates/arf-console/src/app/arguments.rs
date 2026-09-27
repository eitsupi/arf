//! Interactive argument normalization performed before R can change cwd.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

static NORMALIZED_ARGS: OnceLock<Vec<OsString>> = OnceLock::new();

/// Resolve an IPC bind path while the process still has its original cwd.
#[cfg(unix)]
pub(crate) fn absolute_ipc_bind_path(path: &OsStr) -> OsString {
    let path = std::path::PathBuf::from(path);
    if path.is_absolute() {
        path.into_os_string()
    } else {
        std::path::absolute(&path).unwrap_or(path).into_os_string()
    }
}

/// Store the normalized argv snapshot for a possible loader re-exec.
pub(crate) fn initialize_normalized_args(
    args: Vec<OsString>,
    bind_path: Option<&OsStr>,
    pid_path: Option<&OsStr>,
) {
    let _ = NORMALIZED_ARGS.set(normalize_interactive_args(args, bind_path, pid_path));
}

/// Return argv with interactive IPC paths fixed against the initial cwd.
pub(crate) fn normalized_args() -> Vec<OsString> {
    NORMALIZED_ARGS
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::args_os().skip(1).collect())
}

fn normalize_interactive_args(
    args: Vec<OsString>,
    bind_path: Option<&OsStr>,
    pid_path: Option<&OsStr>,
) -> Vec<OsString> {
    let mut normalized = Vec::with_capacity(args.len());
    let bind_path = bind_path.map(OsStr::to_os_string);
    let pid_path = pid_path.map(OsStr::to_os_string);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            normalized.extend(args[index..].iter().cloned());
            break;
        }
        if arg == "--ipc-bind" {
            if let Some(bind) = bind_path.as_ref()
                && index + 1 < args.len()
            {
                normalized.push(arg.clone());
                normalized.push(bind.clone());
                index += 2;
                continue;
            }
        } else if arg == "--ipc-pid-file"
            && let Some(pid) = pid_path.as_ref()
            && index + 1 < args.len()
        {
            normalized.push(arg.clone());
            normalized.push(pid.clone());
            index += 2;
            continue;
        }
        if let Some(bind) = bind_path.as_ref()
            && os_str_has_ascii_prefix(arg, b"--ipc-bind=")
        {
            let mut rewritten = OsString::from("--ipc-bind=");
            rewritten.push(bind);
            normalized.push(rewritten);
            index += 1;
            continue;
        }
        if let Some(pid) = pid_path.as_ref()
            && os_str_has_ascii_prefix(arg, b"--ipc-pid-file=")
        {
            let mut rewritten = OsString::from("--ipc-pid-file=");
            rewritten.push(pid);
            normalized.push(rewritten);
            index += 1;
            continue;
        }
        normalized.push(arg.clone());
        index += 1;
    }
    normalized
}

/// Check an ASCII option prefix without requiring the complete argument to be
/// UTF-8. This preserves and rewrites non-UTF-8 path values losslessly.
fn os_str_has_ascii_prefix(value: &OsStr, prefix: &[u8]) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().starts_with(prefix)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        value
            .encode_wide()
            .take(prefix.len())
            .eq(prefix.iter().copied().map(u16::from))
    }
    #[cfg(not(any(unix, windows)))]
    {
        value
            .to_str()
            .is_some_and(|value| value.as_bytes().starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_interactive_args;
    use std::ffi::OsString;

    #[test]
    fn normalize_args_rewrites_separated_options() {
        let args = vec![
            OsString::from("--ipc-bind"),
            OsString::from("relative.sock"),
            OsString::from("--ipc-pid-file"),
            OsString::from("relative.pid"),
        ];
        let normalized = normalize_interactive_args(
            args,
            Some(OsString::from("/tmp/effective.sock").as_os_str()),
            Some(OsString::from("/tmp/effective.pid").as_os_str()),
        );
        assert_eq!(normalized[1], "/tmp/effective.sock");
        assert_eq!(normalized[3], "/tmp/effective.pid");
    }

    #[test]
    fn normalize_args_rewrites_equal_options_and_preserves_non_utf8_arg() {
        #[cfg(unix)]
        use std::os::unix::ffi::OsStringExt;
        let unrelated = {
            #[cfg(unix)]
            {
                OsString::from_vec(vec![b'X', 0xff])
            }
            #[cfg(not(unix))]
            {
                OsString::from("unrelated")
            }
        };
        #[cfg(unix)]
        let non_utf_bind = OsString::from_vec(b"--ipc-bind=relative\xff.sock".to_vec());
        #[cfg(unix)]
        let non_utf_pid = OsString::from_vec(b"--ipc-pid-file=relative\xfe.pid".to_vec());
        let args = vec![
            #[cfg(unix)]
            non_utf_bind,
            #[cfg(not(unix))]
            OsString::from("--ipc-bind=relative.sock"),
            #[cfg(unix)]
            non_utf_pid,
            #[cfg(not(unix))]
            OsString::from("--ipc-pid-file=relative.pid"),
            unrelated.clone(),
        ];
        let normalized = normalize_interactive_args(
            args,
            Some(OsString::from("/tmp/effective.sock").as_os_str()),
            Some(OsString::from("/tmp/effective.pid").as_os_str()),
        );
        assert_eq!(normalized[0], "--ipc-bind=/tmp/effective.sock");
        assert_eq!(normalized[1], "--ipc-pid-file=/tmp/effective.pid");
        assert_eq!(normalized[2], unrelated);
    }

    #[test]
    fn normalize_args_without_ipc_options_preserves_arguments() {
        let args = vec![OsString::from("--with-ipc"), OsString::from("--verbose")];
        assert_eq!(normalize_interactive_args(args.clone(), None, None), args);
    }

    #[test]
    fn normalize_args_preserves_options_after_terminator() {
        let args = vec![
            OsString::from("--ipc-bind=before.sock"),
            OsString::from("--"),
            OsString::from("--ipc-bind=after.sock"),
            OsString::from("--ipc-pid-file=after.pid"),
        ];
        let normalized = normalize_interactive_args(
            args,
            Some(OsString::from("/tmp/effective.sock").as_os_str()),
            Some(OsString::from("/tmp/effective.pid").as_os_str()),
        );
        assert_eq!(normalized[0], "--ipc-bind=/tmp/effective.sock");
        assert_eq!(normalized[1], "--");
        assert_eq!(normalized[2], "--ipc-bind=after.sock");
        assert_eq!(normalized[3], "--ipc-pid-file=after.pid");
    }
}
