"""Compare rig's project wrapper with arf's embedded R startup across platforms."""

import ctypes
import json
import os
import platform
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path
from urllib.parse import unquote

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
RESULTS = Path(os.environ.get("ARF_RIG_RESULTS", ROOT / "target/rig-project-integration"))
RESULTS.mkdir(parents=True, exist_ok=True)
EVIDENCE = RESULTS / "evidence"
EVIDENCE.mkdir(parents=True, exist_ok=True)
PROJECT = RESULTS / "project"
PROJECT.mkdir(parents=True, exist_ok=True)
PROJECT_B = RESULTS / "project-b"
USER_HOME = RESULTS / "user-home"
USER_HOME.mkdir(parents=True, exist_ok=True)
ARF = ROOT / ("target/debug/arf.exe" if os.name == "nt" else "target/debug/arf")
R_VERSION = "4.6.1"
MARKER = "rig-project-renviron-loaded"
INHERITED_MARKER = "inherited-process-environment-preserved"
DIAGNOSTIC_ONLY_FIELDS = {
    "r_home_env",
    "r_doc_dir_env",
    "r_share_dir_env",
    "r_include_dir_env",
    "dyld_library_path",
    "dyld_fallback_library_path",
    "dyld_framework_path",
}
ENV = os.environ.copy()
for key in (
    "R_HOME",
    "ARF_R_HOME",
    "ARF_R_VERSION",
    "ARF_R_PROVIDER",
    "ARF_CONFIG",
    "R_LIBS",
    "R_LIBS_SITE",
    "R_LIBS_USER",
    "R_PROFILE_USER",
    "R_ENVIRON_USER",
    "R_DOC_DIR",
    "R_SHARE_DIR",
    "R_INCLUDE_DIR",
    "RVENV",
    "RIG_PROJECT_MARKER",
    "ARF_RIG_RENVIRON_READS",
    "ARF_RIG_HOME_MARKER",
    "ARF_RIG_HOME_RENVIRON_READS",
    "RIG_INHERITED_MARKER",
    "R_USER",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
):
    ENV.pop(key, None)
for key in tuple(ENV):
    if key.startswith("DYLD_"):
        ENV.pop(key)
ENV["NO_COLOR"] = "1"
ENV["RIG_INHERITED_MARKER"] = INHERITED_MARKER
ENV["RIG_MODE"] = os.environ.get("RIG_MODE", "admin")
ENV["RIG_DOWNLOAD_DIR"] = str(RESULTS / "downloads")
ENV["XDG_CACHE_HOME"] = str(RESULTS / "cache")
ENV["XDG_CONFIG_HOME"] = str(RESULTS / "config")
ENV["XDG_DATA_HOME"] = str(RESULTS / "data")
if ENV["RIG_MODE"] == "user":
    ENV["RIG_R_INSTALL_DIR"] = str(RESULTS / "rig-r")
    ENV["RIG_BINARY_DIR"] = str(RESULTS / "rig-bin")
for path in ("RIG_R_INSTALL_DIR", "RIG_BINARY_DIR", "RIG_DOWNLOAD_DIR", "XDG_CACHE_HOME",
             "XDG_CONFIG_HOME", "XDG_DATA_HOME"):
    if path not in ENV:
        continue
    Path(ENV[path]).mkdir(parents=True, exist_ok=True)

CONFIG = RESULTS / "arf.toml"
CONFIG.write_text('[history]\nmode = "volatile"\n', encoding="utf-8")

EXPRESSION = r'''local({
  norm <- function(x) normalizePath(x, winslash = "/", mustWork = FALSE)
  has_jsonlite <- requireNamespace("jsonlite", quietly = TRUE)
  fields <- list(
    r_home = norm(R.home()),
    r_home_env = Sys.getenv("R_HOME", unset = ""),
    r_libs_env = Sys.getenv("R_LIBS", unset = ""),
    r_libs_site_env = Sys.getenv("R_LIBS_SITE", unset = ""),
    r_libs_user_env = Sys.getenv("R_LIBS_USER", unset = ""),
    r_doc_dir_env = Sys.getenv("R_DOC_DIR", unset = ""),
    r_share_dir_env = Sys.getenv("R_SHARE_DIR", unset = ""),
    r_include_dir_env = Sys.getenv("R_INCLUDE_DIR", unset = ""),
    dyld_library_path = Sys.getenv("DYLD_LIBRARY_PATH", unset = ""),
    dyld_fallback_library_path = Sys.getenv("DYLD_FALLBACK_LIBRARY_PATH", unset = ""),
    dyld_framework_path = Sys.getenv("DYLD_FRAMEWORK_PATH", unset = ""),
    lib_paths = paste(vapply(.libPaths(), norm, character(1)), collapse = "\n"),
    rvenv = if (nzchar(Sys.getenv("RVENV"))) norm(Sys.getenv("RVENV")) else "",
    marker = Sys.getenv("RIG_PROJECT_MARKER", unset = ""),
    renviron_reads = Sys.getenv("ARF_RIG_RENVIRON_READS", unset = ""),
    inherited_marker = Sys.getenv("RIG_INHERITED_MARKER", unset = ""),
    jsonlite_available = as.character(has_jsonlite),
    jsonlite_package = if (has_jsonlite) norm(find.package("jsonlite")) else "",
    jsonlite_dll = if (has_jsonlite) norm(getLoadedDLLs()[["jsonlite"]][["path"]]) else "",
    r_version = as.character(getRversion()),
    cwd = norm(getwd()),
    rvenv_unsynced = if (nzchar(Sys.getenv("RVENV_UNSYNCED"))) norm(Sys.getenv("RVENV_UNSYNCED")) else "",
    home = norm(path.expand("~")),
    home_marker = Sys.getenv("ARF_RIG_HOME_MARKER", unset = ""),
    home_renviron_reads = Sys.getenv("ARF_RIG_HOME_RENVIRON_READS", unset = ""),
    doc = norm(R.home("doc")),
    share = norm(R.home("share")),
    include = norm(R.home("include"))
  )
  cat("ARF_RIG_PROOF_BEGIN\n")
  for (key in names(fields)) {
    value <- URLencode(fields[[key]], reserved = TRUE)
    cat(key, "\t", value, "\n", sep = "")
  }
  cat("ARF_RIG_PROOF_END\n")
})'''


