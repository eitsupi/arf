"""Run the unmerged embedding experiment inside a real Pixi environment."""

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
RESULTS = Path(os.environ.get("ARF_SPIKE_RESULTS", ROOT / "target/pixi-spike"))
RESULTS.mkdir(parents=True, exist_ok=True)
SUFFIX = ".exe" if os.name == "nt" else ""
ARF = ROOT / f"target/debug/arf{SUFFIX}"
PROBE = ROOT / f"target/debug/examples/pixi_rhome_probe{SUFFIX}"
MODE = os.environ.get("ARF_SPIKE_DLL_SEARCH", "restricted")
ENV = os.environ.copy()
for key in (
    "R_HOME", "ARF_R_HOME", "ARF_R_VERSION", "ARF_CONFIG", "R_LIBS",
    "R_LIBS_SITE", "R_LIBS_USER", "R_PROFILE_USER", "R_ENVIRON_USER",
    "R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR",
):
    ENV.pop(key, None)
ENV["NO_COLOR"] = "1"
ENV["RUST_LOG"] = "arf_libr=info"
ENV["ARF_TUI_TEST_ARTIFACTS"] = str(RESULTS / "terminal")


def run(name, argv, env=ENV, timeout=120):
    start = time.monotonic()
    result = subprocess.run(
        [str(arg) for arg in argv], cwd=ROOT, env=env,
        capture_output=True, timeout=timeout,
    )
    (RESULTS / f"{name}.stdout").write_bytes(result.stdout)
    (RESULTS / f"{name}.stderr").write_bytes(result.stderr)
    print(f"{name}: exit={result.returncode}, seconds={time.monotonic() - start:.3f}")
    return result


def require(result, name):
    if result.returncode:
        raise RuntimeError(f"{name} failed: {result.stderr.decode(errors='replace')[-4000:]}")


def probe_cases(binary):
    observations = []
    poisoned = ENV | {"R_HOME": str(RESULTS / "unrelated-r-home")}
    for index in range(10):
        result = run(f"rhome-{index}", [PROBE, "default", binary, "RHOME"], poisoned)
        require(result, "R RHOME probe")
        value = json.loads(result.stdout)
        assert value["status"] == "ok", value
        observations.append(value)
    home = Path(observations[0]["r_home"]).resolve(strict=True)
    assert all(Path(value["r_home"]).resolve() == home for value in observations)

    fixture = HERE / "probe_fixture.py"
    failures = {}
    for case in ("hang", "stdout", "stderr", "both", "descendant", "exited-parent"):
        pid_file = RESULTS / f"{case}.pid"
        try:
            result = run(case, [PROBE, "500", sys.executable, fixture, case, pid_file])
            require(result, f"bounded probe fixture {case}")
            value = json.loads(result.stdout)
            expected = "output_limit" if case in ("stdout", "stderr", "both") else "timeout"
            assert value["status"] == expected, value
            assert value["elapsed_ms"] < 2500, value
            failures[case] = value
        finally:
            # The prototype cancels pipes and bounds direct-child cleanup; it
            # does not claim process-tree ownership. Clean up fixture descendants
            # explicitly, even when the launcher has already exited.
            if pid_file.exists():
                pid = int(pid_file.read_text())
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                                   capture_output=True, timeout=5)
                else:
                    import signal
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
    return home, {"real_probes": observations, "fixtures": failures,
                  "timeout_ms": 10000, "bytes_per_stream": 65536,
                  "cleanup_ms": 1000, "descendant_cleanup": "fixture-owned"}


