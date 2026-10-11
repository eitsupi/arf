"""Compare rig's project wrapper with arf's normal embedded R startup."""

import ctypes
import json
import os
import platform
import re
import shutil
import subprocess
import sys
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
ARF = ROOT / ("target/debug/arf.exe" if os.name == "nt" else "target/debug/arf")
R_VERSION = "4.5.2"
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
    "RIG_INHERITED_MARKER",
):
    ENV.pop(key, None)
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
    r_doc_dir_env = Sys.getenv("R_DOC_DIR", unset = ""),
    r_share_dir_env = Sys.getenv("R_SHARE_DIR", unset = ""),
    r_include_dir_env = Sys.getenv("R_INCLUDE_DIR", unset = ""),
    dyld_library_path = Sys.getenv("DYLD_LIBRARY_PATH", unset = ""),
    dyld_fallback_library_path = Sys.getenv("DYLD_FALLBACK_LIBRARY_PATH", unset = ""),
    dyld_framework_path = Sys.getenv("DYLD_FRAMEWORK_PATH", unset = ""),
    lib_paths = paste(vapply(.libPaths(), norm, character(1)), collapse = "\n"),
    rvenv = if (nzchar(Sys.getenv("RVENV"))) norm(Sys.getenv("RVENV")) else "",
    marker = Sys.getenv("RIG_PROJECT_MARKER", unset = ""),
    inherited_marker = Sys.getenv("RIG_INHERITED_MARKER", unset = ""),
    jsonlite_available = as.character(has_jsonlite),
    jsonlite_package = if (has_jsonlite) norm(find.package("jsonlite")) else "",
    jsonlite_dll = if (has_jsonlite) norm(getLoadedDLLs()[["jsonlite"]][["path"]]) else "",
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


def proof(result, name):
    text = result.stdout.decode(errors="replace")
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    start = text.find("ARF_RIG_PROOF_BEGIN\n")
    end = text.find("ARF_RIG_PROOF_END\n", start)
    if result.returncode or start < 0 or end < 0:
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


def wrapper_path():
    name = "R.exe" if os.name == "nt" else "R"
    return PROJECT / ".rvenv" / "bin" / name


def wrapper_rhome():
    wrapper = wrapper_path()
    if os.name == "nt":
        command = subprocess.list2cmdline([str(wrapper), "RHOME"])
        result, record = run("wrapper-rhome", command, shell=True)
    else:
        result, record = run("wrapper-rhome", [wrapper, "RHOME"])
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


