"""Local web UI. Same engine as the CLI, driven from a browser.

Exports run in a worker thread and report progress by polling, because a big
export outlives any sensible HTTP request timeout.
"""

from __future__ import annotations

import json
import logging
import os
import sys
import tempfile
import threading
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import keyring

from fastapi import FastAPI, HTTPException
from fastapi.responses import FileResponse, HTMLResponse
from pydantic import BaseModel, Field

from .export import bundle, run_export
from .source import TrinoConfig
from .writers import FORMAT_LABELS

log = logging.getLogger(__name__)
JOB_RETENTION = 50

CONNECTIONS_FILE = Path.home() / ".queryhive-connections.json"
QUERIES_FILE = Path.home() / ".queryhive-queries.json"


def _static_dir() -> Path:
    local = Path(__file__).parent / "static"
    if local.is_dir():
        return local
    # PyInstaller unpacks --add-data next to the frozen package
    return Path(getattr(sys, "_MEIPASS", ".")) / "exporter" / "static"


STATIC = _static_dir()


# ---------------------------------------------------------------------------
# Pydantic models
# ---------------------------------------------------------------------------

class ExportRequest(BaseModel):
    url: str = ""  # full connection string; the fields below override its parts
    host: str = ""
    port: int | None = None
    user: str = ""
    password: str = ""
    catalog: str = ""
    schema_: str = Field("", alias="schema")
    http_scheme: str = "http"
    verify: bool = True

    sql: str = ""
    format: str = "csv"
    name: str = "export"
    output_dir: str = ""

    batch_size: int = 10_000
    rows_per_file: int | None = None
    retries: int = 5

    delimiter: str = ""
    encoding: str = "utf-8"
    header: bool = True
    bom: bool = False
    null_text: str = ""
    jsonl: bool = False
    sql_table: str = ""
    sheet: str = "Sheet1"
    dbf_char_width: int = 254

    columns: list[str] = []  # empty = all columns; non-empty = project these columns only

    model_config = {"populate_by_name": True}

    def to_config(self) -> TrinoConfig:
        # If the URL field explicitly starts with https://, honour it regardless
        # of what the scheme dropdown says (common mismatch when restoring state).
        url = self.url.strip()
        scheme = self.http_scheme
        if url.lower().startswith("https://"):
            scheme = "https"

        parts = dict(
            host=self.host, port=self.port, user=self.user,
            password=self.password or None,
            catalog=self.catalog or None, schema=self.schema_ or None,
            http_scheme=scheme or None,
        )
        config = (
            TrinoConfig.from_url(url, **parts) if url
            else TrinoConfig(**{k: v for k, v in parts.items() if v is not None})
        )
        config.verify = self.verify
        return config

    def effective_sql(self) -> str:
        """Return SQL with optional column projection when user selected a subset."""
        sql = self.sql.strip().rstrip(";")
        if not self.columns:
            return sql
        cols = ", ".join(f'"{c}"' for c in self.columns)
        return f"SELECT {cols} FROM (\n{sql}\n) __export__"

    def to_opts(self) -> dict:
        return {
            "delimiter": self.delimiter or None,
            "encoding": self.encoding,
            "header": self.header,
            "bom": self.bom,
            "null_text": self.null_text,
            "jsonl": self.jsonl,
            "sql_table": self.sql_table or None,
            "sheet": self.sheet,
            "dbf_char_width": self.dbf_char_width,
            "title": self.name,
        }


class ConnectionRecord(BaseModel):
    name: str
    url: str = ""
    host: str = ""
    port: int | None = None
    user: str = ""
    password: str = ""
    catalog: str = ""
    schema_: str = Field("", alias="schema")
    http_scheme: str = "https"
    verify: bool = True
    model_config = {"populate_by_name": True}


class QueryRecord(BaseModel):
    id: str = ""
    name: str = "New Query"
    connection_name: str = ""
    sql: str = ""
    format: str = "csv"
    output_dir: str = ""


# ---------------------------------------------------------------------------
# Persistent storage helpers
# ---------------------------------------------------------------------------

