"""Writer throughput benchmark: where an export actually spends its time.

    .venv/bin/python tests/bench_writers.py [rows]

Prints, per format, how long the writer takes and how much of that is pure
value formatting versus the backend and the filesystem. That split is the whole
question behind "should the engine be rewritten in something faster":

  * if formatting dominates, a faster language buys almost all of it;
  * if the backend (openpyxl, xlwt) dominates, only replacing that backend helps;
  * if the disk dominates, nothing in the engine matters.

Not a test: it asserts nothing and is never run by build_dmg.sh.
"""

from __future__ import annotations

import sys
import tempfile
import time
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from exporter.writers import WRITERS, Column, to_text  # noqa: E402

COLUMNS = [
    Column("kode_wilayah", "varchar"),
    Column("nama", "varchar"),
    Column("jumlah_jiwa", "bigint"),
    Column("bobot", "double"),
    Column("nilai_bantuan", "decimal"),
    Column("aktif", "boolean"),
    Column("tanggal_lahir", "date"),
    Column("diperbarui", "timestamp"),
    Column("catatan", "varchar"),
]

# BIFF8 has no streaming mode and a 65,536 row ceiling; xlwt past that is garbage.
ROW_CEILING = {"xls": 60_000}


def rows(count: int):
    """A realistic row: strings wide enough to matter, every native type the
    writers special-case, and one always-null column."""
    for i in range(count):
        yield (
            f"32.01.01.{2000 + i % 900}",
            f"Keluarga Penerima Manfaat Desa Sukamaju Kecamatan {i % 400}",
            1 + i % 9,
            (i % 1000) / 7.0,
            Decimal("750000.00"),
            i % 2 == 0,
            date(1980 + i % 40, 1 + i % 12, 1 + i % 28),
            datetime(2026, 1, 1, 12, 30, 45),
            None,
        )


def time_formatting(count: int) -> float:
    """Cost of turning values into text, with no writer and no disk involved."""
    start = time.perf_counter()
    cells = 0
    for row in rows(count):
        for value in row:
            to_text(value)
            cells += 1
    return time.perf_counter() - start


def bench(fmt: str, count: int, directory: Path) -> tuple[float, int]:
    writer_cls = WRITERS[fmt]
    path = directory / f"bench.{writer_cls.ext}"
    start = time.perf_counter()
    writer = writer_cls(path, COLUMNS, title="bench")
    for row in rows(count):
        writer.write_row(row)
    writer.close()
    elapsed = time.perf_counter() - start
    return elapsed, path.stat().st_size if path.exists() else 0


def main() -> int:
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 200_000
    print(f"{count:,} rows x {len(COLUMNS)} columns\n")

    format_only = time_formatting(count)
    print(f"pure to_text() formatting: {format_only:6.2f}s  "
          f"({count / format_only:,.0f} rows/s, no writer, no disk)\n")

    header = f"{'format':7} {'rows':>9} {'seconds':>9} {'rows/s':>10} {'MB':>8} {'MB/s':>8}  {'formatting share':>16}"
    print(header)
    print("-" * len(header))

    results: list[tuple[str, float]] = []
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        for fmt in WRITERS:
            n = min(count, ROW_CEILING.get(fmt, count))
            elapsed, size = bench(fmt, n, directory)
            results.append((fmt, elapsed))
            share = format_only * (n / count) / elapsed * 100 if elapsed else 0
            print(f"{fmt:7} {n:>9,} {elapsed:>9.2f} {n / elapsed:>10,.0f} "
                  f"{size / 1e6:>8.1f} {size / 1e6 / elapsed:>8.1f}  {share:>15.0f}%")

    print()
    slowest = max(results, key=lambda item: item[1])
    fastest = min(results, key=lambda item: item[1])
    print(f"slowest: {slowest[0]} at {slowest[1]:.2f}s   "
          f"fastest: {fastest[0]} at {fastest[1]:.2f}s   "
          f"spread: {slowest[1] / fastest[1]:.1f}x")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
