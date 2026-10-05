"""Typed surface of the ``pyarrow`` package used by HARES.

The published ``pyarrow`` wheel ships no py.typed marker and the community
``pyarrow-stubs`` package stops at pyarrow 20 with unknowns in its parquet
signatures, so this stub models the Arrow surface HARES uses: building
numeric and string tables, attaching schema metadata, and reading float
columns back.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Sequence

if TYPE_CHECKING:
    import pandas as pd


class DataType: ...


def float64() -> DataType: ...


class Array:
    def to_pylist(self) -> list[float]: ...
    def __len__(self) -> int: ...


class ChunkedArray:
    def to_pylist(self) -> list[float]: ...
    def __len__(self) -> int: ...


class Schema:
    metadata: dict[bytes, bytes] | None


class Table:
    num_rows: int
    column_names: list[str]
    schema: Schema

    def column(self, i: int | str) -> ChunkedArray: ...
    def replace_schema_metadata(self, metadata: dict[bytes, bytes] | None) -> Table: ...

    @classmethod
    def from_pandas(
        cls,
        df: pd.DataFrame,
        *,
        preserve_index: bool | None = ...,
    ) -> Table: ...

    def __repr__(self) -> str: ...


def array(values: Sequence[float] | Sequence[str], type: DataType | None = ...) -> Array: ...


def table(data: dict[str, Array]) -> Table: ...
