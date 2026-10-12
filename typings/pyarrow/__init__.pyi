"""Typed surface of the ``pyarrow`` package used by HARES.

The published ``pyarrow`` wheel ships no py.typed marker and the community
``pyarrow-stubs`` package stops at pyarrow 20 with unknowns in its parquet
signatures, so this stub models the Arrow surface HARES uses: building
numeric and string tables, attaching schema metadata, reading float
columns back, the record-batch stream surface the Arrow C stream
interface pins build their streams with, and importing a
``__arrow_c_stream__`` capsule to read a field's schema metadata.
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


class Field:
    name: str
    metadata: dict[bytes, bytes] | None


class Schema:
    metadata: dict[bytes, bytes] | None

    def field(self, i: int | str) -> Field: ...
    def fields(self) -> list[Field]: ...


class Table:
    num_rows: int
    column_names: list[str]
    schema: Schema

    def column(self, i: int | str) -> ChunkedArray: ...
    def replace_schema_metadata(self, metadata: dict[bytes, bytes] | None) -> Table: ...
    def to_batches(self, max_chunksize: int | None = ...) -> list[RecordBatch]: ...

    @classmethod
    def from_pandas(
        cls,
        df: pd.DataFrame,
        *,
        preserve_index: bool | None = ...,
    ) -> Table: ...

    def __repr__(self) -> str: ...


class RecordBatch:
    num_rows: int
    schema: Schema

    def append_column(self, name: str, column: Array) -> RecordBatch: ...


class RecordBatchReader:
    schema: Schema

    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...

    @classmethod
    def from_batches(
        cls, schema: Schema, batches: Sequence[RecordBatch]
    ) -> RecordBatchReader: ...

    @classmethod
    def from_stream(cls, schemaish: Any, /) -> RecordBatchReader: ...

    def read_all(self) -> Table: ...


def array(values: Sequence[float] | Sequence[str], type: DataType | None = ...) -> Array: ...


def table(data: dict[str, Array]) -> Table: ...
