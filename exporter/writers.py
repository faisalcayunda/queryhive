"""Streaming writers, one per export format.

Every writer takes rows one at a time and flushes to disk as it goes, so peak
memory stays flat no matter how many rows the query returns. The two Excel
writers are the exception: xlwt buffers the whole sheet (BIFF has no streaming
mode) and openpyxl's write_only mode buffers to a temp dir. Both are capped by
`max_rows`, so the export splits into parts before either gets huge.
"""

from __future__ import annotations

import csv
import io
import json
import re
import struct
from dataclasses import dataclass
from datetime import date, datetime, time
from decimal import Decimal
from html import escape
from pathlib import Path

DBF_MAX_RECORD_BYTES = 4000  # dBase III+ hard limit
XLS_MAX_ROWS = 65_536  # BIFF8 sheet limit, header included
XLSX_MAX_ROWS = 1_048_576


@dataclass(frozen=True)
class Column:
    name: str
    type: str = "varchar"


# --------------------------------------------------------------------------- #
# value formatting
# --------------------------------------------------------------------------- #

def to_text(value) -> str | None:
    """Canonical text form of a Trino value. None stays None: each writer
    decides how a NULL looks in its own format."""
    if value is None:
        return None
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float, Decimal)):
        return str(value)
    if isinstance(value, datetime):
        return value.isoformat(sep=" ")
    if isinstance(value, (date, time)):
        return value.isoformat()
    if isinstance(value, (bytes, bytearray)):
        return value.hex()
    if isinstance(value, str):
        return value
    return json.dumps(value, default=str, ensure_ascii=False)


