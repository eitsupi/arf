"""Install the released rig CLI archive used by this integration proof."""

import os
import platform
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

VERSION = "0.11.0"
PREFIX = Path(os.environ.get("RUNNER_TEMP", Path.home())) / "rig-project-tool"


def main():
    machine = platform.machine().lower()
    if os.name == "nt":
        os_name, arch, extension = "windows", "arm64" if machine in ("arm64", "aarch64") else "x86_64", "zip"
    elif sys.platform == "darwin":
        os_name, arch, extension = "macos", "arm64" if machine in ("arm64", "aarch64") else "x86_64", "tar.gz"
    else:
        os_name, arch, extension = "linux", "aarch64" if machine in ("arm64", "aarch64") else "x86_64", "tar.gz"

    filename = f"rig-{os_name}-{arch}-{VERSION}.{extension}"
    url = f"https://github.com/r-lib/rig/releases/download/v{VERSION}/{filename}"
    archive = PREFIX.parent / filename
    PREFIX.mkdir(parents=True, exist_ok=True)
    print(f"Downloading released rig {VERSION} for {os_name}-{arch}")
    urllib.request.urlretrieve(url, archive)
    if extension == "zip":
        with zipfile.ZipFile(archive) as bundle:
            bundle.extractall(PREFIX)
    else:
        with tarfile.open(archive, "r:gz") as bundle:
            bundle.extractall(PREFIX, filter="data")

    bindir = PREFIX / "bin"
    binary = bindir / ("rig.exe" if os.name == "nt" else "rig")
    if not binary.is_file():
        raise RuntimeError(f"rig archive did not contain {binary}")
    github_path = os.environ.get("GITHUB_PATH")
    if github_path:
        with open(github_path, "a", encoding="utf-8") as stream:
            stream.write(f"{bindir}\n")
    print(binary)


if __name__ == "__main__":
    main()
