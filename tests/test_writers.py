"""Round-trip check for every writer plus the part-splitting logic.

Run: python tests/test_writers.py
"""

import csv
import json
import struct
import sys
import tempfile
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from exporter.export import export_rows  # noqa: E402
from exporter.writers import WRITERS, Column  # noqa: E402

COLUMNS = [
    Column("id", "bigint"),
    Column("nama lengkap", "varchar"),
    Column("harga", "decimal(18,2)"),
    Column("aktif", "boolean"),
    Column("tanggal", "date"),
    Column("dibuat", "timestamp"),
    Column("meta", "map(varchar,varchar)"),
]
ROWS = [
    [1, "Budi 'Santoso'", Decimal("1250.75"), True, date(2026, 1, 31),
     datetime(2026, 1, 31, 8, 30, 0), {"kota": "Jakarta"}],
    [2, 'Ani "Wijaya"\nbaris dua', Decimal("-3.5"), False, date(1999, 12, 25),
     datetime(1999, 12, 25, 23, 59, 59), None],
    [3, None, None, None, None, None, ["a", "b"]],
]


def run(fmt, tmp, **opts):
    result = export_rows(COLUMNS, ROWS, tmp, "t", fmt, opts=opts)
    assert result.rows == 3, (fmt, result.rows)
    assert len(result.files) == 1
    path = result.files[0]
    assert path.stat().st_size > 0, fmt
    return path


def test_csv(tmp):
    rows = list(csv.reader(run("csv", tmp).open(newline="", encoding="utf-8")))
    assert rows[0] == [c.name for c in COLUMNS]
    assert rows[1][1] == "Budi 'Santoso'"
    assert rows[1][2] == "1250.75"
    assert rows[1][3] == "true"
    assert rows[2][1] == 'Ani "Wijaya"\nbaris dua'  # embedded newline survives quoting
    assert rows[3][1] == "" and rows[3][2] == ""    # NULL -> empty


def test_txt(tmp):
    lines = run("txt", tmp).read_text(encoding="utf-8").splitlines()
    assert lines[0].split("\t")[0] == "id"
    assert run("txt", tmp, delimiter="|", null_text="\\N").read_text().count("\\N") == 6


def test_json(tmp):
    data = json.loads(run("json", tmp).read_text(encoding="utf-8"))
    assert len(data) == 3
    assert data[0]["harga"] == 1250.75 and data[0]["aktif"] is True
    assert data[0]["meta"] == {"kota": "Jakarta"}
    assert data[2]["nama lengkap"] is None
    lines = run("json", tmp, jsonl=True).read_text().strip().splitlines()
    assert len(lines) == 3 and json.loads(lines[1])["id"] == 2


def test_xml(tmp):
    import xml.etree.ElementTree as ET

    text = run("xml", tmp).read_text(encoding="utf-8").replace(' xsi:nil="true"', "")
    root = ET.fromstring(text)
    records = list(root)
    assert len(records) == 3
    assert records[0].find("nama_lengkap").text == "Budi 'Santoso'"  # space -> _
    assert records[2].find("harga").text is None


def test_html(tmp):
    text = run("html", tmp).read_text(encoding="utf-8")
    assert text.count("<tr>") == 4  # header + 3 data rows
    assert "&#x27;" in text or "&quot;" in text  # quotes escaped
    assert "<script" not in text.lower()


def test_sql(tmp):
    text = run("sql", tmp, sql_table="dim_orang").read_text(encoding="utf-8")
    assert 'INSERT INTO "dim_orang" ("id", "nama lengkap"' in text
    assert "'Budi ''Santoso'''" in text  # single quote doubled
    assert "TRUE" in text and "NULL" in text


def test_xlsx(tmp):
    from openpyxl import load_workbook

    ws = load_workbook(run("xlsx", tmp)).active
    values = list(ws.values)
    assert values[0][0] == "id"
    assert values[1][0] == 1 and values[1][2] == 1250.75
    assert values[1][4] == datetime(2026, 1, 31, 0, 0)
    assert values[3][1] is None


def test_xls(tmp):
    path = run("xls", tmp)
    assert path.read_bytes()[:2] == b"\xd0\xcf"  # OLE2 compound file magic


def test_dbf(tmp):
    raw = run("dbf", tmp).read_bytes()
    version, _, _, _, count, header_len, record_len = struct.unpack("<4BIHH", raw[:12])
    assert version == 0x03
    assert count == 3
    assert header_len == 32 + 32 * len(COLUMNS) + 1
    names = [raw[32 + 32 * i: 32 + 32 * i + 11].rstrip(b"\x00").decode() for i in range(len(COLUMNS))]
    assert names[:3] == ["ID", "NAMA_LENGK", "HARGA"]  # upper, 10 chars
    kinds = [raw[32 + 32 * i + 11: 32 + 32 * i + 12] for i in range(len(COLUMNS))]
    assert kinds == [b"N", b"C", b"N", b"L", b"D", b"C", b"C"]
    body = raw[header_len:]
    assert len(body) == count * record_len + 1  # + EOF marker
    first = body[:record_len]
    assert first[0:1] == b" "  # not deleted
    assert b"Budi 'Santoso'" in first
    assert b"20260131" in first
    assert body[-1:] == b"\x1a"


def test_splitting(tmp):
    many = ([i, f"r{i}", None, None, None, None, None] for i in range(250))
    result = export_rows(COLUMNS, many, tmp, "big", "csv", rows_per_file=100)
    assert result.rows == 250
    assert [p.name for p in result.files] == ["big_part01.csv", "big_part02.csv", "big_part03.csv"]
    assert all(p.exists() for p in result.files)
    last = result.files[-1].read_text().strip().splitlines()
    assert len(last) == 51  # header + 50 rows


def test_cancel(tmp):
    state = {"n": 0}

    def rows():
        for i in range(1000):
            state["n"] = i
            yield [i, "x", None, None, None, None, None]

    result = export_rows(COLUMNS, rows(), tmp, "c", "csv",
                         cancel=lambda: state["n"] >= 10)
    assert result.cancelled and result.rows == 10
    assert result.files[0].exists()  # partial file is still valid


def main():
    with tempfile.TemporaryDirectory() as tmp:
        tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
        for fn in tests:
            fn(Path(tmp))
            print(f"  ok  {fn.__name__}")
    assert set(WRITERS) == {"txt", "csv", "json", "xml", "html", "sql", "xls", "xlsx", "dbf"}
    print(f"\n{len(tests)} passed")


if __name__ == "__main__":
    main()