def run(name, argv, *, cwd=PROJECT, env=ENV, timeout=180, shell=False):
    started = time.monotonic()
    command = [str(arg) for arg in argv] if not shell else argv
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            timeout=timeout,
            shell=shell,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or b""
        stderr = error.stderr or b""
        if isinstance(stdout, str):
            stdout = stdout.encode()
        if isinstance(stderr, str):
            stderr = stderr.encode()
        (EVIDENCE / f"{name}.stdout").write_bytes(stdout)
        (EVIDENCE / f"{name}.stderr").write_bytes(stderr)
        return subprocess.CompletedProcess(command, 124, stdout, stderr), {
            "exit_code": 124,
            "timed_out": True,
            "timeout_seconds": timeout,
            "seconds": round(time.monotonic() - started, 3),
            "stdout": stdout.decode(errors="replace"),
            "stderr": stderr.decode(errors="replace"),
        }
    (EVIDENCE / f"{name}.stdout").write_bytes(result.stdout)
    (EVIDENCE / f"{name}.stderr").write_bytes(result.stderr)
    record = {
        "exit_code": result.returncode,
        "timed_out": False,
        "seconds": round(time.monotonic() - started, 3),
        "stdout": result.stdout.decode(errors="replace"),
        "stderr": result.stderr.decode(errors="replace"),
    }
    return result, record


def require(result, name):
    if result.returncode:
        raise RuntimeError(f"{name} failed: {result.stderr.decode(errors='replace')[-4000:]}")


def r_process_env(env):
    child_env = env.copy()
    child_env["HOME"] = str(USER_HOME)
    # Windows R prefers R_USER over HOME when expanding `~`.
    if os.name == "nt":
        child_env["R_USER"] = str(USER_HOME)
    return child_env


def proof(result, name):
    return proof_text(result.stdout.decode(errors="replace"), name)


def proof_text(text, name):
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    start = text.find("ARF_RIG_PROOF_BEGIN\n")
    end = text.find("ARF_RIG_PROOF_END\n", start)
    if start < 0 or end < 0:
        raise RuntimeError(f"{name} produced no proof payload: {text[-3000:]}")
    fields = {}
    for line in text[start + len("ARF_RIG_PROOF_BEGIN\n"):end].splitlines():
        key, value = line.split("\t", 1)
        fields[key] = unquote(value)
    fields["lib_paths"] = fields["lib_paths"].splitlines()
    fields["jsonlite_available"] = fields["jsonlite_available"] == "TRUE"
    return fields


def rig_command(mode, *args):
    return ["rig", *args, f"--{mode}"]


def wrapper_path(project=PROJECT):
    name = "R.exe" if os.name == "nt" else "R"
    return project / ".rvenv" / "bin" / name


def wrapper_rhome():
    wrapper = wrapper_path()
    child_env = r_process_env(ENV)
    if os.name == "nt":
        command = subprocess.list2cmdline([str(wrapper), "RHOME"])
        result, record = run("wrapper-rhome", command, env=child_env, shell=True)
    else:
        result, record = run("wrapper-rhome", [wrapper, "RHOME"], env=child_env)
    require(result, "rig R wrapper RHOME")
    candidates = [line.strip() for line in record["stdout"].splitlines() if line.strip()]
    if not candidates:
        raise RuntimeError(f"rig wrapper printed no R_HOME: {record}")
    home = Path(candidates[-1]).resolve(strict=True)
    return home, record


