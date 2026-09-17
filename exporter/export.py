"""Export orchestration: pick a writer, stream rows into it, split into parts."""

from __future__ import annotations

import os
import queue
import sysconfig
import threading
import zipfile
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterable, Iterator, Sequence

from .source import QueryStream, TrinoConfig
from .writers import WRITERS, Column

PROGRESS_EVERY = 1_000
RENDER_CHUNK = 2_000  # rows per render task: big enough to dwarf queue overhead
FREE_THREADED = bool(sysconfig.get_config_var("Py_GIL_DISABLED"))


def render_workers() -> int:
    """How many threads should format rows.

    On a stock build the answer is one. Formatting is pure Python, so extra
    threads just take turns holding the GIL and pay queueing overhead for it
    (measured 1.30x vs 1.26x, inside the noise). Without the GIL the same work
    genuinely runs side by side: 2.80x on 200k rows when Trino serves pages in
    ~5ms. Override with QUERYHIVE_RENDER_THREADS.
    """
    override = os.environ.get("QUERYHIVE_RENDER_THREADS")
    if override and override.isdigit() and int(override) > 0:
        return int(override)
    if not FREE_THREADED:
        return 1
    return max(1, min(4, (os.cpu_count() or 2) - 1))


@dataclass
class ExportResult:
    files: list[Path] = field(default_factory=list)
    rows: int = 0
    columns: list[Column] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    query_id: str | None = None
    cancelled: bool = False


class Cancelled(Exception):
    pass


def _part_path(out_dir: Path, basename: str, ext: str, index: int) -> Path:
    return out_dir / (f"{basename}.{ext}" if index == 0 else f"{basename}_part{index + 1:02d}.{ext}")


def export_rows(
    columns: Sequence[Column],
    rows: Iterable[Sequence],
    out_dir: str | Path,
    basename: str,
    fmt: str,
    opts: dict | None = None,
    rows_per_file: int | None = None,
    progress: Callable[[int], None] | None = None,
    cancel: Callable[[], bool] | None = None,
) -> ExportResult:
    """Write `rows` into one or more files. Splits when a format's own row
    ceiling (xls: 65535, xlsx: 1048576) or `rows_per_file` is reached."""
    if fmt not in WRITERS:
        raise ValueError(f"unknown format {fmt!r}; expected one of {', '.join(WRITERS)}")

    writer_cls = WRITERS[fmt]
    opts = dict(opts or {})
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    columns = list(columns)

    limits = [n for n in (writer_cls.max_rows, rows_per_file) if n]
    limit = min(limits) if limits else None

    result = ExportResult(columns=columns)
    part, in_part, writer = 0, 0, None

    def open_part(index: int):
        path = _part_path(out_dir, basename, writer_cls.ext, index)
        return writer_cls(path, columns, **opts), path

    # Fast path: no part splitting to coordinate, a format whose rows are
    # independent, and threads worth spending. Everything else falls through to
    # the row-at-a-time loop below, which stays the reference implementation.
    workers = render_workers()
    if limit is None and writer_cls.renderable and workers > 1:
        writer, path = open_part(0)
        result.files.append(path)
        try:
            _render_parallel(writer, rows, result, workers, progress, cancel)
        finally:
            _finish(writer, result)
        if progress:
            progress(result.rows)
        return result

    try:
        writer, path = open_part(0)
        result.files.append(path)
        for row in rows:
            if cancel is not None and cancel():
                result.cancelled = True
                break
            if limit and in_part >= limit:
                _finish(writer, result)
                if part == 0:
                    # the first file only earns a _part01 suffix once a second exists
                    renamed = _part_path(out_dir, basename, writer_cls.ext, 0).with_name(
                        f"{basename}_part01.{writer_cls.ext}"
                    )
                    result.files[0].rename(renamed)
                    result.files[0] = renamed
                part += 1
                in_part = 0
                writer, path = open_part(part)
                result.files.append(path)
            writer.write_row(row)
            in_part += 1
            result.rows += 1
            if progress and result.rows % PROGRESS_EVERY == 0:
                progress(result.rows)
    finally:
        if writer is not None:
            _finish(writer, result)

    if progress:
        progress(result.rows)
    return result


