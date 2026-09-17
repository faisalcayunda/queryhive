"""Command line entry point: queryhive-export."""

from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path

from .export import bundle, run_export
from .source import TrinoConfig
from .writers import FORMAT_LABELS, WRITERS

def get_saved_config() -> dict:
    import json
    import keyring
    # Check new queryhive config first, then fall back to old trino-exporter file (migration)
    config_file = Path.home() / ".queryhive-connections.json"
    old_file    = Path.home() / ".trino-exporter.json"
    config = {}
    if old_file.is_file() and not config_file.is_file():
        try:
            config = json.loads(old_file.read_text())
        except Exception:
            pass
    if config.get("host") and config.get("user"):
        # Try queryhive keychain first, then old trino-exporter key
        try:
            import keyring as _kr
            pwd = (_kr.get_password("queryhive", f"{config['host']}:{config['user']}") or
                   _kr.get_password("trino-exporter", f"{config['host']}:{config['user']}"))
            if pwd:
                config["password"] = pwd
        except Exception:
            pass
    return config


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="queryhive-export",
        description="QueryHive — Export a SQL query to txt/csv/json/xml/html/sql/xls/xlsx/dbf.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="formats:\n  " + "\n  ".join(f"{k:5s} {v}" for k, v in FORMAT_LABELS.items()),
    )
    c = p.add_argument_group("connection")
    c.add_argument("--url", default=os.getenv("TRINO_URL"),
                   help="whole connection in one string, e.g. "
                        "https://user@trino.internal:8443/hive/analytics (env: TRINO_URL). "
                        "Individual flags below override any part of it.")
    c.add_argument("--host", default=os.getenv("TRINO_HOST"))
    c.add_argument("--port", type=int, default=int(os.getenv("TRINO_PORT", "0")) or None)
    c.add_argument("--user", default=os.getenv("TRINO_USER"))
    c.add_argument("--password", default=os.getenv("TRINO_PASSWORD"),
                   help="enables BasicAuth over https (env: TRINO_PASSWORD)")
    c.add_argument("--catalog", default=os.getenv("TRINO_CATALOG"))
    c.add_argument("--schema", default=os.getenv("TRINO_SCHEMA"))
    c.add_argument("--https", action="store_true", help="force https")
    c.add_argument("--insecure", action="store_true", help="skip TLS verification")
    c.add_argument("--session", action="append", default=[], metavar="K=V",
                   help="session property, repeatable")

    q = p.add_argument_group("query")
    g = q.add_mutually_exclusive_group(required=True)
    g.add_argument("-q", "--query")
    g.add_argument("-f", "--file", type=Path, help="read SQL from a file ('-' for stdin)")

    o = p.add_argument_group("output")
    o.add_argument("-F", "--format", default="csv", choices=list(WRITERS))
    o.add_argument("-o", "--out-dir", type=Path, default=Path.cwd())
    o.add_argument("-n", "--name", default="export", help="base filename without extension")
    o.add_argument("--zip", action="store_true", help="zip the result files")

    b = p.add_argument_group("batching")
    b.add_argument("--batch-size", type=int, default=10_000,
                   help="rows per fetch from Trino (default: 10000)")
    b.add_argument("--rows-per-file", type=int, default=None,
                   help="split output every N rows")
    b.add_argument("--retries", type=int, default=5,
                   help="retries on transient fetch errors (default: 5)")

    f = p.add_argument_group("format options")
    f.add_argument("--delimiter", help="txt/csv field separator")
    f.add_argument("--encoding", default="utf-8", help="txt/csv encoding")
    f.add_argument("--no-header", action="store_true")
    f.add_argument("--bom", action="store_true", help="UTF-8 BOM for Excel-friendly csv")
    f.add_argument("--null-text", default="", help="how NULL is rendered in txt/csv")
    f.add_argument("--jsonl", action="store_true", help="newline-delimited json")
    f.add_argument("--sql-table", help="target table name for the sql format")
    f.add_argument("--sheet", default="Sheet1", help="xls/xlsx sheet name")
    f.add_argument("--dbf-char-width", type=int, default=254)
    f.add_argument("--dbf-encoding", default="cp1252")

    p.add_argument("-v", "--verbose", action="store_true")
    return p


def _format_opts(args) -> dict:
    return {
        "delimiter": args.delimiter,
        "encoding": args.encoding,
        "header": not args.no_header,
        "bom": args.bom,
        "null_text": args.null_text,
        "jsonl": args.jsonl,
        "sql_table": args.sql_table,
        "sheet": args.sheet,
        "dbf_char_width": args.dbf_char_width,
        "dbf_encoding": args.dbf_encoding,
        "title": args.name,
    }


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s %(message)s",
    )

    if args.file:
        sql = sys.stdin.read() if str(args.file) == "-" else args.file.read_text()
    else:
        sql = args.query

    session = {}
    for item in args.session:
        key, _, value = item.partition("=")
        if not value:
            print(f"bad --session {item!r}, expected KEY=VALUE", file=sys.stderr)
            return 2
        session[key.strip()] = value.strip()

    overrides = dict(
        host=args.host, port=args.port, user=args.user, password=args.password,
        catalog=args.catalog, schema=args.schema,
        http_scheme="https" if args.https else None,
    )
    try:
        if args.url:
            config = TrinoConfig.from_url(args.url, **overrides)
        elif args.host:
            config = TrinoConfig(**{k: v for k, v in overrides.items() if v is not None})
        else:
            saved = get_saved_config()
            if saved and "host" in saved:
                merged = {**saved, **{k: v for k, v in overrides.items() if v is not None}}
                valid_keys = {"host", "port", "user", "password", "catalog", "schema", "http_scheme", "verify"}
                config = TrinoConfig(**{k: v for k, v in merged.items() if k in valid_keys})
            else:
                print("need --url or --host (or TRINO_URL / TRINO_HOST)", file=sys.stderr)
                return 2
    except ValueError as exc:
        print(f"bad connection settings: {exc}", file=sys.stderr)
        return 2

    if config.password and not (args.port or args.url or args.https):
        config.port = 443  # password implied https, so the default port moves too
    config.user = config.user or os.getenv("USER") or "trino"
    config.verify = not args.insecure
    config.session_properties = session

    def progress(rows: int) -> None:
        print(f"\r  {rows:,} rows", end="", file=sys.stderr, flush=True)

    try:
        result = run_export(
            config, sql, args.out_dir, args.name, args.format,
            batch_size=args.batch_size, retries=args.retries,
            opts=_format_opts(args), rows_per_file=args.rows_per_file,
            progress=progress,
        )
    except Exception as exc:
        print(f"\nexport failed: {exc}", file=sys.stderr)
        return 1

    print(file=sys.stderr)
    files = result.files
    if args.zip and files:
        files = [bundle(files, args.out_dir / f"{args.name}.zip")]
    for warning in result.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for path in files:
        print(f"{path}  ({path.stat().st_size:,} bytes)")
    print(f"{result.rows:,} rows exported", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