def loader_diagnostics(home, native_binary):
    library = r_library_path(home)
    wrapper_candidates = [home / "bin" / "R", native_binary]
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
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            wrapper_details[str(path)] = {
                "exists": True,
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
        stream.write(f"\nRIG_PROJECT_MARKER={MARKER}\n")

    lock, lock_record = run("rig-project-lock", rig_command(mode, "proj", "lock", "--r-version", R_VERSION))
    require(lock, "rig proj lock")
    sync, sync_record = run("rig-project-sync", rig_command(mode, "proj", "sync", "--no-install-r"))
    require(sync, "rig proj sync")
    status, status_record = run("rig-project-status", rig_command(mode, "proj", "status", "--json"))
    require(status, "rig proj status")
    status_data = json.loads(status.stdout)
    if status_data.get("warnings") != []:
        raise RuntimeError(f"rig project status reported warnings: {status_data.get('warnings')}")
    sync_status = status_data.get("sync", {})
    if sync_status.get("up_to_date") is not True or sync_status.get("reason") is not None:
        raise RuntimeError(f"rig project is not synced to its lockfile: {sync_status}")
    (EVIDENCE / "rig-project-status.json").write_text(
        json.dumps(status_data, indent=2), encoding="utf-8"
    )

    home, wrapper_record = wrapper_rhome()
    native_binary = native_r_binary(home)
    if not ARF.is_file():
        raise RuntimeError(f"arf binary does not exist: {ARF}")

    outcomes = {
        "rig_version": version_record["stdout"].strip(),
        "version_context": {
            "baseline": "released rig 0.11.0; the main branch's 0.11.1-dev is not substituted",
            "0.10.0": "Windows project wrappers were .bat shims; 0.11.0 uses .exe shims.",
            "0.11.1-dev": "Quotes the saved R_LIBS_USER in the generated .Renviron for Windows paths; this unreleased fix is intentionally outside the baseline.",
        },
        "setup": {"init": init_record, "lock": lock_record, "sync": sync_record,
                  "status": status_record,
                  "wrapper_rhome": wrapper_record, "r_home": str(home),
                  "native_r_binary": str(native_binary), "rig_mode": mode},
        "loader_diagnostics": loader_diagnostics(home, native_binary),
        "modes": {},
    }
    errors = []
    for name, flags in (("normal", []), ("no-environ", ["--no-environ"]), ("vanilla", ["--vanilla"])):
        native, native_record = run(
            f"native-{name}", [native_binary, "--slave", "--no-save", *flags, "-e", EXPRESSION]
        )
        arf, arf_record = run(
            f"arf-{name}",
            [ARF, "--config", CONFIG, "--r-home", home, "--no-r-auto-discovery",
             "--no-r-source-overrides", *flags, "-e", EXPRESSION],
        )
        case = {"native": native_record, "arf": arf_record}
        outcomes["modes"][name] = case
        expected = None
        try:
            require(native, f"native R {name} startup")
            expected = proof(native, f"native R {name}")
            case["native_proof"] = expected
            require(arf, f"arf {name} startup")
            actual = proof(arf, f"arf {name}")
            case["arf_proof"] = actual
            compared_expected = {key: value for key, value in expected.items() if key not in DIAGNOSTIC_ONLY_FIELDS}
            compared_actual = {key: value for key, value in actual.items() if key not in DIAGNOSTIC_ONLY_FIELDS}
            if compared_expected != compared_actual:
                errors.append(f"{name}: arf proof differs from native R")
            if actual["inherited_marker"] != INHERITED_MARKER:
                errors.append(f"{name}: inherited process environment was unexpectedly cleared")
            project_lib = str((PROJECT / ".rvenv" / "lib").resolve()).replace("\\", "/").casefold()
            if name == "normal":
                if actual["marker"] != MARKER:
                    errors.append(f"normal: .Renviron marker was not loaded: {actual['marker']!r}")
                if not actual["rvenv"]:
                    errors.append("normal: rig did not set RVENV")
                if not any(path.replace("\\", "/").casefold() == project_lib for path in actual["lib_paths"]):
                    errors.append(f"normal: project library missing from .libPaths(): {actual['lib_paths']}")
                if not actual["jsonlite_available"]:
                    errors.append("normal: project jsonlite package did not load")
                elif not actual["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"normal: jsonlite did not resolve to rig project library: {actual['jsonlite_package']}")
                elif not actual["jsonlite_dll"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"normal: jsonlite DLL did not resolve to rig project library: {actual['jsonlite_dll']}")
            else:
                if actual["marker"]:
                    errors.append(f"{name}: .Renviron marker should be absent: {actual['marker']!r}")
                if actual["rvenv"]:
                    errors.append(f"{name}: RVENV should not be set when rig .Renviron is skipped")
                if any(path.replace("\\", "/").casefold() == project_lib for path in actual["lib_paths"]):
                    errors.append(f"{name}: project library unexpectedly appears in .libPaths()")
                if actual["jsonlite_available"] and actual["jsonlite_package"].replace("\\", "/").casefold().startswith(project_lib + "/"):
                    errors.append(f"{name}: jsonlite unexpectedly loaded from project library")
        except Exception as error:
            case["comparison_error"] = str(error)
            errors.append(f"{name}: {error}")

        if sys.platform == "darwin":
            variant = {"status": "skipped", "reason": "native R proof unavailable"}
            if expected is not None:
                variant_env = ENV.copy()
                path_keys = {
                    "R_DOC_DIR": "r_doc_dir_env",
                    "R_SHARE_DIR": "r_share_dir_env",
                    "R_INCLUDE_DIR": "r_include_dir_env",
                    "DYLD_LIBRARY_PATH": "dyld_library_path",
                    "DYLD_FALLBACK_LIBRARY_PATH": "dyld_fallback_library_path",
                }
                applied_environment = {}
                for env_key, proof_key in path_keys.items():
                    value = expected[proof_key]
                    if value:
                        variant_env[env_key] = value
                        applied_environment[env_key] = value
                    else:
                        variant_env.pop(env_key, None)
                        applied_environment[env_key] = ""

                variant_result, variant_record = run(
                    f"arf-native-path-env-{name}",
                    [ARF, "--config", CONFIG, "--r-home", home, "--no-r-auto-discovery",
                     "--no-r-source-overrides", *flags, "-e", EXPRESSION],
                    env=variant_env,
                )
                variant = {
                    "status": "failed" if variant_result.returncode else "started",
                    "environment": applied_environment,
                    "arf": variant_record,
                }
                if not variant_result.returncode:
                    try:
                        actual_variant = proof(variant_result, f"arf path-env variant {name}")
                        variant["arf_proof"] = actual_variant
                        comparable_expected = {
                            key: value for key, value in expected.items()
                            if key not in DIAGNOSTIC_ONLY_FIELDS
                        }
                        comparable_actual = {
                            key: value for key, value in actual_variant.items()
                            if key not in DIAGNOSTIC_ONLY_FIELDS
                        }
                        variant["proof_matches_native"] = comparable_expected == comparable_actual
                        variant["status"] = "passed" if variant["proof_matches_native"] else "mismatch"
                    except Exception as error:
                        variant["status"] = "invalid-proof"
                        variant["comparison_error"] = str(error)
            case["arf_native_path_env_variant"] = variant

    wrappers = (".rvenv/bin/R.exe", ".rvenv/bin/Rscript.exe") if os.name == "nt" else (
        ".rvenv/bin/R", ".rvenv/bin/Rscript"
    )
    for relative in ("rproj.toml", "rproj.lock", ".Renviron", ".rvenv/rvenv.cfg", *wrappers):
        source = PROJECT / relative
        if source.is_file():
            target = EVIDENCE / "project" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)

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
