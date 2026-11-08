"""Typed surface of ``pyarrow.ipc`` used by HARES (see ``pyarrow/__init__.pyi``)."""

from __future__ import annotations

from pyarrow import RecordBatchReader

Buffer = bytes


def open_stream(source: Buffer) -> RecordBatchReader: ...
