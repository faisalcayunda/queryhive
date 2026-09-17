"""Jalur render paralel harus menghasilkan byte yang identik dengan sekuensial.

Ini test yang paling penting dari perubahan free-threading: kalau chunk tertukar
urutannya atau ada yang hilang, file hasil export salah tanpa error apa pun.

Jalankan: python tests/test_parallel_render.py
"""

import os
import sys
import tempfile
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from exporter import export as E  # noqa: E402
from exporter.writers import WRITERS, Column  # noqa: E402

COLS = [Column("id", "bigint"), Column("name", "varchar"), Column("amount", "decimal(18,2)"),
        Column("ts", "timestamp"), Column("flag", "boolean"), Column("note", "varchar")]


def make_rows(n):
    """Sengaja pakai nilai yang bikin format rewel: NULL, koma, kutip, newline,
    non-ASCII, dan angka yang harus dibulatkan."""
    out = []
    for i in range(n):
        out.append([
            i,
            None if i % 7 == 0 else f'nama, "{i}"\nbaris-dua',
            Decimal(f"{i}.05"),
            datetime(2026, 8, 20, 10, 30, i % 60),
            i % 2 == 0,
            "ünïcode ➜ " + "x" * (i % 13),
        ])
    return out


def run(fmt, rows, workers, monkey):
    monkey(workers)
    tmp = Path(tempfile.mkdtemp())
    res = E.export_rows(COLS, iter(rows), tmp, "out", fmt)
    return res, res.files[0].read_bytes()


def test_identical_output():
    rows = make_rows(9_137)          # bukan kelipatan RENDER_CHUNK, sengaja
    original = E.render_workers
    try:
        for fmt in ("csv", "txt", "xml", "html"):
            E.render_workers = lambda: 1
            seq_res, seq = run(fmt, rows, 1, lambda w: None)
            E.render_workers = lambda: 4
            par_res, par = run(fmt, rows, 4, lambda w: None)
            assert seq == par, f"{fmt}: output paralel beda dari sekuensial"
            assert seq_res.rows == par_res.rows == len(rows), (
                f"{fmt}: jumlah baris {par_res.rows} != {len(rows)}")
            print(f"ok  {fmt:5} {len(seq):>9,} byte identik, {par_res.rows:,} baris")
    finally:
        E.render_workers = original


def test_non_renderable_formats_untouched():
    """xlsx/xls/dbf/json/sql harus tetap lewat jalur sekuensial."""
    for fmt in ("json", "sql", "dbf", "xlsx", "xls"):
        assert not WRITERS[fmt].renderable, f"{fmt} tidak boleh ditandai renderable"
    print("ok  json/sql/dbf/xlsx/xls tetap di jalur row-at-a-time")


def test_splitting_still_sequential():
    """Dengan rows_per_file, jalur paralel harus dilewati dan split tetap benar."""
    original = E.render_workers
    E.render_workers = lambda: 4
    try:
        rows = make_rows(2_500)
        tmp = Path(tempfile.mkdtemp())
        res = E.export_rows(COLS, iter(rows), tmp, "out", "csv", rows_per_file=1_000)
        assert len(res.files) == 3, f"harusnya 3 part, dapat {len(res.files)}"
        assert res.rows == 2_500
        print(f"ok  rows_per_file: {len(res.files)} part, {res.rows:,} baris")
    finally:
        E.render_workers = original


def test_cancel_midway():
    original = E.render_workers
    E.render_workers = lambda: 4
    try:
        rows = make_rows(50_000)
        tmp = Path(tempfile.mkdtemp())
        seen = {"n": 0}

        def cancel():
            seen["n"] += 1
            return seen["n"] > 3

        res = E.export_rows(COLS, iter(rows), tmp, "out", "csv", cancel=cancel)
        assert res.cancelled, "cancel diabaikan"
        assert 0 < res.rows < len(rows), f"rows={res.rows} tidak parsial"
        assert res.files[0].is_file(), "file parsial tidak ditutup"
        print(f"ok  cancel di tengah: {res.rows:,} baris tertulis, file tertutup")
    finally:
        E.render_workers = original


def test_worker_exception_propagates():
    original = E.render_workers
    E.render_workers = lambda: 4
    try:
        class Boom:
            def __init__(self, *a, **k): self.path = Path(tempfile.mkdtemp()) / "x.csv"
            renderable = True
            ext, max_rows = "csv", None
            def render(self, rows): raise RuntimeError("render meledak")
            def write_chunk(self, t): pass
            def close(self): pass
        WRITERS["__boom"] = Boom
        try:
            E.export_rows(COLS, iter(make_rows(10_000)), tempfile.mkdtemp(), "o", "__boom")
            raise AssertionError("error dari worker tertelan")
        except RuntimeError as exc:
            assert "meledak" in str(exc)
        print("ok  error di worker thread diteruskan, tidak ditelan")
    finally:
        WRITERS.pop("__boom", None)
        E.render_workers = original


if __name__ == "__main__":
    print(f"free-threaded={E.FREE_THREADED}  workers default={E.render_workers()}\n")
    test_identical_output()
    test_non_renderable_formats_untouched()
    test_splitting_still_sequential()
    test_cancel_midway()
    test_worker_exception_propagates()
    print("\nsemua test render paralel lulus")