def native_r_binary(home):
    candidates = (
        home / "bin" / "Rterm.exe",
        home / "bin" / "x64" / "Rterm.exe",
        home / "bin" / "R.exe",
        home / "bin" / "x64" / "R.exe",
        home / "bin" / "R",
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise RuntimeError(f"could not locate native R executable under {home}")


def r_library_path(home):
    if sys.platform == "darwin":
        return home / "lib" / "libR.dylib"
    if os.name == "nt":
        bin_dir = home / "bin"
        if platform.machine().lower() not in ("arm64", "aarch64"):
            bin_dir /= "x64"
        return bin_dir / "R.dll"
    return home / "lib" / "libR.so"


def file_kind(path):
    header = path.open("rb").read(8)
    if header.startswith(b"#!"):
        return "script"
    if header.startswith(b"MZ"):
        return "binary:MZ"
    if header.startswith(b"\x7fELF"):
        return "binary:ELF"
    if header[:4] in {
        b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
    }:
        return "binary:Mach-O"
    if b"\0" in header:
        return "binary:unknown"
    return "text"


def loader_diagnostics(home, native_binary):
    library = r_library_path(home)
    wrapper_candidates = [home / "bin" / "R", native_binary, wrapper_path()]
    dependency_result = None
    if library.is_file():
        tool = "otool" if sys.platform == "darwin" else "ldd" if os.name != "nt" else None
        if tool and shutil.which(tool):
            command = [tool, "-L", library] if tool == "otool" else [tool, library]
            _, dependency_result = run("r-library-dependencies", command, cwd=ROOT)
        try:
            ctypes.CDLL(str(library), mode=getattr(ctypes, "RTLD_GLOBAL", 0))
            dlopen = {"loaded": True, "error": None}
        except OSError as error:
            dlopen = {"loaded": False, "error": str(error)}
    else:
        dlopen = {"loaded": False, "error": "R shared library path does not exist"}

    wrapper_details = {}
    relevant = ("R_HOME", "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR", "DYLD_", "LD_LIBRARY_PATH")
    assignments = ("R_HOME", "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR")

    def assignment_matches(lines, key):
        patterns = (
            re.compile(rf"^\s*(?:export\s+)?{key}\s*=\s*(.*?)\s*$", re.IGNORECASE),
            re.compile(rf"^\s*set\s+[\"']?{key}=(.*?)[\"']?\s*$", re.IGNORECASE),
            re.compile(rf"^\s*\$env:{key}\s*=\s*(.*?)\s*$", re.IGNORECASE),
        )
        matches = []
        for line in lines:
            for pattern in patterns:
                match = pattern.match(line)
                if match:
                    matches.append({"line": line.strip(), "value": match.group(1)})
                    break
        return matches[:20]

    for path in dict.fromkeys(wrapper_candidates):
        if not path.is_file():
            wrapper_details[str(path)] = {"exists": False}
            continue
        try:
            kind = file_kind(path)
            if kind.startswith("binary:"):
                wrapper_details[str(path)] = {
                    "exists": True,
                    "kind": kind,
                    "size_bytes": path.stat().st_size,
                }
                continue
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            wrapper_details[str(path)] = {
                "exists": True,
                "kind": kind,
                "relevant_lines": [line.strip() for line in lines if any(key in line for key in relevant)][:80],
                "path_assignments": {
                    key: assignment_matches(lines, key)
                    for key in assignments
                },
            }
        except OSError as error:
            wrapper_details[str(path)] = {"exists": True, "read_error": str(error)}

    return {
        "r_home": str(home),
        "r_library": str(library),
        "r_library_exists": library.is_file(),
        "ctypes_dlopen": dlopen,
        "dependency_inspection": dependency_result,
        "wrapper_candidates": wrapper_details,
        "r_home_etc": {
            str(path): {"exists": path.is_file(), "relevant_lines": [
                line.strip() for line in path.read_text(encoding="utf-8", errors="replace").splitlines()
                if any(key in line for key in relevant)
            ][:80]} if path.is_file() else {"exists": False}
            for path in (home / "etc" / "Renviron", home / "etc" / "Renviron.site")
        },
        "inherited_loader_environment": {
            key: ENV.get(key, "") for key in (
                "R_HOME", "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR",
                "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH",
                "LD_LIBRARY_PATH",
            )
        },
    }


def switch_native_baselines(switch_env):
    homes = {}
    baselines = {}
    child_env = r_process_env(switch_env)
    for version in ("4.6.1", "4.6.0", "4.5.3"):
        resolved, resolve_record = run(
            f"switch-resolve-{version}",
            [ARF, "r", "resolve", "--with-r-version", version],
            cwd=PROJECT,
            env=ENV,
        )
        require(resolved, f"arf resolve R {version}")
        descriptor = json.loads(resolved.stdout)
        home_value = descriptor.get("target", {}).get("r_home")
        if not isinstance(home_value, str):
            raise RuntimeError(f"arf resolve did not report R_HOME for {version}: {descriptor}")
        home = Path(home_value).resolve(strict=True)
        native_binary = native_r_binary(home)
        native, native_record = run(
            f"switch-native-{version}",
            [native_binary, "--slave", "--no-save", "-e", EXPRESSION],
            cwd=PROJECT,
            env=child_env,
        )
        require(native, f"native R {version} switch baseline")
        native_proof = proof(native, f"native R {version} switch baseline")
        if native_proof["home"].replace("\\", "/").casefold() != str(USER_HOME.resolve()).replace("\\", "/").casefold():
            raise RuntimeError(f"native R {version} did not use the isolated HOME: {native_proof['home']}")
        homes[version] = str(home)
        baselines[version] = {
            "resolve": resolve_record,
            "native": native_record,
            "proof": native_proof,
        }
    return homes, baselines


def explicit_rig_project_resolution(source, home, installation_name):
    child_env = r_process_env(ENV)
    args = [ARF, "r", "resolve", "--no-r-source-overrides", "--config", CONFIG]
    if source == "cli":
        args.extend(["--r-provider", "rig-project"])
    elif source == "environment":
        child_env["ARF_R_PROVIDER"] = "rig-project"
    else:
        raise ValueError(f"unknown rig-project selection source: {source}")
    result, record = run(f"resolve-rig-project-{source}", args, env=child_env)
    require(result, f"rig-project resolve from {source}")
    descriptor = json.loads(result.stdout)
    selected_by = descriptor.get("selected_by", {})
    expected_source = {
        "kind": "command_line_argument" if source == "cli" else "environment_variable",
        "name": "--r-provider" if source == "cli" else "ARF_R_PROVIDER",
    }
    metadata = descriptor.get("metadata", {})
    target = descriptor.get("target", {})
    if descriptor.get("provider") != "rig-project":
        raise RuntimeError(f"{source} request selected an unexpected provider: {descriptor}")
    if (
        selected_by.get("kind") != "provider_request"
        or selected_by.get("requested_provider") != "rig-project"
        or selected_by.get("source", {}).get("kind") != expected_source["kind"]
        or selected_by.get("source", {}).get("name") != expected_source["name"]
    ):
        raise RuntimeError(f"{source} provider provenance is incorrect: {selected_by}")
    if metadata.get("kind") != "rig_project" or metadata.get("r_installation") != installation_name:
        raise RuntimeError(f"{source} provider metadata is incorrect: {metadata}")
    for label, actual, expected in (
        ("metadata root", metadata.get("root"), PROJECT),
        ("metadata manifest", metadata.get("manifest"), PROJECT / "rproj.toml"),
        ("metadata wrapper", metadata.get("r_binary"), wrapper_path()),
        ("target R_HOME", target.get("r_home"), home),
        ("target wrapper", target.get("r_binary"), wrapper_path()),
    ):
        if not isinstance(actual, str) or not Path(actual).samefile(expected):
            raise RuntimeError(f"{source} {label} is {actual!r}, expected {expected}")
    if target.get("resolved_version") is not None:
        raise RuntimeError(f"{source} reported an exact R version for an installation name: {target}")
    return record, descriptor


def require_nonproject_strict_failure():
    nonproject = RESULTS / "not-a-rig-project"
    nonproject.mkdir(parents=True, exist_ok=True)
    result, record = run(
        "resolve-rig-project-outside-project",
        [ARF, "r", "resolve", "--r-provider", "rig-project",
         "--no-r-source-overrides", "--config", CONFIG],
        cwd=nonproject,
    )
    if result.returncode == 0:
        raise RuntimeError("explicit rig-project selection unexpectedly succeeded outside a project")
    error = json.loads(result.stderr)
    reason_code = error.get("error", {}).get("data", {}).get("reason_code")
    if reason_code != "r_provider.rig_project_not_found":
        raise RuntimeError(f"strict non-project failure had the wrong reason code: {error}")
    return record


def run_headless_provider(home, expected_resolution):
    stderr_path = EVIDENCE / "headless-rig-project.stderr"
    command = [
        ARF, "headless", "--r-provider", "rig-project", "--no-r-source-overrides",
        "--no-r-auto-discovery", "--ipc-eval-unrestricted", "--json", "--config", CONFIG,
    ]
    ready = queue.Queue(maxsize=1)
    with stderr_path.open("wb") as stderr:
        process = subprocess.Popen(
            [str(arg) for arg in command], cwd=PROJECT, env=r_process_env(ENV),
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=stderr,
        )

        def read_ready_line():
            line = process.stdout.readline()
            ready.put(line)

        reader = threading.Thread(target=read_ready_line, daemon=True)
        reader.start()
        headless_record = {"pid": process.pid, "stderr": str(stderr_path)}
        try:
            try:
                line = ready.get(timeout=180)
            except queue.Empty as error:
                raise RuntimeError("headless rig-project session did not report JSON readiness") from error
            if not line:
                stderr_text = stderr_path.read_text(encoding="utf-8", errors="replace")
                raise RuntimeError(
                    f"headless rig-project exited before readiness: {process.poll()}: {stderr_text}"
                )
            info = json.loads(line)
            selection = info.get("r_source_selection", {})
            for key in ("provider", "selected_by", "metadata"):
                if selection.get(key) != expected_resolution.get(key):
                    raise RuntimeError(
                        f"headless {key} differs from arf r resolve: "
                        f"{selection.get(key)!r} != {expected_resolution.get(key)!r}"
                    )
            if info.get("r_home") is None or not Path(info["r_home"]).samefile(home):
                raise RuntimeError(f"headless session selected the wrong R_HOME: {info.get('r_home')}")

            evaluated, eval_record = run(
                "headless-rig-project-eval",
                [ARF, "ipc", "eval", EXPRESSION, "--pid", str(info["pid"]), "--timeout", "90000"],
                cwd=PROJECT,
                env=r_process_env(ENV),
                timeout=120,
            )
            require(evaluated, "headless rig-project IPC evaluation")
            response = json.loads(evaluated.stdout)
            if response.get("error") is not None:
                raise RuntimeError(f"headless rig-project R evaluation failed: {response}")
            observed = proof_text(response.get("stdout", ""), "headless rig-project")
            project_lib = str((PROJECT / ".rvenv" / "lib").resolve()).replace("\\", "/").casefold()
            if not observed["jsonlite_available"] or not observed["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                raise RuntimeError(f"headless jsonlite did not load from the project library: {observed}")
            if not observed["jsonlite_dll"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                raise RuntimeError(f"headless jsonlite DLL did not load from the project library: {observed}")
            headless_record["selection"] = selection
            headless_record["ipc_eval"] = eval_record
            headless_record["proof"] = observed
            return headless_record
        finally:
            active_error = sys.exc_info()[0] is not None
            cleanup_errors = []
            shutdown_sent = False
            forced_termination = False
            if process.poll() is None and "info" in locals() and isinstance(info, dict) and info.get("pid"):
                try:
                    shutdown, shutdown_record = run(
                        "headless-rig-project-shutdown",
                        [ARF, "ipc", "shutdown", "--pid", str(info["pid"])],
                        cwd=PROJECT,
                        env=r_process_env(ENV),
                        timeout=30,
                    )
                except Exception as error:
                    cleanup_errors.append(f"headless rig-project shutdown failed: {error}")
                else:
                    if shutdown.returncode:
                        cleanup_errors.append(
                            "headless rig-project shutdown failed: "
                            f"{shutdown.stderr.decode(errors='replace')}"
                        )
                    else:
                        shutdown_sent = True
                        headless_record["shutdown"] = shutdown_record
            elif process.poll() is None:
                try:
                    process.terminate()
                except ProcessLookupError:
                    pass
                forced_termination = True
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                try:
                    process.terminate()
                except ProcessLookupError:
                    pass
                forced_termination = True
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        process.kill()
                    except ProcessLookupError:
                        pass
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        cleanup_errors.append("headless rig-project process remained alive after kill")
            exit_code = process.returncode
            headless_record["exit_code"] = exit_code
            if forced_termination:
                cleanup_errors.append("headless rig-project required forced process termination")
            if shutdown_sent and exit_code != 0:
                cleanup_errors.append(
                    f"headless rig-project exited with status {exit_code} after shutdown"
                )
            if not shutdown_sent and not active_error:
                cleanup_errors.append("headless rig-project exited before graceful shutdown")
            if cleanup_errors:
                headless_record["cleanup_errors"] = cleanup_errors
                if not active_error:
                    raise RuntimeError("; ".join(cleanup_errors))


def project_b_native_baselines(home, a_proof):
    binary = native_r_binary(home)
    clean_env = r_process_env(ENV)
    inherited_env = clean_env.copy()
    inherited_fields = {
        "R_HOME": "r_home_env",
        "R_DOC_DIR": "r_doc_dir_env",
        "R_SHARE_DIR": "r_share_dir_env",
        "R_INCLUDE_DIR": "r_include_dir_env",
        "R_LIBS": "r_libs_env",
        "R_LIBS_SITE": "r_libs_site_env",
        "R_LIBS_USER": "r_libs_user_env",
        "RVENV": "rvenv",
        "RVENV_UNSYNCED": "rvenv_unsynced",
        "DYLD_LIBRARY_PATH": "dyld_library_path",
        "DYLD_FALLBACK_LIBRARY_PATH": "dyld_fallback_library_path",
        "DYLD_FRAMEWORK_PATH": "dyld_framework_path",
    }
    for key, field in inherited_fields.items():
        value = a_proof[field]
        if value:
            inherited_env[key] = value
        else:
            inherited_env.pop(key, None)

    baselines = {}
    for name, child_env in (("clean", clean_env), ("inherited-a", inherited_env)):
        result, record = run(
            f"project-b-native-{name}",
            [binary, "--slave", "--no-save", "-e", EXPRESSION],
            cwd=PROJECT_B,
            env=child_env,
        )
        require(result, f"native R project B {name} baseline")
        baselines[name] = {"record": record, "proof": proof(result, f"native project B {name}")}
    return baselines


def compare_project_b_restart(baselines):
    path = EVIDENCE / "switch-proofs" / "project-b-restart.txt"
    if not path.is_file():
        return {}, [f"missing project B restart proof: {path}"]
    try:
        observed = proof_text(path.read_text(encoding="utf-8"), "project B restart")
        inherited = baselines["inherited-a"]["proof"]
        clean = baselines["clean"]["proof"]

        def same_path(actual, expected):
            try:
                return Path(actual).samefile(expected)
            except OSError:
                return os.path.normcase(str(Path(actual).resolve())) == os.path.normcase(
                    str(Path(expected).resolve())
                )

        ignored = DIAGNOSTIC_ONLY_FIELDS | {"renviron_reads", "home_renviron_reads"}
        comparable_observed = {key: value for key, value in observed.items() if key not in ignored}
        comparable_inherited = {key: value for key, value in inherited.items() if key not in ignored}
        errors = []
        if comparable_observed != comparable_inherited:
            errors.append("project B restart differs from native R B with project A's inherited environment")
        for field in (
            "r_home", "doc", "share", "include", "r_home_env", "r_doc_dir_env",
            "r_share_dir_env", "r_include_dir_env",
        ):
            observed_value = observed[field]
            clean_value = clean[field]
            if not observed_value or not clean_value:
                same = observed_value == clean_value
            else:
                same = same_path(observed_value, clean_value)
            if not same:
                errors.append(
                    f"project B restart retained a stale R {field} path: "
                    f"{observed_value!r}, clean project B uses {clean_value!r}"
                )
        project_b_rvenv = PROJECT_B / ".rvenv"
        project_b_lib = project_b_rvenv / "lib"
        if clean["rvenv"] and same_path(clean["rvenv"], project_b_rvenv):
            if not observed["rvenv"] or not same_path(observed["rvenv"], project_b_rvenv):
                errors.append(f"project B restart did not select project B's RVENV: {observed['rvenv']!r}")
            if not any(same_path(item, project_b_lib) for item in observed["lib_paths"]):
                errors.append(f"project B restart omitted project B's library: {observed['lib_paths']}")
        if observed["cwd"].replace("\\", "/").casefold() != str(PROJECT_B.resolve()).replace("\\", "/").casefold():
            errors.append(f"project B restart did not retain the new CWD: {observed['cwd']}")
        if observed["marker"] != "rig-project-b-renviron-loaded":
            errors.append(f"project B .Renviron marker was not loaded: {observed['marker']!r}")
        return observed, errors
    except Exception as error:
        return {}, [f"project B restart proof could not be compared: {error}"]


def compare_switch_proofs(homes, baselines):
    proof_dir = EVIDENCE / "switch-proofs"
    stages = {
        "initial": "4.6.1",
        "restart": "4.6.1",
        "same-minor-after-invalid": "4.6.0",
        "switch-same-minor": "4.6.0",
        "switch-different-minor": "4.5.3",
        "restart-different-minor": "4.5.3",
    }
    ignored = DIAGNOSTIC_ONLY_FIELDS | {"renviron_reads", "home_renviron_reads"}
    records = {}
    errors = []
    project_lib = str((PROJECT / ".rvenv" / "lib").resolve()).replace("\\", "/").casefold()
    unrelated_loader_dir = str((RESULTS / "dyld-unrelated").resolve()).replace("\\", "/").casefold()
    for stage, version in stages.items():
        path = proof_dir / f"{stage}.txt"
        if not path.is_file():
            errors.append(f"missing switch proof {stage}: {path}")
            continue
        try:
            observed = proof_text(path.read_text(encoding="utf-8"), stage)
            expected = baselines[version]["proof"]
            records[stage] = observed
            comparable_observed = {key: value for key, value in observed.items() if key not in ignored}
            comparable_expected = {key: value for key, value in expected.items() if key not in ignored}
            if comparable_observed != comparable_expected:
                errors.append(f"{stage}: arf switch proof differs from native R {version}")
            expected_home = homes[version].replace("\\", "/")
            actual_home = observed["r_home"].replace("\\", "/")
            if actual_home != expected_home:
                errors.append(f"{stage}: R.home() is {actual_home}, expected {expected_home}")
            if observed["cwd"].replace("\\", "/").casefold() != str(PROJECT.resolve()).replace("\\", "/").casefold():
                errors.append(f"{stage}: restart changed CWD: {observed['cwd']}")
            isolated_home = str(USER_HOME.resolve()).replace("\\", "/").casefold()
            if observed["home"].replace("\\", "/").casefold() != isolated_home:
                errors.append(f"{stage}: R did not use isolated HOME: {observed['home']}")
            if observed["marker"] != MARKER:
                errors.append(f"{stage}: project .Renviron marker is missing")
            if observed["home_marker"] != "rig-home-renviron-loaded":
                errors.append(f"{stage}: user .Renviron marker is missing")
            if observed["inherited_marker"] != INHERITED_MARKER:
                errors.append(f"{stage}: inherited process marker was lost")
            if version in ("4.6.1", "4.6.0"):
                if not observed["rvenv"]:
                    errors.append(f"{stage}: rig project activation was not retained")
                if not any(item.replace("\\", "/").casefold() == project_lib for item in observed["lib_paths"]):
                    errors.append(f"{stage}: project library is missing from .libPaths()")
                if not observed["jsonlite_available"]:
                    errors.append(f"{stage}: compiled jsonlite did not load")
                elif not observed["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"{stage}: jsonlite package did not load from the project library")
                elif not observed["jsonlite_dll"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"{stage}: jsonlite DLL did not load from the project library")
            # TODO: Restore the mismatch assertions after https://github.com/r-lib/rig/issues/450
            # is fixed: RVENV should be unset and the incompatible project library absent.
            # else:
            #     if observed["rvenv"]:
            #         errors.append(f"{stage}: stale RVENV was retained after the R minor mismatch")
            #     if any(item.replace("\\", "/").casefold() == project_lib for item in observed["lib_paths"]):
            #         errors.append(f"{stage}: incompatible project library remains in .libPaths()")
            if stage in ("switch-same-minor", "switch-different-minor", "restart-different-minor"):
                target_lib = str((Path(homes[version]) / "lib").resolve()).replace("\\", "/").casefold()
                loader_paths = [item.replace("\\", "/").casefold() for item in observed["dyld_library_path"].split(os.pathsep) if item]
                if not loader_paths or loader_paths[0] != target_lib:
                    errors.append(f"{stage}: selected R lib is not first in DYLD_LIBRARY_PATH: {loader_paths}")
                if unrelated_loader_dir not in loader_paths:
                    errors.append(f"{stage}: unrelated startup loader path was not preserved: {loader_paths}")
        except Exception as error:
            errors.append(f"{stage}: {error}")

    read_counts = {
        "initial": ("x", "x"),
        "restart": ("xx", "x"),
        "same-minor-after-invalid": ("xxx", "xx"),
        "switch-same-minor": ("xxx", "xx"),
        "switch-different-minor": ("xxxx", "xxx"),
        # TODO: Restore the `xxxx` home .Renviron read-count expectation after rig issue #450
        # (https://github.com/r-lib/rig/issues/450) is fixed. RVENV incorrectly remains active
        # after the mismatch, so rig skips rereading home .Renviron.
        # "restart-different-minor": ("xxxxx", "xxxx"),
        "restart-different-minor": ("xxxxx", None),
    }
    for stage, (project_reads, home_reads) in read_counts.items():
        if stage in records:
            if records[stage]["renviron_reads"] != project_reads:
                errors.append(f"{stage}: project .Renviron read marker is not {project_reads!r}")
            if home_reads is not None and records[stage]["home_renviron_reads"] != home_reads:
                errors.append(f"{stage}: home .Renviron read marker is not {home_reads!r}")
    return records, errors


def main():
    rig = shutil.which("rig")
    if not rig:
        raise RuntimeError("rig 0.11.0 is not on PATH")
    version_result, version_record = run("rig-version", [rig, "--version"], cwd=ROOT)
    require(version_result, "rig version")
    if "0.11.0" not in version_record["stdout"]:
        raise RuntimeError(f"this experiment requires released rig 0.11.0: {version_record['stdout']}")

    mode = "user" if ENV["RIG_MODE"] == "user" else "admin"
    init, init_record = run("rig-project-init", rig_command(mode, "proj", "init", "--r-version", R_VERSION))
    require(init, "rig proj init")
    (PROJECT / "rproj.toml").write_text(
        f'''[project]
name = "arf-rig-startup-proof"
version = "0.1.0"
type = "project"

[dependencies]
R = "={R_VERSION}"
jsonlite = "*"
''',
        encoding="utf-8",
    )
    with (PROJECT / ".Renviron").open("a", encoding="utf-8", newline="\n") as stream:
        stream.write(
            f'\nRIG_PROJECT_MARKER={MARKER}\n'
            'ARF_RIG_RENVIRON_READS="${ARF_RIG_RENVIRON_READS}x"\n'
        )

    lock, lock_record = run("rig-project-lock", rig_command(mode, "proj", "lock", "--r-version", R_VERSION))
    require(lock, "rig proj lock")
    sync, sync_record = run("rig-project-sync", rig_command(mode, "proj", "sync", "--no-install-r"))
    require(sync, "rig proj sync")
    status, status_record = run("rig-project-status", rig_command(mode, "proj", "status", "--json"))
    require(status, "rig proj status")
    status_data = json.loads(status.stdout)
    if not isinstance(status_data, dict):
        raise RuntimeError("rig project status did not return an object")
    if status_data.get("warnings") != []:
        raise RuntimeError(f"rig project status reported warnings: {status_data.get('warnings')}")
    if status_data.get("manifest") is None or status_data.get("lock") is None:
        raise RuntimeError("rig project status did not report both manifest and lock")
    status_root = status_data.get("root")
    if not isinstance(status_root, str) or Path(status_root).resolve() != PROJECT.resolve():
        raise RuntimeError(f"rig project status resolved a different root: {status_data.get('root')}")
    sync_status = status_data.get("sync")
    if not isinstance(sync_status, dict) or not {"up_to_date", "reason"}.issubset(sync_status):
        raise RuntimeError(f"rig project status omitted sync fields: {sync_status}")
    if sync_status["up_to_date"] is not True or sync_status["reason"] is not None:
        raise RuntimeError(f"rig project is not synced to its lockfile: {sync_status}")
    (EVIDENCE / "rig-project-status.json").write_text(
        json.dumps(status_data, indent=2), encoding="utf-8"
    )

    (USER_HOME / ".Renviron").write_text(
        "ARF_RIG_HOME_MARKER=rig-home-renviron-loaded\n"
        'ARF_RIG_HOME_RENVIRON_READS="${ARF_RIG_HOME_RENVIRON_READS}x"\n',
        encoding="utf-8",
    )

    home, wrapper_record = wrapper_rhome()
    native_binary = native_r_binary(home)
    if not ARF.is_file():
        raise RuntimeError(f"arf binary does not exist: {ARF}")

    resolution_records = {}
    resolution_descriptors = {}
    for source in ("cli", "environment"):
        record, descriptor = explicit_rig_project_resolution(
            source, home, status_data["sync"]["r_version"]
        )
        resolution_records[source] = record
        resolution_descriptors[source] = descriptor

    strict_failure = require_nonproject_strict_failure()

    outcomes = {
        "rig_version": version_record["stdout"].strip(),
        "version_context": "released rig 0.11.0 user-mode fixture",
        "arf_startup": "explicit rig-project provider startup, script mode, and headless IPC package loading; macOS also checks version switching and project-CWD restart",
        "comparison": "startup compares the native rig wrapper with arf; project-CWD restart compares inherited and clean native baselines; switch baselines use arf-resolved native R binaries",
        "rig_project_resolution": resolution_records,
        "strict_nonproject_failure": strict_failure,
        "setup": {"init": init_record, "lock": lock_record, "sync": sync_record,
                  "status": status_record,
                  "wrapper_rhome": wrapper_record, "r_home": str(home),
                  "native_r_binary": str(native_binary), "rig_mode": mode},
        "initial_environment": {
            key: ENV.get(key, "") for key in (
                "R_LIBS", "R_LIBS_SITE", "R_LIBS_USER", "RVENV",
                "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR",
                "R_USER",
                "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH",
                "DYLD_FRAMEWORK_PATH",
            )
        },
        "loader_diagnostics": loader_diagnostics(home, native_binary),
        "modes": {},
    }
    errors = []
    r_env = r_process_env(ENV)
    for name, flags in (("normal", []), ("no-environ", ["--no-environ"]), ("vanilla", ["--vanilla"])):
        native, native_record = run(
            f"native-{name}", [native_binary, "--slave", "--no-save", *flags, "-e", EXPRESSION],
            env=r_env,
        )
        arf, arf_record = run(
            f"arf-{name}",
            [ARF, "--r-provider", "rig-project", "--config", CONFIG,
             "--no-r-auto-discovery", "--no-r-source-overrides", *flags,
             "-e", EXPRESSION],
            env=r_env,
        )
        case = {"native": native_record, "arf": arf_record}
        outcomes["modes"][name] = case
        try:
            require(native, f"native R {name} startup")
            native_proof = proof(native, f"native R {name}")
            case["native_proof"] = native_proof
            expected_home = str(USER_HOME.resolve()).replace("\\", "/").casefold()
            if native_proof["home"].replace("\\", "/").casefold() != expected_home:
                errors.append(f"native {name}: R did not use isolated HOME: {native_proof['home']}")
            expected_reads = "x" if name == "normal" else ""
            if native_proof["renviron_reads"] != expected_reads:
                errors.append(
                    f"native {name}: expected .Renviron read marker {expected_reads!r}, "
                    f"got {native_proof['renviron_reads']!r}"
                )
        except Exception as error:
            case["native_error"] = str(error)
            errors.append(f"native {name}: {error}")

        try:
            require(arf, f"arf embedded R {name} startup")
            arf_proof = proof(arf, f"arf embedded R {name}")
            case["arf_proof"] = arf_proof
            expected_home = str(USER_HOME.resolve()).replace("\\", "/").casefold()
            if arf_proof["home"].replace("\\", "/").casefold() != expected_home:
                errors.append(f"arf {name}: R did not use isolated HOME: {arf_proof['home']}")
            if "native_proof" in case:
                comparable_native = {
                    key: value for key, value in case["native_proof"].items()
                    if key not in DIAGNOSTIC_ONLY_FIELDS
                }
                comparable_arf = {
                    key: value for key, value in arf_proof.items()
                    if key not in DIAGNOSTIC_ONLY_FIELDS
                }
                if comparable_arf != comparable_native:
                    errors.append(f"{name}: arf embedded R proof differs from native R")
            expected_reads = "x" if name == "normal" else ""
            if arf_proof["renviron_reads"] != expected_reads:
                errors.append(
                    f"arf {name}: expected .Renviron read marker {expected_reads!r}, "
                    f"got {arf_proof['renviron_reads']!r}"
                )
            if arf_proof["inherited_marker"] != INHERITED_MARKER:
                errors.append(f"arf {name}: inherited process environment was unexpectedly cleared")
            project_lib = str((PROJECT / ".rvenv" / "lib").resolve()).replace("\\", "/").casefold()
            if name == "normal":
                if arf_proof["marker"] != MARKER:
                    errors.append(f"normal: .Renviron marker was not loaded: {arf_proof['marker']!r}")
                if not arf_proof["rvenv"]:
                    errors.append("normal: rig did not set RVENV")
                if not any(path.replace("\\", "/").casefold() == project_lib
                           for path in arf_proof["lib_paths"]):
                    errors.append(f"normal: project library missing from .libPaths(): {arf_proof['lib_paths']}")
                if not arf_proof["jsonlite_available"]:
                    errors.append("normal: project jsonlite package did not load")
                elif not arf_proof["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"normal: jsonlite did not resolve to rig project library: {arf_proof['jsonlite_package']}")
                elif not arf_proof["jsonlite_dll"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"normal: jsonlite DLL did not resolve to rig project library: {arf_proof['jsonlite_dll']}")
            else:
                if arf_proof["marker"] or arf_proof["rvenv"]:
                    errors.append(f"{name}: rig .Renviron should not activate the project")
                if any(path.replace("\\", "/").casefold() == project_lib
                       for path in arf_proof["lib_paths"]):
                    errors.append(f"{name}: project library unexpectedly appears in .libPaths()")
                if arf_proof["jsonlite_available"] and arf_proof["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"{name}: jsonlite unexpectedly loaded from project library")
        except Exception as error:
            case["arf_error"] = str(error)
            errors.append(f"arf embedded R {name}: {error}")

    script_path = EVIDENCE / "rig-project-proof.R"
    # Keep the generated source LF-terminated; the embedded parser rejects CRLF input.
    with script_path.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write(EXPRESSION + "\n")
    script_result, script_record = run(
        "arf-rig-project-script",
        [ARF, "--r-provider", "rig-project", "--config", CONFIG,
         "--no-r-auto-discovery", "--no-r-source-overrides", "--file", script_path],
        env=r_env,
    )
    outcomes["script"] = script_record
    try:
        require(script_result, "arf rig-project script mode")
        script_proof = proof(script_result, "arf rig-project script")
        project_lib = str((PROJECT / ".rvenv" / "lib").resolve()).replace("\\", "/").casefold()
        if script_proof["marker"] != MARKER or not script_proof["rvenv"]:
            errors.append("script mode did not activate the rig project .Renviron")
        if not script_proof["jsonlite_available"] or not script_proof["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
            errors.append(f"script mode jsonlite did not load from the project library: {script_proof}")
        if not script_proof["jsonlite_dll"].replace("\\", "/").casefold().startswith(project_lib + "/"):
            errors.append(f"script mode jsonlite DLL did not load from the project library: {script_proof}")
        outcomes["script_proof"] = script_proof
    except Exception as error:
        errors.append(f"arf rig-project script mode: {error}")

    try:
        outcomes["headless"] = run_headless_provider(
            home, resolution_descriptors["cli"]
        )
    except Exception as error:
        outcomes["headless_error"] = str(error)
        errors.append(f"headless rig-project provider: {error}")

    wrappers = (".rvenv/bin/R.exe", ".rvenv/bin/Rscript.exe") if os.name == "nt" else (
        ".rvenv/bin/R", ".rvenv/bin/Rscript"
    )
    for relative in ("rproj.toml", "rproj.lock", ".Renviron", ".rvenv/rvenv.cfg", *wrappers):
        source = PROJECT / relative
        if source.is_file():
            target = EVIDENCE / "project" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)

    # The multi-version TUI harness also checks selected-library DYLD ordering.
    if sys.platform == "darwin":
        unrelated_loader_dir = RESULTS / "dyld-unrelated"
        unrelated_loader_dir.mkdir(parents=True, exist_ok=True)
        switch_env = ENV.copy()
        switch_env["DYLD_LIBRARY_PATH"] = os.pathsep.join((str(home / "lib"), str(unrelated_loader_dir)))
        proof_script = EVIDENCE / "switch-proof.R"
        proof_script.write_text(EXPRESSION + "\n", encoding="utf-8")
        switch_proof_dir = EVIDENCE / "switch-proofs"
        switch_proof_dir.mkdir(parents=True, exist_ok=True)
        switch_homes, switch_baselines = switch_native_baselines(switch_env)
        PROJECT_B.mkdir(parents=True, exist_ok=True)
        init_b, init_b_record = run(
            "rig-project-b-init",
            rig_command(mode, "proj", "init", "--r-version", "4.6.0"),
            cwd=PROJECT_B,
        )
        require(init_b, "rig proj init for project B")
        (PROJECT_B / "rproj.toml").write_text(
            '''[project]
name = "arf-rig-project-b-proof"
version = "0.1.0"
type = "project"

[dependencies]
R = "=4.6.0"
''',
            encoding="utf-8",
        )
        with (PROJECT_B / ".Renviron").open("a", encoding="utf-8", newline="\n") as stream:
            stream.write(
                '\nRIG_PROJECT_MARKER=rig-project-b-renviron-loaded\n'
                'ARF_RIG_RENVIRON_READS="${ARF_RIG_RENVIRON_READS}x"\n'
            )
        lock_b, lock_b_record = run(
            "rig-project-b-lock",
            rig_command(mode, "proj", "lock", "--r-version", "4.6.0"),
            cwd=PROJECT_B,
        )
        require(lock_b, "rig proj lock for project B")
        sync_b, sync_b_record = run(
            "rig-project-b-sync", rig_command(mode, "proj", "sync", "--no-install-r"),
            cwd=PROJECT_B,
        )
        require(sync_b, "rig proj sync for project B")
        status_b, status_b_record = run(
            "rig-project-b-status", rig_command(mode, "proj", "status", "--json"),
            cwd=PROJECT_B,
        )
        require(status_b, "rig proj status for project B")
        status_b_data = json.loads(status_b.stdout)
        if (
            status_b_data.get("warnings") != []
            or status_b_data.get("sync", {}).get("up_to_date") is not True
            or status_b_data.get("sync", {}).get("reason") is not None
            or not isinstance(status_b_data.get("root"), str)
            or Path(status_b_data["root"]).resolve() != PROJECT_B.resolve()
        ):
            raise RuntimeError(f"rig project B is not an up-to-date project root: {status_b_data}")
        (EVIDENCE / "rig-project-b-status.json").write_text(
            json.dumps(status_b_data, indent=2), encoding="utf-8"
        )
        b_home = Path(switch_homes["4.6.0"])
        switch_state_path = RESULTS / "switch-state.json"
        switch_state = {
            "project": str(PROJECT.resolve()),
            "project_b": str(PROJECT_B.resolve()),
            "user_home": str(USER_HOME.resolve()),
            "r_homes": switch_homes,
            "project_b_r_home": str(b_home),
            "proof_script": str(proof_script.resolve()),
            "proof_dir": str(switch_proof_dir.resolve()),
            "base_dyld_library_path": switch_env["DYLD_LIBRARY_PATH"],
        }
        switch_state_path.write_text(json.dumps(switch_state, indent=2), encoding="utf-8")
        switch_env["ARF_RIG_SWITCH_STATE"] = str(switch_state_path.resolve())
        switch_test, switch_test_record = run(
            "rig-project-version-switch-test",
            ["cargo", "test", "--locked", "--test", "rig_project_switch_tests", "--", "--ignored", "--nocapture", "--test-threads=1"],
            cwd=ROOT,
            env=switch_env,
            timeout=600,
        )
        outcomes["switch"] = {
            "versions": ["4.6.1", "4.6.0", "4.5.3"],
            "project_b": {
                "init": init_b_record,
                "lock": lock_b_record,
                "sync": sync_b_record,
                "status": status_b_record,
            },
            "startup_dyld_library_path": switch_env["DYLD_LIBRARY_PATH"],
            "native_baselines": switch_baselines,
            "tui_test": switch_test_record,
            "proofs": {},
        }
        if switch_test.returncode:
            errors.append(f"rig project version switch TUI test failed: {switch_test_record['stderr'][-4000:]}")
        else:
            switch_proofs, switch_errors = compare_switch_proofs(switch_homes, switch_baselines)
            outcomes["switch"]["proofs"] = switch_proofs
            errors.extend(switch_errors)
            a_proof_path = switch_proof_dir / "cwd-restart-before-project-b.txt"
            if a_proof_path.is_file():
                a_proof = proof_text(a_proof_path.read_text(encoding="utf-8"), "project A before CWD restart")
                b_baselines = project_b_native_baselines(b_home, a_proof)
                outcomes["switch"]["project_b"]["native_baselines"] = b_baselines
            else:
                b_baselines = {}
                errors.append(f"missing project A proof before CWD restart: {a_proof_path}")
            project_b_proof, project_b_errors = compare_project_b_restart(b_baselines)
            outcomes["switch"]["project_b"]["restart_proof"] = project_b_proof
            errors.extend(project_b_errors)
    else:
        outcomes["switch"] = {"skipped": "the rig project version-switch test currently validates macOS DYLD behavior"}

    outcomes["result"] = "failed" if errors else "passed"
    outcomes["platform"] = platform.platform()
    outcomes["machine"] = platform.machine()
    outcomes["errors"] = errors
    (EVIDENCE / "summary.json").write_text(json.dumps(outcomes, indent=2), encoding="utf-8")
    print(json.dumps({"result": outcomes["result"], "platform": outcomes["platform"],
                      "r_home": str(home), "errors": errors}, indent=2))
    if errors:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
