"""Adversarial child process fixtures for the native probe prototype."""

import os
import subprocess
import sys
import time
from pathlib import Path

case = sys.argv[1]
if case == "hang":
    time.sleep(30)
elif case in ("stdout", "stderr", "both"):
    for _ in range(1024):
        if case in ("stdout", "both"):
            os.write(1, b"x" * 4096)
        if case in ("stderr", "both"):
            os.write(2, b"x" * 4096)
elif case in ("descendant", "exited-parent"):
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                             stdin=subprocess.DEVNULL)
    Path(sys.argv[2]).write_text(str(child.pid))
    if case == "descendant":
        time.sleep(30)
else:
    raise ValueError(case)
