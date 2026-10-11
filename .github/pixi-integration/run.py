"""Exercise the production Pixi provider inside an externally activated environment."""

import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
RESULTS = Path(os.environ.get("ARF_PIXI_RESULTS", ROOT / "target/pixi-r-source"))
RESULTS.mkdir(parents=True, exist_ok=True)
WORK = RESULTS / "workspace"
WORK.mkdir(exist_ok=True)
(WORK / "pixi.toml").write_text("[workspace]\nname='different-cwd-workspace'\n")
(WORK / ".r-version").write_text("99.99.99\n")
CONFIG = RESULTS / "arf.toml"
CONFIG.write_text(r"""[history]
mode='volatile'
[experimental]
r_source_overrides=[{provider='version-file',file='.r-version'},{provider='pixi'}]
""")
EXPLICIT_CONFIG = RESULTS / "explicit-arf.toml"
EXPLICIT_CONFIG.write_text(r"""[history]
mode='volatile'
""")
ARF = ROOT / ("target/debug/arf.exe" if os.name == "nt" else "target/debug/arf")
ENV = os.environ.copy()
for key in ("R_HOME", "ARF_R_HOME", "ARF_R_VERSION", "ARF_R_PROVIDER", "ARF_CONFIG", "R_LIBS",
            "R_LIBS_SITE", "R_LIBS_USER", "R_PROFILE_USER", "R_ENVIRON_USER",
            "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR", "RUST_LOG"):
    ENV.pop(key, None)
ENV["NO_COLOR"] = "1"
ENV["ARF_TUI_TEST_ARTIFACTS"] = str(RESULTS / "terminal")


def run(name, argv, env=None, cwd=WORK, timeout=120):
    start = time.monotonic()
    result = subprocess.run([str(arg) for arg in argv], cwd=cwd, env=ENV if env is None else env,
                            capture_output=True, timeout=timeout)
    (RESULTS / f"{name}.stdout").write_bytes(result.stdout)
    (RESULTS / f"{name}.stderr").write_bytes(result.stderr)
    print(f"{name}: exit={result.returncode}, seconds={time.monotonic() - start:.3f}")
    return result


def require(result, name):
    if result.returncode:
        raise RuntimeError(f"{name} failed: {result.stderr.decode(errors='replace')[-4000:]}")


def headless(expression, home, binary, json_mode, expected_descriptor=None, explicit=False):
    name = "headless-explicit-json" if explicit else ("headless-json" if json_mode else "headless-text")
    with tempfile.TemporaryDirectory() as temporary:
        env = ENV | {"ARF_IPC_SESSIONS_DIR": str(Path(temporary) / "sessions")}
        config = EXPLICIT_CONFIG if explicit else CONFIG
        args = [ARF, "headless", "--config", config, "--no-r-auto-discovery", "--vanilla",
                "--ipc-eval-unrestricted"] + (["--json"] if json_mode else [])
        if explicit:
            args.extend(["--no-r-source-overrides", "--r-provider", "pixi"])
        process = subprocess.Popen([str(arg) for arg in args], cwd=WORK, env=env,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None:
                    stdout, stderr = process.communicate()
                    raise RuntimeError(f"{name} exited before readiness: {stderr.decode(errors='replace')}")
                ready = run(f"{name}-session", [ARF, "ipc", "session", "--pid", process.pid], env)
                if not ready.returncode:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"{name} readiness timeout")
                time.sleep(0.1)
            result = run(f"{name}-eval", [ARF, "ipc", "eval", expression, "--pid", process.pid,
                                        "--timeout", "10000"], env)
            require(result, f"{name} compiled package eval")
            value = json.loads(result.stdout)
            assert value.get("error") is None and "PIXI_COMPILED_OK" in value.get("stdout", ""), value
            require(run(f"{name}-shutdown", [ARF, "ipc", "shutdown", "--pid", process.pid], env), "shutdown")
            stdout, stderr = process.communicate(timeout=15)
            (RESULTS / f"{name}.stdout").write_bytes(stdout)
            (RESULTS / f"{name}.stderr").write_bytes(stderr)
            assert process.returncode == 0
            if json_mode:
                info = json.loads(stdout)
                assert Path(info["r_home"]).samefile(home), info
                source = info["r_source_override"]
                selection = info["r_source_selection"]
                assert selection["provider"] == "pixi", selection
                assert expected_descriptor is not None
                assert selection == {
                    "provider": expected_descriptor["provider"],
                    "selected_by": expected_descriptor["selected_by"],
                    "metadata": expected_descriptor["metadata"],
                }, selection
                metadata = selection["metadata"]
                assert metadata["kind"] == "pixi", metadata
                assert metadata["environment"] == os.environ["PIXI_ENVIRONMENT_NAME"], metadata
                assert Path(metadata["r_binary"]).samefile(binary), source
                if explicit:
                    assert source["state"] == "disabled", source
                    assert source["provider"] is None and source["metadata"] is None, source
                    assert info["warnings"] == [], info
                else:
                    assert source["state"] == "applied" and source["provider"] == "pixi", source
                    assert source["metadata"] == metadata, (source, metadata)
                    assert source["requested_version"] is None, source
                    assert len(info["warnings"]) == 1, info
                assert not stderr, stderr
            else:
                assert stderr.count(b"Warning:") == 1, stderr
            assert b"All R source overrides failed" not in stderr, stderr
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)


