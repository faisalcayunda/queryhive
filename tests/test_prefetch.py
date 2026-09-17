"""_prefetched harus jaga urutan, teruskan error, dan tidak bocorkan thread.

Jalankan: python tests/test_prefetch.py
"""

import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from exporter.export import _prefetched  # noqa: E402


def test_order_preserved():
    batches = [[i * 10 + j for j in range(10)] for i in range(20)]
    got = list(_prefetched(batches))
    assert got == [v for b in batches for v in b], "urutan baris berubah"
    print("ok  urutan 200 baris dari 20 batch tetap")


def test_actually_overlaps():
    """Producer tidur 20ms per batch, consumer 20ms. Sekuensial = 400ms,
    overlap harus mendekati 200ms plus satu batch."""
    def slow():
        for i in range(10):
            time.sleep(0.02)
            yield [i]

    t = time.perf_counter()
    for _ in _prefetched(slow()):
        time.sleep(0.02)
    dt = time.perf_counter() - t
    assert dt < 0.32, f"tidak overlap: {dt*1000:.0f}ms (sekuensial ~400ms)"
    print(f"ok  overlap nyata: {dt*1000:.0f}ms vs ~400ms sekuensial")


def test_error_propagates():
    def boom():
        yield [1, 2]
        raise ValueError("fetch meledak")

    try:
        list(_prefetched(boom()))
        raise AssertionError("error dari producer tertelan")
    except ValueError as exc:
        assert "meledak" in str(exc)
    print("ok  error producer diteruskan ke consumer")


def test_no_thread_leak_on_early_exit():
    """Cancel di tengah stream: thread prefetch harus mati, bukan menggantung."""
    before = threading.active_count()

    def endless():
        i = 0
        while True:
            yield [i]
            i += 1

    gen = _prefetched(endless())
    next(gen), next(gen)
    gen.close()                       # ini yang terjadi saat user menekan Cancel
    for _ in range(50):
        if threading.active_count() <= before:
            break
        time.sleep(0.02)
    leaked = threading.active_count() - before
    assert leaked == 0, f"{leaked} thread prefetch bocor setelah cancel"
    print("ok  tidak ada thread bocor setelah cancel")


if __name__ == "__main__":
    test_order_preserved()
    test_actually_overlaps()
    test_error_propagates()
    test_no_thread_leak_on_early_exit()
    print("\nsemua test prefetch lulus")
