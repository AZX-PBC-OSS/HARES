"""Tests for retry-with-backoff logic in download_resstock_fixtures.py."""

from __future__ import annotations

import importlib.util
import socket
from pathlib import Path
from unittest import mock

_SCRIPT = Path(__file__).resolve().parent.parent.parent / "scripts" / "download_resstock_fixtures.py"
_spec = importlib.util.spec_from_file_location("download_resstock_fixtures", str(_SCRIPT))
_script = importlib.util.module_from_spec(_spec)
# Import under mock to prevent real network access from top-level module body
with mock.patch("ochre_next.data.fetch_resstock_building"):
    _spec.loader.exec_module(_script)


class TestIsTransientError:
    def test_http_503_is_transient(self):
        class FakeResp:
            status_code = 503

        exc = Exception("boom")
        exc.response = FakeResp  # type: ignore[attr-defined]
        assert _script._is_transient_error(exc) is True

    def test_http_200_is_not_transient(self):
        class FakeResp:
            status_code = 200

        exc = Exception("ok")
        exc.response = FakeResp  # type: ignore[attr-defined]
        assert _script._is_transient_error(exc) is False

    def test_http_429_is_transient(self):
        class FakeResp:
            status_code = 429

        exc = Exception("rate limited")
        exc.response = FakeResp  # type: ignore[attr-defined]
        assert _script._is_transient_error(exc) is True

    def test_socket_timeout_is_transient(self):
        assert _script._is_transient_error(socket.timeout("timed out")) is True

    def test_connection_reset_is_transient(self):
        assert _script._is_transient_error(ConnectionResetError("reset")) is True

    def test_oserror_is_transient(self):
        assert _script._is_transient_error(OSError("I/O error")) is True

    def test_value_error_is_not_transient(self):
        assert _script._is_transient_error(ValueError("bad value")) is False

    def test_type_error_is_not_transient(self):
        assert _script._is_transient_error(TypeError("wrong type")) is False

    def test_key_error_is_not_transient(self):
        assert _script._is_transient_error(KeyError("missing key")) is False

    def test_httpx_timeout_classname_is_transient(self):
        class ConnectTimeout(Exception):
            pass

        assert _script._is_transient_error(ConnectTimeout("timed out")) is True

    def test_plain_exception_is_not_transient(self):
        assert _script._is_transient_error(Exception("generic")) is False

    def test_non_transient_wrapping_transient_returns_false(self):
        inner = socket.timeout("timed out")
        outer = ValueError("bad")
        outer.__cause__ = inner
        assert _script._is_transient_error(outer) is False

    def test_transient_cause_of_non_transient_checked(self):
        inner = ConnectionResetError("reset")
        outer = RuntimeError("wrapped")
        outer.__cause__ = inner
        assert _script._is_transient_error(outer) is True


class TestRetryFetchResStockBuilding:
    """Exercise _retry_fetch_resstock_building via a mock callable."""

    def test_succeeds_on_first_attempt(self):
        fake_bldg = object()
        with mock.patch.object(_script, "fetch_resstock_building", return_value=fake_bldg):
            bldg, retries = _script._retry_fetch_resstock_building(1, "2024.2", "bldg0000001")
        assert bldg is fake_bldg
        assert retries == 0

    def test_retries_after_transient_failure_then_succeeds(self):
        fake_bldg = object()
        side_effect = [
            ConnectionError("first fail"),
            ConnectionError("second fail"),
            fake_bldg,
        ]
        with mock.patch.object(_script, "fetch_resstock_building", side_effect=side_effect):
            with mock.patch.object(_script.time, "sleep") as fake_sleep:
                bldg, retries = _script._retry_fetch_resstock_building(1, "2024.2", "bldg0000001")
        assert bldg is fake_bldg
        assert retries == 2
        assert fake_sleep.call_count == 2

    def test_exhausts_all_retries_and_returns_none(self):
        with mock.patch.object(
            _script,
            "fetch_resstock_building",
            side_effect=ConnectionError("persistent failure"),
        ):
            with mock.patch.object(_script.time, "sleep"):
                bldg, retries = _script._retry_fetch_resstock_building(1, "2024.2", "bldg0000001")
        assert bldg is None
        assert retries == 2  # retry_count is incremented before returning

    def test_non_transient_error_returns_none_immediately(self):
        with mock.patch.object(
            _script,
            "fetch_resstock_building",
            side_effect=ValueError("bad config"),
        ):
            bldg, retries = _script._retry_fetch_resstock_building(1, "2024.2", "bldg0000001")
        assert bldg is None
        assert retries == 0

    def test_backoff_delay_increases_exponentially(self):
        fake_bldg = object()
        side_effect = [
            ConnectionError("fail"),
            ConnectionError("fail"),
            fake_bldg,
        ]
        sleep_calls: list[float] = []

        def record_sleep(seconds: float) -> None:
            sleep_calls.append(seconds)

        with mock.patch.object(_script, "fetch_resstock_building", side_effect=side_effect):
            with mock.patch.object(_script.time, "sleep", side_effect=record_sleep):
                _script._retry_fetch_resstock_building(1, "2024.2", "bldg0000001")

        assert len(sleep_calls) == 2
        # First delay: 2^0 + jitter = 1.0 + random in [0,1)
        assert 1.0 <= sleep_calls[0] < 2.0
        # Second delay: 2^1 + jitter = 2.0 + random in [0,1)
        assert 2.0 <= sleep_calls[1] < 3.0
