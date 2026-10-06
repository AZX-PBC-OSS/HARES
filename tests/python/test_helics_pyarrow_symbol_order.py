"""The helics wheel's libzmq declares no libstdc++ dependency.

libzmq.so.5 in the helics wheel exports the C++ standard-library symbols
it was built against (2022 _ZSt/_ZNSt/__cxa definitions) without a
libstdc++.so.6 NEEDED entry. When helics is imported before pyarrow, the
dynamic resolver binds those symbols from libzmq, the two libraries
disagree about the C++ runtime, and the first pyarrow parquet write
segfaults the process. Importing pyarrow first resolves every symbol from
libstdc++ and both libraries work; tests/python/conftest.py enforces that
order for every pytest worker.

The canary below reproduces the defect in a subprocess with the bad order.
When a helics wheel whose libzmq declares its runtime dependency lands,
the subprocess survives and this test fails: that is the signal to delete
it and the conftest import-order comment.
"""

from __future__ import annotations

import subprocess
import sys

BAD_ORDER_PROBE = """
import helics
import pyarrow as pa
import pyarrow.parquet as pq
import pandas as pd
table = pa.Table.from_pandas(pd.DataFrame({"a": [1.0, 2.0]}))
pq.write_table(table, "/tmp/opencode/helics-pyarrow-canary.parquet")
print("write ok")
"""


def test_helics_wheel_reports_its_cxx_runtime():
    import helics  # noqa: F401
    from pathlib import Path

    libzmq = next(
        p
        for p in (Path(helics.__file__).parent / "install" / "lib64").glob("libzmq.so*")
        if ".so." in p.name
    )
    needed = subprocess.run(
        ["readelf", "-d", str(libzmq)], capture_output=True, text=True
    ).stdout
    assert "libstdc++.so.6" not in needed, (
        "the helics wheel's libzmq now declares libstdc++; the symbol-order "
        "defect is fixed upstream: delete this canary and the conftest's "
        "import-order workaround"
    )


def test_pyarrow_write_after_helics_import_in_the_bad_order_segfaults():
    proc = subprocess.run(
        [sys.executable, "-c", BAD_ORDER_PROBE],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert proc.returncode == -11, (
        "the helics-first order no longer segfaults the pyarrow parquet "
        "writer: the wheel is fixed; delete this canary and the conftest's "
        f"import-order workaround. (returncode {proc.returncode}, "
        f"stdout {proc.stdout!r})"
    )