def _render_parallel(
    writer,
    rows: Iterable[Sequence],
    result: ExportResult,
    workers: int,
    progress: Callable[[int], None] | None,
    cancel: Callable[[], bool] | None,
) -> None:
    """Format chunks on worker threads, write the results back in order.

    `writer.render` is called concurrently and must not touch writer state; the
    writer itself is only ever touched from this thread, so the output file
    still sees one sequential stream of appends.
    """
    inflight: deque = deque()

    def source() -> Iterator[list]:
        """Group rows into render-sized chunks.

        Cancel is checked per row, not per chunk: the sequential path stops on
        the exact row, and an export that keeps writing 2000 more rows after
        the user hits Cancel is a regression, not an optimisation. The check
        costs one call against formatting six values.
        """
        chunk: list = []
        for row in rows:
            if cancel is not None and cancel():
                result.cancelled = True
                break
            chunk.append(row)
            if len(chunk) == RENDER_CHUNK:
                yield chunk
                chunk = []
        if chunk:
            yield chunk

    def drain_one() -> None:
        count, future = inflight.popleft()
        writer.write_chunk(future.result())     # re-raises a worker's exception
        before = result.rows
        result.rows += count
        if progress and before // PROGRESS_EVERY != result.rows // PROGRESS_EVERY:
            progress(result.rows)

    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix="render") as pool:
        try:
            for chunk in source():
                inflight.append((len(chunk), pool.submit(writer.render, chunk)))
                # Cap the backlog so memory stays flat on a huge result set.
                while len(inflight) > workers:
                    drain_one()
            while inflight:
                drain_one()
        finally:
            for _, future in inflight:          # cancelled mid-stream
                future.cancel()


def _finish(writer, result: ExportResult) -> None:
    writer.close()
    truncated = getattr(writer, "truncated", 0)
    if truncated:
        result.warnings.append(
            f"{writer.path.name}: {truncated} value(s) truncated to the dbf field width"
        )


def _prefetched(batches: Iterable[Sequence], depth: int = 2) -> Iterator[Sequence]:
    """Yield rows while a background thread already pulls the next pages.

    Without this, fetching and writing take turns: nothing is written while the
    coordinator is handing over a page, and nothing is fetched while the writer
    formats one. Overlapping them is worth 1.3x to 1.5x end to end, and it works
    the same under the GIL because socket reads release it.
    """
    q: queue.Queue = queue.Queue(maxsize=depth)
    stop = threading.Event()
    DONE = object()

    def pump():
        try:
            for batch in batches:
                if stop.is_set():
                    break
                q.put(batch)
        except BaseException as exc:  # re-raised on the consumer side
            q.put(exc)
        else:
            q.put(DONE)

    thread = threading.Thread(target=pump, name="prefetch", daemon=True)
    thread.start()
    try:
        while True:
            item = q.get()
            if item is DONE:
                return
            if isinstance(item, BaseException):
                raise item
            yield from item
    finally:
        # On cancel the consumer walks away mid-stream; drain so a pump parked
        # on put() can wake up, see the flag and exit instead of leaking.
        stop.set()
        while True:
            try:
                q.get_nowait()
            except queue.Empty:
                break


def run_export(
    config: TrinoConfig,
    sql: str,
    out_dir: str | Path,
    basename: str,
    fmt: str,
    batch_size: int = 10_000,
    retries: int = 5,
    opts: dict | None = None,
    rows_per_file: int | None = None,
    progress: Callable[[int], None] | None = None,
    cancel: Callable[[], bool] | None = None,
    on_start: Callable[[Sequence[Column], str | None], None] | None = None,
) -> ExportResult:
    with QueryStream(config, sql, batch_size=batch_size, retries=retries) as stream:
        # The cursor is open and the coordinator has produced its first page: the
        # result's columns and the Trino query id are known from here on. Callers
        # that report progress to a UI use this to name what is being written.
        if on_start is not None:
            on_start(stream.columns, stream.query_id)
        result = export_rows(
            stream.columns, _prefetched(stream.batches()), out_dir, basename, fmt,
            opts=opts, rows_per_file=rows_per_file, progress=progress, cancel=cancel,
        )
        result.query_id = stream.query_id
        if result.cancelled:
            stream.cancel()
        return result


def bundle(files: Sequence[Path], zip_path: Path) -> Path:
    """Zip multi-part exports so the browser gets a single download."""
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
        for path in files:
            zf.write(path, arcname=path.name)
    return zip_path