def to_json_value(value):
    """JSON-native form: numbers stay numbers, everything exotic becomes text."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, Decimal):
        return float(value)
    if isinstance(value, (list, tuple)):
        return [to_json_value(v) for v in value]
    if isinstance(value, dict):
        return {str(k): to_json_value(v) for k, v in value.items()}
    return to_text(value)


class Writer:
    """Interface: open on construction, `write_row` per row, `close` once."""

    ext = "txt"
    max_rows: int | None = None  # data rows per file before the export splits

    def __init__(self, path: Path, columns: list[Column], **opts):
        self.path = Path(path)
        self.columns = columns
        self.opts = opts

    def write_row(self, row) -> None:  # pragma: no cover - interface
        raise NotImplementedError

    def close(self) -> None:  # pragma: no cover - interface
        raise NotImplementedError

    # -- optional: render off-thread ---------------------------------------
    # Formats whose rows are independent can turn a chunk of rows into text on
    # a worker thread, leaving the writer to only concatenate. That is what
    # makes a free-threaded build worth anything here: to_text and the csv/xml
    # formatting are pure Python, so the GIL would otherwise serialise them.
    #
    # Writers that carry per-row state (json's separators, sql's INSERT
    # batching) or a binary/stateful backend (xls, xlsx, dbf) leave this False
    # and keep the row-at-a-time path.
    renderable = False

    def render(self, rows) -> str:  # pragma: no cover - interface
        """Format `rows`. MUST NOT touch writer state: called from N threads."""
        raise NotImplementedError

    def write_chunk(self, text: str) -> None:
        self._fh.write(text)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


# --------------------------------------------------------------------------- #
# text / csv
# --------------------------------------------------------------------------- #

class DelimitedWriter(Writer):
    """Shared body for .txt and .csv. They differ only in defaults."""

    ext = "txt"
    default_delimiter = "\t"
    default_quoting = csv.QUOTE_MINIMAL

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        encoding = opts.get("encoding", "utf-8")
        self.null = opts.get("null_text", "")
        self._fh = open(
            self.path,
            "w",
            newline="",
            encoding=encoding,
            errors="replace",
        )
        if opts.get("bom") and encoding.lower().replace("-", "") == "utf8":
            self._fh.write("﻿")  # makes Excel open UTF-8 correctly
        self._dialect = dict(
            delimiter=opts.get("delimiter") or self.default_delimiter,
            quotechar=opts.get("quotechar", '"'),
            quoting=opts.get("quoting", self.default_quoting),
            lineterminator=opts.get("lineterminator", "\r\n"),
        )
        self._csv = csv.writer(self._fh, **self._dialect)
        if opts.get("header", True):
            self._csv.writerow([c.name for c in columns])

    def write_row(self, row):
        self._csv.writerow([self.null if v is None else to_text(v) for v in row])

    renderable = True

    def render(self, rows):
        buf = io.StringIO()
        writer = csv.writer(buf, **self._dialect)
        null = self.null
        for row in rows:
            writer.writerow([null if v is None else to_text(v) for v in row])
        return buf.getvalue()

    def close(self):
        self._fh.close()


class TextWriter(DelimitedWriter):
    ext = "txt"
    default_delimiter = "\t"


class CsvWriter(DelimitedWriter):
    ext = "csv"
    default_delimiter = ","
    default_quoting = csv.QUOTE_MINIMAL


# --------------------------------------------------------------------------- #
# json
# --------------------------------------------------------------------------- #

class JsonWriter(Writer):
    ext = "json"

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        self.names = [c.name for c in columns]
        self.lines = bool(opts.get("jsonl"))  # newline-delimited instead of array
        self.indent = opts.get("indent", 2)
        self._fh = open(self.path, "w", encoding="utf-8")
        self._first = True
        if not self.lines:
            self._fh.write("[\n")

    def write_row(self, row):
        obj = {n: to_json_value(v) for n, v in zip(self.names, row)}
        if self.lines:
            self._fh.write(json.dumps(obj, ensure_ascii=False, default=str) + "\n")
            return
        if not self._first:
            self._fh.write(",\n")
        self._first = False
        text = json.dumps(obj, ensure_ascii=False, default=str, indent=self.indent)
        self._fh.write("".join("  " + ln for ln in text.splitlines(keepends=True)))

    def close(self):
        if not self.lines:
            self._fh.write("\n]\n" if not self._first else "]\n")
        self._fh.close()


# --------------------------------------------------------------------------- #
# xml
# --------------------------------------------------------------------------- #

_XML_BAD_START = re.compile(r"^[^A-Za-z_]")
_XML_BAD_CHAR = re.compile(r"[^A-Za-z0-9_.\-]")
_XML_CONTROL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")


def xml_tag(name: str) -> str:
    tag = _XML_BAD_CHAR.sub("_", name)
    return "_" + tag if _XML_BAD_START.match(tag) else tag


class XmlWriter(Writer):
    ext = "xml"

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        self.tags = [xml_tag(c.name) for c in columns]
        self.root = opts.get("xml_root", "RECORDS")
        self.record = opts.get("xml_record", "RECORD")
        self._fh = open(self.path, "w", encoding="utf-8")
        self._fh.write(f'<?xml version="1.0" encoding="UTF-8"?>\n<{self.root}>\n')

    def write_row(self, row):
        out = [f"  <{self.record}>\n"]
        for tag, value in zip(self.tags, row):
            if value is None:
                out.append(f'    <{tag} xsi:nil="true"/>\n')
                continue
            text = _XML_CONTROL.sub("", to_text(value))
            out.append(f"    <{tag}>{escape(text)}</{tag}>\n")
        out.append(f"  </{self.record}>\n")
        self._fh.write("".join(out))

    renderable = True

    def render(self, rows):
        out = []
        tags, record = self.tags, self.record
        for row in rows:
            out.append(f"  <{record}>\n")
            for tag, value in zip(tags, row):
                if value is None:
                    out.append(f'    <{tag} xsi:nil="true"/>\n')
                    continue
                text = _XML_CONTROL.sub("", to_text(value))
                out.append(f"    <{tag}>{escape(text)}</{tag}>\n")
            out.append(f"  </{record}>\n")
        return "".join(out)

    def close(self):
        self._fh.write(f"</{self.root}>\n")
        self._fh.close()


# --------------------------------------------------------------------------- #
# html
# --------------------------------------------------------------------------- #

_HTML_HEAD = """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>{title}</title>
<style>
 body{{font:13px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;margin:24px;color:#1a1a1a}}
 table{{border-collapse:collapse;font-variant-numeric:tabular-nums}}
 th,td{{border:1px solid #d8d8d8;padding:4px 9px;text-align:left;vertical-align:top}}
 th{{background:#f2f2f2;position:sticky;top:0;font-weight:600}}
 tr:nth-child(even) td{{background:#fafafa}}
 td.null{{color:#999;font-style:italic}}
</style></head><body>
<h2>{title}</h2>
<table><thead><tr>{head}</tr></thead><tbody>
"""


class HtmlWriter(Writer):
    ext = "html"

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        self._fh = open(self.path, "w", encoding="utf-8")
        head = "".join(f"<th>{escape(c.name)}</th>" for c in columns)
        title = escape(opts.get("title") or self.path.stem)
        self._fh.write(_HTML_HEAD.format(title=title, head=head))

    def write_row(self, row):
        cells = []
        for value in row:
            if value is None:
                cells.append('<td class="null">NULL</td>')
            else:
                cells.append(f"<td>{escape(to_text(value))}</td>")
        self._fh.write("<tr>" + "".join(cells) + "</tr>\n")

    renderable = True

    def render(self, rows):
        out = []
        for row in rows:
            out.append("<tr>")
            for value in row:
                if value is None:
                    out.append('<td class="null">NULL</td>')
                else:
                    out.append(f"<td>{escape(to_text(value))}</td>")
            out.append("</tr>\n")
        return "".join(out)

    def close(self):
        self._fh.write("</tbody></table></body></html>\n")
        self._fh.close()


# --------------------------------------------------------------------------- #
# sql
# --------------------------------------------------------------------------- #

class SqlWriter(Writer):
    """INSERT statements, several rows per statement to keep replay fast."""

    ext = "sql"

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        quote = opts.get("sql_ident_quote", '"')
        self.rows_per_insert = max(1, int(opts.get("sql_rows_per_insert", 200)))
        table = opts.get("sql_table") or self.path.stem
        cols = ", ".join(f"{quote}{c.name.replace(quote, quote * 2)}{quote}" for c in columns)
        self._prefix = f"INSERT INTO {quote}{table}{quote} ({cols}) VALUES\n"
        self._fh = open(self.path, "w", encoding="utf-8")
        self._buf: list[str] = []

    @staticmethod
    def literal(value) -> str:
        if value is None:
            return "NULL"
        if isinstance(value, bool):
            return "TRUE" if value else "FALSE"
        if isinstance(value, (int, float, Decimal)):
            return str(value)
        if isinstance(value, (bytes, bytearray)):
            return f"X'{value.hex()}'"
        return "'" + to_text(value).replace("'", "''") + "'"

    def write_row(self, row):
        self._buf.append("(" + ", ".join(self.literal(v) for v in row) + ")")
        if len(self._buf) >= self.rows_per_insert:
            self._flush()

    def _flush(self):
        if not self._buf:
            return
        self._fh.write(self._prefix + ",\n".join(self._buf) + ";\n")
        self._buf.clear()

    def close(self):
        self._flush()
        self._fh.close()


# --------------------------------------------------------------------------- #
# excel
# --------------------------------------------------------------------------- #

_XLSX_ILLEGAL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")


def _excel_value(value, keep_datetime=True):
    """Native cell value where Excel can hold one, text otherwise."""
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, Decimal):
        return float(value)
    if isinstance(value, datetime):
        # tz-aware datetimes are not representable in a cell; keep the offset as text
        return value if (keep_datetime and value.tzinfo is None) else to_text(value)
    if isinstance(value, (date, time)) and keep_datetime:
        return value
    text = to_text(value)
    return _XLSX_ILLEGAL.sub("", text)[:32767]


class XlsxWriter(Writer):
    ext = "xlsx"
    max_rows = XLSX_MAX_ROWS - 1

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        from openpyxl import Workbook

        self._wb = Workbook(write_only=True)
        self._ws = self._wb.create_sheet(title=(opts.get("sheet") or "Sheet1")[:31])
        self._ws.append([c.name for c in columns])

    def write_row(self, row):
        self._ws.append([_excel_value(v) for v in row])

    def close(self):
        self._wb.save(self.path)
        self._wb.close()


class XlsWriter(Writer):
    ext = "xls"
    max_rows = XLS_MAX_ROWS - 1

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        import xlwt

        if len(columns) > 256:
            raise ValueError(
                f"xls supports 256 columns, query returned {len(columns)}. Use xlsx."
            )
        self._wb = xlwt.Workbook(encoding="utf-8")
        self._ws = self._wb.add_sheet((opts.get("sheet") or "Sheet1")[:31])
        bold = xlwt.easyxf("font: bold on")
        self._styles = {
            datetime: xlwt.easyxf(num_format_str="YYYY-MM-DD HH:MM:SS"),
            date: xlwt.easyxf(num_format_str="YYYY-MM-DD"),
            time: xlwt.easyxf(num_format_str="HH:MM:SS"),
        }
        self._plain = xlwt.Style.default_style
        for col, column in enumerate(columns):
            self._ws.write(0, col, column.name, bold)
        self._row = 1

    def write_row(self, row):
        for col, raw in enumerate(row):
            value = _excel_value(raw)
            style = self._styles.get(type(value), self._plain)
            self._ws.write(self._row, col, value, style)
        self._row += 1

    def close(self):
        self._wb.save(str(self.path))


# --------------------------------------------------------------------------- #
# dbf
# --------------------------------------------------------------------------- #

_NUMERIC_TRINO_TYPES = (
    "tinyint", "smallint", "integer", "int", "bigint",
    "real", "double", "decimal",
)


def _dbf_field_names(columns: list[Column]) -> list[bytes]:
    """DBF names: 10 chars, A-Z0-9_, unique."""
    names, seen = [], set()
    for column in columns:
        base = re.sub(r"[^A-Z0-9_]", "_", column.name.upper())[:10] or "FIELD"
        name, suffix = base, 1
        while name in seen:
            tail = str(suffix)
            name = base[: 10 - len(tail)] + tail
            suffix += 1
        seen.add(name)
        names.append(name.encode("ascii", "replace"))
    return names


class DbfWriter(Writer):
    """dBase III+ writer. No dependency: the format is a fixed-width binary
    layout that streams naturally, and the pypi options are read-oriented.

    Char fields are fixed width, so wide text is truncated (`truncated` counts
    it). Width shrinks automatically when the row would exceed the 4000-byte
    record limit.
    """

    ext = "dbf"

    def __init__(self, path, columns, **opts):
        super().__init__(path, columns, **opts)
        if len(columns) > 255:
            raise ValueError(f"dbf supports 255 fields, query returned {len(columns)}")
        self.encoding = opts.get("dbf_encoding", "cp1252")
        self.truncated = 0
        self._names = _dbf_field_names(columns)
        self._fields = self._plan_fields(columns, int(opts.get("dbf_char_width", 254)))
        self._record_len = 1 + sum(f[2] for f in self._fields)
        self._count = 0
        self._fh = open(self.path, "wb")
        self._write_header()

    def _plan_fields(self, columns, char_width):
        """(kind, decimals, width) per column, char width scaled to fit 4000 bytes."""
        kinds = []
        for column in columns:
            base = (column.type or "varchar").split("(")[0].strip().lower()
            if base == "boolean":
                kinds.append(("L", 0, 1))
            elif base == "date":
                kinds.append(("D", 0, 8))
            elif base in _NUMERIC_TRINO_TYPES:
                decimals = 0 if base in ("tinyint", "smallint", "integer", "int", "bigint") else 6
                kinds.append(("N", decimals, 20))
            else:
                kinds.append(("C", 0, None))

        char_count = sum(1 for k, _, w in kinds if w is None)
        if char_count:
            fixed = 1 + sum(w for _, _, w in kinds if w is not None)
            budget = (DBF_MAX_RECORD_BYTES - fixed) // char_count
            width = max(1, min(254, char_width, budget))
            if width < 1:
                raise ValueError("too many columns for a dbf record (4000 byte limit)")
            kinds = [(k, d, width if w is None else w) for k, d, w in kinds]
        return kinds

    def _write_header(self):
        today = date.today()
        header_len = 32 + 32 * len(self._fields) + 1
        self._fh.write(struct.pack(
            "<4BIHH2xBB12xBB2x",
            0x03, today.year - 1900, today.month, today.day,
            0, header_len, self._record_len,
            0, 0,   # incomplete transaction, encryption
            0, 0x03,  # mdx flag, language driver = cp1252
        ))
        for name, (kind, decimals, width) in zip(self._names, self._fields):
            self._fh.write(struct.pack(
                "<11sc4xBB14x", name, kind.encode("ascii"), width, decimals
            ))
        self._fh.write(b"\x0d")

    def _encode(self, value, kind, decimals, width) -> bytes:
        if value is None:
            return b"?" if kind == "L" else b" " * width
        if kind == "L":
            return b"T" if value else b"F"
        if kind == "D":
            if isinstance(value, (date, datetime)):
                return value.strftime("%Y%m%d").encode("ascii")
            digits = re.sub(r"[^0-9]", "", to_text(value))[:8]
            return digits.encode("ascii").ljust(8, b" ")
        if kind == "N":
            try:
                text = f"{Decimal(str(value)):.{decimals}f}" if decimals else str(int(value))
            except (ValueError, ArithmeticError, TypeError):
                return b" " * width
            if len(text) > width:
                self.truncated += 1
                return b"*" * width
            return text.rjust(width).encode("ascii")
        raw = to_text(value).replace("\r", " ").replace("\n", " ")
        out = raw.encode(self.encoding, "replace")
        if len(out) > width:
            self.truncated += 1
            out = out[:width]
        return out.ljust(width, b" ")

    def write_row(self, row):
        parts = [b" "]  # not-deleted flag
        for value, (kind, decimals, width) in zip(row, self._fields):
            parts.append(self._encode(value, kind, decimals, width))
        self._fh.write(b"".join(parts))
        self._count += 1

    def close(self):
        self._fh.write(b"\x1a")  # EOF marker
        self._fh.seek(4)
        self._fh.write(struct.pack("<I", self._count))
        self._fh.close()


# --------------------------------------------------------------------------- #

WRITERS: dict[str, type[Writer]] = {
    "txt": TextWriter,
    "csv": CsvWriter,
    "json": JsonWriter,
    "xml": XmlWriter,
    "html": HtmlWriter,
    "sql": SqlWriter,
    "xls": XlsWriter,
    "xlsx": XlsxWriter,
    "dbf": DbfWriter,
}

FORMAT_LABELS = {
    "txt": "Text file (*.txt)",
    "csv": "CSV file (*.csv)",
    "json": "JSON file (*.json)",
    "xml": "XML file (*.xml)",
    "html": "HTML file (*.htm;*.html)",
    "sql": "SQL script file (*.sql)",
    "xls": "Excel file (*.xls)",
    "xlsx": "Excel file (2007 or later) (*.xlsx)",
    "dbf": "DBase file (*.dbf)",
}