def _load_connections() -> list[dict]:
    if CONNECTIONS_FILE.is_file():
        try:
            return json.loads(CONNECTIONS_FILE.read_text()).get("connections", [])
        except Exception:
            pass
    return []


def _save_connections(conns: list[dict]) -> None:
    CONNECTIONS_FILE.write_text(json.dumps({"connections": conns}, indent=2))


def _load_queries() -> list[dict]:
    if QUERIES_FILE.is_file():
        try:
            return json.loads(QUERIES_FILE.read_text()).get("queries", [])
        except Exception:
            pass
    return []


def _save_queries(queries: list[dict]) -> None:
    QUERIES_FILE.write_text(json.dumps({"queries": queries}, indent=2))


def _keyring_key(name: str) -> str:
    return f"conn:{name}"


# ---------------------------------------------------------------------------
# Export job machinery
# ---------------------------------------------------------------------------

@dataclass
class Job:
    id: str
    status: str = "running"  # running | done | error | cancelled
    rows: int = 0
    files: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    error: str = ""
    query_id: str | None = None
    download: str | None = None
    cancel_flag: bool = False

    def public(self) -> dict:
        return {
            "id": self.id, "status": self.status, "rows": self.rows,
            "files": self.files, "warnings": self.warnings, "error": self.error,
            "query_id": self.query_id, "download": bool(self.download),
        }


JOBS: dict[str, Job] = {}
JOBS_LOCK = threading.Lock()


def _run_job(job: Job, req: ExportRequest, out_dir: Path) -> None:
    try:
        result = run_export(
            req.to_config(), req.effective_sql(), out_dir, req.name or "export", req.format,
            batch_size=req.batch_size, retries=req.retries, opts=req.to_opts(),
            rows_per_file=req.rows_per_file,
            progress=lambda n: setattr(job, "rows", n),
            cancel=lambda: job.cancel_flag,
        )
        job.query_id = result.query_id
        job.warnings = result.warnings
        job.files = [str(p) for p in result.files]
        job.download = str(
            bundle(result.files, out_dir / f"{req.name or 'export'}.zip")
            if len(result.files) > 1 else result.files[0]
        )
        job.status = "cancelled" if result.cancelled else "done"
    except Exception as exc:  # surfaced verbatim in the UI
        log.exception("export job %s failed", job.id)
        job.status, job.error = "error", f"{type(exc).__name__}: {exc}"


# ---------------------------------------------------------------------------
# FastAPI app
# ---------------------------------------------------------------------------

