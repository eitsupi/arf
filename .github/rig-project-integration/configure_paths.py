"""Set isolated integration paths through GitHub Actions' environment file."""

import os
from pathlib import Path

root = Path(os.environ["RUNNER_TEMP"]) / "arf-rig-project-integration"
paths = {
    "ARF_RIG_RESULTS": root,
    "RIG_R_INSTALL_DIR": root / "rig-r",
    "RIG_BINARY_DIR": root / "rig-bin",
    "RIG_DOWNLOAD_DIR": root / "downloads",
    "XDG_CACHE_HOME": root / "cache",
    "XDG_CONFIG_HOME": root / "config",
    "XDG_DATA_HOME": root / "data",
}
for path in paths.values():
    path.mkdir(parents=True, exist_ok=True)

with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as stream:
    for name, path in paths.items():
        stream.write(f"{name}={path}\n")