def main():
    prefix = Path(os.environ["CONDA_PREFIX"]).resolve(strict=True)
    binary = Path(shutil.which("R.exe" if os.name == "nt" else "R")).resolve(strict=True)
    assert binary.is_relative_to(prefix), binary
    if os.environ["PIXI_ENVIRONMENT_NAME"] == "analysis":
        assert not prefix.is_relative_to(HERE.resolve()), prefix
    native = run("native-rhome", [binary, "RHOME"])
    require(native, "native RHOME")
    home = Path(native.stdout.decode().strip().splitlines()[-1]).resolve(strict=True)
    assert home.is_relative_to(prefix), home
    ENV["ARF_TEST_PIXI_R_HOME"] = str(home)
    # Only arf's child probe should remove this poisoned inherited value.
    ENV["R_HOME"] = str(RESULTS / "unrelated-r-home")
    resolved = run("resolve", [ARF, "r", "resolve", "--config", CONFIG])
    require(resolved, "resolve")
    assert not resolved.stderr, resolved.stderr
    descriptor = json.loads(resolved.stdout)
    assert descriptor["provider"] == "pixi", descriptor
    assert descriptor["selected_by"]["kind"] == "active_environment", descriptor
    source = descriptor["selected_by"]["source"]
    assert source["kind"] == "pixi" and source["name"] == os.environ["PIXI_ENVIRONMENT_NAME"], source
    manifest = os.environ.get("PIXI_WORKSPACE_MANIFEST", os.environ.get("PIXI_PROJECT_MANIFEST"))
    # Rust canonical paths use Windows extended-length prefixes. Compare file
    # identity so equivalent native and extended-length spellings both work.
    assert Path(source["path"]).samefile(manifest), source
    assert Path(descriptor["target"]["r_binary"]).samefile(binary), descriptor
    assert Path(descriptor["target"]["r_home"]).samefile(home), descriptor
    assert descriptor["selected_by"]["requested_provider"] is None, descriptor
    metadata = descriptor["metadata"]
    assert metadata["kind"] == "pixi" and metadata["environment"] == os.environ["PIXI_ENVIRONMENT_NAME"], metadata
    assert Path(metadata["manifest"]).samefile(manifest), metadata
    assert Path(metadata["prefix"]).samefile(prefix), metadata
    assert Path(metadata["r_binary"]).samefile(binary), metadata
    assert len(descriptor["diagnostics"]) == 1, descriptor
    assert descriptor["diagnostics"][0]["code"] != "r_source_override.fallback", descriptor
    expression = r"""stopifnot(normalizePath(R.home()) == normalizePath(Sys.getenv('ARF_TEST_PIXI_R_HOME')));
stopifnot(abs(stats::sd(1:3) - 1) < 1e-10);
stopifnot(jsonlite::fromJSON('{"answer":42}')$answer == 42);
stopifnot(nchar(curl::curl_version()$version) > 0);
cat('PIXI_COMPILED_OK\n')"""
    explicit_resolved = run(
        "explicit-resolve",
        [ARF, "r", "resolve", "--r-provider", "pixi", "--no-r-source-overrides",
         "--config", EXPLICIT_CONFIG],
    )
    require(explicit_resolved, "explicit Pixi resolve")
    assert not explicit_resolved.stderr, explicit_resolved.stderr
    explicit_descriptor = json.loads(explicit_resolved.stdout)
    assert explicit_descriptor["provider"] == "pixi", explicit_descriptor
    assert explicit_descriptor["selected_by"]["kind"] == "provider_request", explicit_descriptor
    assert explicit_descriptor["selected_by"]["requested_provider"] == "pixi", explicit_descriptor
    selected_source = explicit_descriptor["selected_by"]["source"]
    assert selected_source["kind"] == "command_line_argument", selected_source
    assert selected_source["name"] == "--r-provider", selected_source
    assert Path(explicit_descriptor["target"]["r_binary"]).samefile(binary), explicit_descriptor
    assert Path(explicit_descriptor["target"]["r_home"]).samefile(home), explicit_descriptor
    assert explicit_descriptor["metadata"] == descriptor["metadata"], explicit_descriptor
    assert explicit_descriptor["diagnostics"] == [], explicit_descriptor

    env_request = ENV | {"ARF_R_PROVIDER": "pixi"}
    env_resolved = run(
        "environment-resolve",
        [ARF, "r", "resolve", "--config", EXPLICIT_CONFIG, "--no-r-source-overrides"],
        env=env_request,
    )
    require(env_resolved, "environment Pixi resolve")
    assert not env_resolved.stderr, env_resolved.stderr
    env_descriptor = json.loads(env_resolved.stdout)
    assert env_descriptor["provider"] == "pixi", env_descriptor
    assert env_descriptor["selected_by"]["kind"] == "provider_request", env_descriptor
    assert env_descriptor["selected_by"]["requested_provider"] == "pixi", env_descriptor
    assert env_descriptor["selected_by"]["source"]["kind"] == "environment_variable", env_descriptor
    assert env_descriptor["selected_by"]["source"]["name"] == "ARF_R_PROVIDER", env_descriptor
    assert env_descriptor["metadata"] == descriptor["metadata"], env_descriptor
    assert Path(env_descriptor["target"]["r_binary"]).samefile(binary), env_descriptor
    assert Path(env_descriptor["target"]["r_home"]).samefile(home), env_descriptor
    assert env_descriptor["diagnostics"] == [], env_descriptor

    clean = ENV.copy()
    clean.pop("R_HOME")
    # Conda's Windows launcher does not forward captured output reliably.
    # Use Rterm directly for the baseline; resolve must still report the launcher.
    native_binary = home / "bin/x64/Rterm.exe" if os.name == "nt" else binary
    baseline = run("native-compiled", [native_binary, "--vanilla", "--slave", "-e", expression], clean)
    require(baseline, "native R baseline")
    assert b"PIXI_COMPILED_OK" in baseline.stdout, baseline.stdout
    result = run("eval", [ARF, "--config", CONFIG, "--vanilla", "--no-r-auto-discovery", "-e", expression])
    if result.returncode:
        run("eval-initialization-debug",
            [ARF, "--config", CONFIG, "--vanilla", "--no-r-auto-discovery", "-e", expression],
            ENV | {"RUST_LOG": "arf_libr=info"})
    require(result, "embedded eval")
    assert b"PIXI_COMPILED_OK" in result.stdout, result.stdout
    assert result.stderr.count(b"Warning:") == 1, result.stderr
    assert b"All R source overrides failed" not in result.stderr, result.stderr
    explicit_eval = run(
        "explicit-eval",
        [ARF, "--config", EXPLICIT_CONFIG, "--r-provider", "pixi", "--no-r-source-overrides",
         "--vanilla", "--no-r-auto-discovery", "-e", expression],
    )
    require(explicit_eval, "explicit Pixi eval")
    assert explicit_eval.stdout.count(b"PIXI_COMPILED_OK") == 1, explicit_eval.stdout
    assert b"Warning:" not in explicit_eval.stderr, explicit_eval.stderr
    assert b"r_source_override" not in explicit_eval.stderr, explicit_eval.stderr
    script = RESULTS / "compiled.R"
    script.write_text(expression)
    require(run("script", [ARF, "--config", CONFIG, "--vanilla", "--no-r-auto-discovery", "-f", script]), "script")
    headless(expression, home, binary, True, descriptor)
    headless(expression, home, binary, False, descriptor)
    headless(expression, home, binary, True, explicit_descriptor, explicit=True)
    require(run("restart", ["cargo", "test", "--locked", "--test", "pixi_environment_tests",
                            "--", "--ignored", "--nocapture"], cwd=ROOT, timeout=180), "compiled packages and restart")
    summary = {"result": "passed", "platform": platform.platform(), "machine": platform.machine(),
               "environment": os.environ["PIXI_ENVIRONMENT_NAME"], "prefix": str(prefix),
               "manifest": manifest, "r_binary": str(binary), "r_home": str(home),
               "resolved_version": descriptor["target"]["resolved_version"]}
    (RESULTS / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
