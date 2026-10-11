"""Select a runner-local detached prefix for the named-environment test."""

import json
import os
from pathlib import Path

config = Path(__file__).resolve().parent / ".pixi/config.toml"
config.parent.mkdir(exist_ok=True)
config.write_text("detached-environments = " + json.dumps(os.environ["ARF_TEST_DETACHED_PREFIX"]))
