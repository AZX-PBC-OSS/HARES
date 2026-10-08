"""Refuse the network in a Python child of the test session (see ``offline_guard``)."""

import os
import sys
from pathlib import Path

if os.environ.get("HARES_TESTS_OFFLINE") == "1":
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    import offline_guard

    offline_guard.refuse_for_this_process()