def headless(home, expression):
    with tempfile.TemporaryDirectory() as temporary:
        env = ENV | {"ARF_IPC_SESSIONS_DIR": str(Path(temporary) / "sessions")}
        process = subprocess.Popen(
            [str(ARF), "headless", "--r-home", str(home), "--no-r-auto-discovery",
             "--no-r-source-overrides", "--vanilla", "--json", "--ipc-eval-unrestricted"],
            cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None:
                    raise RuntimeError("headless exited before readiness")
                ready = run("headless-session", [ARF, "ipc", "session", "--pid", process.pid], env)
                if not ready.returncode:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("headless readiness timeout")
                time.sleep(0.1)
            result = run("headless-eval", [ARF, "ipc", "eval", expression,
                                          "--pid", process.pid, "--timeout", "10000"], env)
            require(result, "headless compiled package eval")
            value = json.loads(result.stdout)
            assert value.get("error") is None and "PIXI_COMPILED_OK" in value.get("stdout", ""), value
            require(run("headless-shutdown", [ARF, "ipc", "shutdown", "--pid", process.pid], env),
                    "headless shutdown")
            stdout, stderr = process.communicate(timeout=15)
            (RESULTS / "headless.stdout").write_bytes(stdout)
            (RESULTS / "headless.stderr").write_bytes(stderr)
            info = json.loads(stdout)
            assert Path(info["r_home"]).resolve() == home, info
            assert process.returncode == 0
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)


def main():
    prefix = Path(os.environ["CONDA_PREFIX"]).resolve(strict=True)
    binary = Path(shutil.which(f"R{SUFFIX}")).resolve(strict=True)
    assert binary.is_relative_to(prefix), binary
    home, probes = probe_cases(binary)
    assert home.is_relative_to(prefix), home
    library = home / ("bin/x64/R.dll" if os.name == "nt" else
                      "lib/libR.dylib" if sys.platform == "darwin" else "lib/libR.so")
    assert library.resolve(strict=True).is_relative_to(prefix), library
    ENV.update({"ARF_R_HOME": str(home), "R_HOME": str(home)})
    manifest = os.environ.get("PIXI_WORKSPACE_MANIFEST", os.environ.get("PIXI_PROJECT_MANIFEST"))
    marker = json.loads((prefix / "conda-meta/pixi").read_text())
    assert Path(marker["manifest_path"]).resolve() == Path(manifest).resolve()
    assert marker["environment_name"] == os.environ["PIXI_ENVIRONMENT_NAME"]
    summary = {"platform": platform.platform(), "machine": platform.machine(),
               "policy": MODE, "r_binary": str(binary), "r_home": str(home),
               "r_library": str(library), "probes": probes}
    (RESULTS / "summary.json").write_text(json.dumps(summary, indent=2))
    expression = (
        "stopifnot(normalizePath(R.home()) == normalizePath(Sys.getenv('ARF_R_HOME'))); "
        "stopifnot(abs(stats::sd(1:3) - 1) < 1e-10); "
        "stopifnot(jsonlite::fromJSON('{\"answer\":42}')$answer == 42); "
        "stopifnot(nchar(curl::curl_version()$version) > 0); "
        "cat('PIXI_COMPILED_OK\\n')"
    )
    require(run("native-r", [binary, "--vanilla", "--slave", "-e", expression]), "native R baseline")
    result = run("embedded-eval", [ARF, "--vanilla", "--no-r-auto-discovery",
                                  "--no-r-source-overrides", "-e", expression])
    summary["embedding_exit"] = result.returncode
    if result.returncode and os.name == "nt" and MODE == "restricted":
        # The unchanged loader is a baseline, not an expected supported policy.
        summary["result"] = "baseline_embedding_failure"
        (RESULTS / "summary.json").write_text(json.dumps(summary, indent=2))
        print("Recorded restricted DLL baseline failure; inspect embedded-eval.stderr")
        return
    require(result, "embedded eval")
    assert b"PIXI_COMPILED_OK" in result.stdout, result.stdout
    script = RESULTS / "compiled.R"
    script.write_text(expression)
    require(run("embedded-script", [ARF, "--vanilla", "--no-r-auto-discovery",
                                    "--no-r-source-overrides", "-f", script]), "embedded script")
    headless(home, expression)
    require(run("restart", ["cargo", "test", "--locked", "--features", "pixi-embedding-spike",
                            "--test", "pixi_embedding_spike", "--", "--ignored", "--nocapture"],
                timeout=180), "compiled package restart")
    require(run("existing-restart", ["cargo", "test", "--locked", "--features", "pixi-embedding-spike",
                                     "--test", "tui_tests", "restart_preserves_environment_and_reconnects_ipc",
                                     "--", "--nocapture"], timeout=180), "existing IPC restart")
    summary["result"] = "passed"
    (RESULTS / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
