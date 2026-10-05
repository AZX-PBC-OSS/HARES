"""Typed surface of ``pyarrow.parquet`` used by HARES (see ``pyarrow/__init__.pyi``)."""

from __future__ import annotations

from pathlib import Path

from pyarrow import Table

StrPath = str | Path


def write_table(
    table: Table,
    where: StrPath,
    *,
    compression: str | None = ...,
) -> None: ...


def read_table(source: StrPath) -> Table: ...
