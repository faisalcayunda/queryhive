"""A permanent Trino error must fail fast; a transient one must still retry.

Run: python tests/test_retry.py
"""

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from trino import exceptions as trino_exc  # noqa: E402

from exporter.source import QueryStream, TrinoConfig  # noqa: E402


def _stream(fail_with, retries=3):
    """A QueryStream whose fetch always raises `fail_with`."""
    s = QueryStream(TrinoConfig(host="x"), "SELECT 1", retries=retries, retry_backoff=0.01)

    class Cursor:
        calls = 0

        def fetchmany(self, size):
            Cursor.calls += 1
            raise fail_with

    s._cursor = Cursor()
    return s, Cursor


def test_permission_denied_fails_fast():
    err = trino_exc.TrinoExternalError(
        {"errorName": "JDBC_ERROR", "message": 'ERROR: permission denied for schema atlas'}
    )
    s, cur = _stream(err)
    t = time.perf_counter()
    try:
        s._fetch(10)
        raise AssertionError("expected the permission error to propagate")
    except trino_exc.TrinoExternalError:
        pass  # raised verbatim, not wrapped in "giving up after N retries"
    assert cur.calls == 1, f"retried a permanent error {cur.calls} times"
    assert time.perf_counter() - t < 0.05, "backoff slept on a permanent error"
    print("ok  permission denied -> 1 attempt, no backoff")


def test_transient_error_still_retries():
    err = trino_exc.TrinoExternalError(
        {"errorName": "GENERIC_INTERNAL_ERROR", "message": "worker node lost"}
    )
    s, cur = _stream(err, retries=3)
    try:
        s._fetch(10)
        raise AssertionError("expected RuntimeError after exhausting retries")
    except RuntimeError as exc:
        assert "giving up" in str(exc)
    assert cur.calls == 4, f"expected 1 try + 3 retries, got {cur.calls}"
    print("ok  transient error -> 4 attempts")


if __name__ == "__main__":
    test_permission_denied_fails_fast()
    test_transient_error_still_retries()
    print("\nall retry tests passed")