def create_app() -> FastAPI:
    app = FastAPI(title="QueryHive", docs_url=None, redoc_url=None)

    @app.get("/", response_class=HTMLResponse)
    def index() -> str:
        return (STATIC / "index.html").read_text(encoding="utf-8")

    @app.get("/api/formats")
    def formats() -> list[dict]:
        return [{"id": k, "label": v} for k, v in FORMAT_LABELS.items()]

    @app.get("/api/defaults")
    def defaults() -> dict:
        downloads = Path.home() / "Downloads"
        return {"output_dir": str(downloads if downloads.is_dir() else Path.home())}

    # ------------------------------------------------------------------
    # Named connections (multi-connection support)
    # ------------------------------------------------------------------

    @app.get("/api/connections")
    def list_connections() -> list[dict]:
        conns = _load_connections()
        return [{k: v for k, v in c.items() if k != "password"} for c in conns]

    @app.get("/api/connections/{name}")
    def get_named_connection(name: str) -> dict:
        conns = _load_connections()
        for c in conns:
            if c.get("name") == name:
                result = dict(c)
                try:
                    pwd = keyring.get_password("queryhive", _keyring_key(name))
                    if pwd:
                        result["password"] = pwd
                except Exception as exc:
                    log.warning("failed to read keychain for %s: %s", name, exc)
                return result
        raise HTTPException(404, "connection not found")

    @app.post("/api/connections")
    def save_named_connection(rec: ConnectionRecord) -> dict:
        conns = _load_connections()
        data: dict[str, Any] = {
            "name": rec.name,
            "url": rec.url,
            "host": rec.host,
            "port": rec.port,
            "user": rec.user,
            "catalog": rec.catalog,
            "schema": rec.schema_,
            "http_scheme": rec.http_scheme,
            "verify": rec.verify,
        }
        idx = next((i for i, c in enumerate(conns) if c.get("name") == rec.name), None)
        if idx is not None:
            conns[idx] = data
        else:
            conns.append(data)
        try:
            _save_connections(conns)
        except Exception as exc:
            log.warning("failed to save connections: %s", exc)
        if rec.password and rec.name:
            try:
                keyring.set_password("queryhive", _keyring_key(rec.name), rec.password)
            except Exception as exc:
                log.warning("failed to save password to keychain: %s", exc)
        return {"ok": True}

    @app.delete("/api/connections/{name}")
    def delete_named_connection(name: str) -> dict:
        conns = [c for c in _load_connections() if c.get("name") != name]
        _save_connections(conns)
        try:
            keyring.delete_password("queryhive", _keyring_key(name))
        except Exception:
            pass
        return {"ok": True}

    @app.get("/api/connections/{name}/catalogs")
    def get_connection_catalogs(name: str) -> dict:
        from .source import QueryStream
        conn_dict = get_named_connection(name)
        req = ExportRequest(**conn_dict, sql="SHOW CATALOGS")
        try:
            with QueryStream(req.to_config(), "SHOW CATALOGS") as stream:
                catalogs = [row[0] for row in stream.rows()]
            return {"ok": True, "catalogs": catalogs}
        except Exception as exc:
            log.warning("failed to fetch catalogs for %s: %s", name, exc)
            return {"ok": False, "error": str(exc)}

    @app.get("/api/connections/{name}/schemas")
    def get_connection_schemas(name: str, catalog: str) -> dict:
        from .source import QueryStream
        conn_dict = get_named_connection(name)
        conn_dict["catalog"] = catalog
        sql = f'SHOW SCHEMAS FROM "{catalog}"'
        req = ExportRequest(**conn_dict, sql=sql)
        try:
            with QueryStream(req.to_config(), sql) as stream:
                schemas = [row[0] for row in stream.rows()]
            return {"ok": True, "schemas": schemas}
        except Exception as exc:
            log.warning("failed to fetch schemas for %s: %s", name, exc)
            return {"ok": False, "error": str(exc)}

    @app.get("/api/connections/{name}/tables")
    def get_connection_tables(name: str, catalog: str, schema: str) -> dict:
        from .source import QueryStream
        conn_dict = get_named_connection(name)
        conn_dict["catalog"] = catalog
        conn_dict["schema"] = schema
        sql = f'SHOW TABLES FROM "{catalog}"."{schema}"'
        req = ExportRequest(**conn_dict, sql=sql)
        try:
            with QueryStream(req.to_config(), sql) as stream:
                tables = [row[0] for row in stream.rows()]
            return {"ok": True, "tables": tables}
        except Exception as exc:
            log.warning("failed to fetch tables for %s: %s", name, exc)
            return {"ok": False, "error": str(exc)}

    # ------------------------------------------------------------------
    # Saved queries
    # ------------------------------------------------------------------

    @app.get("/api/queries")
    def list_queries() -> list[dict]:
        return _load_queries()

    @app.post("/api/queries")
    def save_query(q: QueryRecord) -> dict:
        queries = _load_queries()
        if not q.id:
            q = q.model_copy(update={"id": uuid.uuid4().hex[:8]})
        idx = next((i for i, qr in enumerate(queries) if qr.get("id") == q.id), None)
        data = q.model_dump()
        if idx is not None:
            queries[idx] = data
        else:
            queries.append(data)
        _save_queries(queries)
        return {"id": q.id, "ok": True}

    @app.delete("/api/queries/{query_id}")
    def delete_query(query_id: str) -> dict:
        queries = [q for q in _load_queries() if q.get("id") != query_id]
        _save_queries(queries)
        return {"ok": True}

    # ------------------------------------------------------------------
    # Preview (returns first N rows as JSON for the results table)
    # ------------------------------------------------------------------

    @app.post("/api/preview")
    def preview_query(req: ExportRequest) -> dict:
        from .source import QueryStream
        try:
            rows: list[list] = []
            columns: list[str] = []
            with QueryStream(req.to_config(), req.effective_sql(), batch_size=500) as stream:
                columns = [c.name for c in stream.columns]
                for i, row in enumerate(stream.rows()):
                    if i >= 500:
                        break
                    rows.append([None if v is None else str(v) for v in row])
            return {"ok": True, "columns": columns, "rows": rows}
        except Exception as exc:
            log.warning("preview failed: %s", exc)
            return {"ok": False, "error": f"{type(exc).__name__}: {exc}"}

    # ------------------------------------------------------------------
    # Directory browser (for Export Wizard folder picker)
    # ------------------------------------------------------------------

    @app.get("/api/browse")
    def browse(path: str = "") -> dict:
        import os as _os
        target = Path(path).expanduser().resolve() if path else Path.home()
        if not target.is_dir():
            raise HTTPException(400, f"Not a directory: {path}")
        try:
            entries = []
            for item in sorted(target.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower())):
                if item.name.startswith("."):
                    continue
                entries.append({"name": item.name, "path": str(item), "is_dir": item.is_dir()})
            parent = str(target.parent) if target.parent != target else None
            return {"path": str(target), "parent": parent, "entries": entries}
        except PermissionError:
            raise HTTPException(403, f"Permission denied: {path}")

    # ------------------------------------------------------------------
    # Test connection
    # ------------------------------------------------------------------

    @app.post("/api/test")
    def test_connection(req: ExportRequest) -> dict:
        from .source import QueryStream
        try:
            with QueryStream(req.to_config(), "SELECT 1") as stream:
                list(stream.rows())
            return {"ok": True}
        except Exception as exc:
            return {"ok": False, "error": f"{type(exc).__name__}: {exc}"}

    # ------------------------------------------------------------------
    # Export jobs
    # ------------------------------------------------------------------

    @app.post("/api/export")
    def start_export(req: ExportRequest) -> dict:
        if not req.sql.strip():
            raise HTTPException(400, "SQL is empty")
        out_dir = Path(req.output_dir).expanduser() if req.output_dir else Path(
            tempfile.mkdtemp(prefix="trino-export-")
        )
        try:
            out_dir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            raise HTTPException(400, f"cannot use output directory: {exc}") from exc

        job = Job(id=uuid.uuid4().hex[:12])
        with JOBS_LOCK:
            JOBS[job.id] = job
            for stale in list(JOBS)[:-JOB_RETENTION]:
                JOBS.pop(stale, None)
        threading.Thread(target=_run_job, args=(job, req, out_dir), daemon=True).start()
        return {"id": job.id}

    @app.get("/api/jobs/{job_id}")
    def job_status(job_id: str) -> dict:
        job = JOBS.get(job_id)
        if job is None:
            raise HTTPException(404, "unknown job")
        return job.public()

    @app.post("/api/jobs/{job_id}/cancel")
    def job_cancel(job_id: str) -> dict:
        job = JOBS.get(job_id)
        if job is None:
            raise HTTPException(404, "unknown job")
        job.cancel_flag = True
        return job.public()

    @app.get("/api/jobs/{job_id}/download")
    def job_download(job_id: str):
        job = JOBS.get(job_id)
        if job is None or not job.download:
            raise HTTPException(404, "nothing to download")
        path = Path(job.download)
        if not path.is_file():
            raise HTTPException(410, "file is gone")
        return FileResponse(path, filename=path.name, media_type="application/octet-stream")

    @app.post("/api/quit")
    def quit_app() -> dict:
        """The .app bundle has no menu bar, so the UI needs a way out."""
        threading.Timer(0.3, lambda: os._exit(0)).start()
        return {"ok": True}

    return app


app = create_app()
