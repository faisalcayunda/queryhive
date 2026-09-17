# Trino Exporter

Export a Trino query to any of nine file formats, streaming in batches so a big
result set never lands in memory whole.

| | |
|---|---|
| **Formats** | `.txt` `.csv` `.json` `.xml` `.html` `.sql` `.xls` `.xlsx` `.dbf` |
| **Interfaces** | native macOS app (SwiftUI), local web UI (browser), CLI, macOS `.app` in a DMG |
| **Connection** | any coordinator: URL or host/port/user/password, nothing hardcoded |

## Why batching

The usual "error fetching results" in a Trino export tool comes from calling
`fetchall()`: the client buffers every row, memory climbs, and the request that
was supposed to drain the result set stalls until the coordinator gives up.

This tool never buffers the result. It walks the cursor with
`fetchmany(batch_size)` and hands each batch straight to a writer that appends
to disk, so:

* peak memory stays flat (300k rows × 9 columns ran in 62 MB RSS);
* pages are pulled at the pace the writer consumes them, and a 30s heartbeat
  keeps the query alive while a slow format catches up;
* transient fetch failures (502/503/504, dropped connections, read timeouts)
  are retried with exponential backoff instead of killing the export;
* results that outgrow a format split into numbered parts automatically.

## Run the macOS app

```bash
./app/build.sh          # -> app/dist/QueryHive.app
open app/dist/QueryHive.app
```

A native SwiftUI app, no browser involved. `app/build.sh` builds a standalone arm64 CPython
plus the pinned engine packages into `app/.engine/` (idempotent — it is a fast no-op until the
pins change), then bundles the app, the engine and the `exporter` package and ad-hoc signs the
result. The `.app` is unsigned, so Gatekeeper blocks the first launch: right-click → Open, or
`xattr -dr com.apple.quarantine app/dist/QueryHive.app`.

**Run shows you the rows; Export writes them.** Run fetches the first N (the `LIMIT` in the grid's
footer, default 1000) and paints them in a result grid — row numbers, a pinned header with type
chips, numbers right-aligned, `null` rendered as a null. Nothing is written until you press Export
in that grid's footer, which re-runs the statement and streams the whole result. The row limit
changes what you look at, never the query: the engine stops reading, it does not rewrite your SQL.

A query can go to one of two destinations, switched in the toolbar. **File** streams the result
into any of the nine formats. **Table** hands the query to Trino to write itself —
`CREATE TABLE … AS`, `DROP TABLE IF EXISTS` + `CREATE`, or `INSERT INTO … SELECT` — so the rows
never travel over the wire to your laptop at all. Replace is drawn in coral and asks first,
because the drop happens before the query runs.

It is laid out like Navicat and finished like CleanMyMac: an object tree down the left
(connection → catalog → schema → table, fetched as you expand), query tabs across the top, a
toolbar with Run/Stop, the SQL editor over a Log / Columns / Files panel you can drag, and a
status bar. The visual layer — near-black canvas, two-colour module glow, frosted glass,
rounded type, gradient accents — is the same one its sibling `scripts/iceberg_importer` uses.
`app/DESIGN.md` is the design contract and the engine protocol;
`app/engine/queryhive_engine.py` is the Python side, which reports progress as one JSON object
per stdout line so the UI can show a live row counter, a real cancel, and the columns the
coordinator returned.

Connections come in three kinds — **Trino**, **PostgreSQL** and **MySQL** — and adding one works
the way Navicat's does: pick the type from a grid of tiles, then fill in its fields. The driver is
not a label: it decides the default port, which fields are required, how an identifier is quoted,
and what the object tree can even browse. Trino has catalog → schema → table; Postgres has no
cross-database browsing, so its database is fixed by the connection and its tree is schema →
table; MySQL has no schema level, so its tree is database → table.

They are saved in `~/Library/Application Support/QueryHive/connections.json` with the password in
your Keychain, and "Add from URL…" accepts `trino://`, `postgresql://` or `mysql://` and shows you
what it parsed before saving. Double-clicking a table in the tree drops its name into the query,
quoted the way that connection's driver wants.

The SQL editor suggests as you type — table, schema and catalog names from the objects you have
loaded, the columns the last run reported, and Trino keywords — filtered by the word under the
caret and ranked so an object beats a keyword on the same prefix. ↑ ↓ pick, ⏎ or ⇥ accepts, esc
dismisses, and ⌃Space opens the list on demand. Nothing is offered inside a string literal.

### Ship it as a DMG

```bash
./app/build-dmg.sh          # -> app/dist/QueryHive-0.0.1-arm64.dmg (40 MB)
```

The DMG holds the app, an `/Applications` symlink and a `READ ME.txt`. The build verifies its own
artifact before reporting success: it mounts the image, checks the signature, confirms the binary
really is arm64, reads the minimum macOS out of the Mach-O, and renders a full UI snapshot **from
the read-only volume** — so a DMG that only works after being copied out cannot pass.

**Gatekeeper.** The build is ad-hoc signed: the signature is internally valid and the app never
reports as damaged, but Apple has not vetted it, so a *downloaded* copy is quarantined and macOS
refuses the first launch until the user allows it once. The `READ ME.txt` in the DMG spells out
the three ways to do that. There is no way around it except notarization:

```bash
xcrun notarytool store-credentials queryhive-notary \
  --apple-id <you@example.com> --team-id <TEAMID> --password <app-specific-password>
NOTARY_PROFILE=queryhive-notary ./app/build-dmg.sh --notarize
```

That needs a **Developer ID Application** certificate. An "Apple Development" certificate does not
substitute for one — measured on this project, `spctl` rejects it exactly as it rejects ad-hoc
(`origin=Apple Development: ...`). `app/QueryHive.entitlements` carries the two hardened-runtime
exceptions a bundled CPython needs, but it has not been exercised against a real Developer ID yet,
so the first notarized build must be launched and its engine used before it is published.

**Requirements.** Apple Silicon (arm64) and macOS 14 or later. Every Apple Silicon Mac can run
macOS 14, so architecture is not a limit; an Intel Mac cannot run it at all, and the app carries
its own Python so nothing else needs installing.

The root `build_dmg.sh` is the older pywebview build of the same tool — the browser UI wrapped in
a window. Both drive the identical `exporter` package.

## Install

```bash
git clone <this repo> && cd trino_exporter
uv venv --python 3.12 .venv
VIRTUAL_ENV=.venv uv pip install -r requirements.txt
```

Plain pip works too: `python3 -m venv .venv && .venv/bin/pip install -r requirements.txt`.

## Run on localhost

```bash
./run_local.sh                 # http://127.0.0.1:8765, opens your browser
./run_local.sh --no-browser    # just serve
TRINO_EXPORTER_PORT=9000 ./run_local.sh
```

The page has a **Trino URL** box plus individual host / port / scheme / user /
password / catalog / schema fields; the two stay in sync, edit whichever you
prefer. Connection settings (never the password) are remembered in
`localStorage`. Exports run in the background with a live row counter, a cancel
button, and a download link. Multi-part exports arrive as a zip.

## Run from the CLI

```bash
.venv/bin/python -m exporter.cli \
  --url https://faisal@trino.internal:8443/hive/analytics \
  -q "SELECT * FROM penerima_manfaat WHERE tahun = 2026" \
  -F xlsx -o ~/Downloads -n penerima_2026 \
  --batch-size 20000 --rows-per-file 500000
```

Connection flags, all optional if the URL carries them:

| flag | env | note |
|---|---|---|
| `--url` | `TRINO_URL` | `https://user:pass@host:port/catalog/schema` |
| `--host` `--port` | `TRINO_HOST` `TRINO_PORT` | override any part of the URL |
| `--user` `--password` | `TRINO_USER` `TRINO_PASSWORD` | a password implies https |
| `--catalog` `--schema` | `TRINO_CATALOG` `TRINO_SCHEMA` | |
| `--https` `--insecure` | | force TLS / skip certificate checks |
| `--session K=V` | | repeatable session property |

Batching and output:

| flag | default | note |
|---|---|---|
| `--batch-size` | 10000 | rows per fetch from the coordinator |
| `--rows-per-file` | none | split the output every N rows |
| `--retries` | 5 | retries on a transient fetch error |
| `-F/--format` | csv | one of the nine |
| `-o/--out-dir` `-n/--name` | cwd, `export` | |
| `--zip` | off | bundle the result files |

Read SQL from a file or a pipe with `-f query.sql` / `-f -`.

Per-format options: `--delimiter --encoding --no-header --bom --null-text`
(txt/csv), `--jsonl`, `--sql-table`, `--sheet`, `--dbf-char-width --dbf-encoding`.

## Build the DMG (Apple Silicon)

```bash
./build_dmg.sh      # -> dist/TrinoExporter-1.0.0-arm64.dmg
```

Runs the tests, builds an arm64 app bundle with PyInstaller, smoke-tests that
the bundle actually serves, then packs the `.app` next to an `/Applications`
symlink. PyInstaller cannot cross-compile, so build on an arm64 Mac.

The bundle is **unsigned**. On first launch Gatekeeper will block it:
right-click → Open, or

```bash
xattr -dr com.apple.quarantine "/Applications/Trino Exporter.app"
```

Opening the app starts the same server on a free port and opens your browser.
There is no menu bar, so the header has a **Quit** button that stops it.

## Format notes

| format | what to know |
|---|---|
| `txt` | tab-separated with a header row; `--delimiter` changes it |
| `csv` | RFC 4180 quoting; `--bom` if Excel mangles UTF-8 |
| `json` | array of objects, or `--jsonl` for one object per line |
| `xml` | `<RECORDS><RECORD>`; column names sanitised into valid tags |
| `html` | standalone page with a sticky header, everything escaped |
| `sql` | multi-row `INSERT`s, 200 rows per statement, quotes doubled |
| `xls` | BIFF8: **65,535 rows / 256 columns** per file, splits automatically |
| `xlsx` | 1,048,576 rows per file, splits automatically; written streaming |
| `dbf` | dBase III+: 10-char upper-case field names, fixed-width fields, 4000-byte records. Char width shrinks to fit and long text is truncated (the run warns and counts it). Verified against `dbfread`. |

Dates and numbers stay native in `xls`/`xlsx`/`dbf`. Timezone-aware timestamps
have no cell representation in Excel, so they are written as text with the
offset intact rather than silently losing it. `ARRAY`/`MAP`/`ROW`/`JSON`
columns are serialised as JSON text everywhere except `json` itself.

## Tests

```bash
.venv/bin/python tests/test_writers.py
```

11 checks covering every writer (parsed back with `csv`, `json`,
`ElementTree`, `openpyxl`, and a byte-level DBF header check), part splitting,
and cancellation. The DBF and XLS outputs were additionally verified against
`dbfread` and `xlrd`, and all nine formats were exported end-to-end against a
real Trino coordinator using the `tpch` catalog.

## Layout

```
exporter/
  writers.py   9 streaming writers + value formatting (dbf is hand-rolled)
  source.py    batched cursor, retries, connection config / URL parsing
  export.py    orchestration: part splitting, progress, zip bundling
  to_table.py  CREATE TABLE AS SELECT / INSERT INTO ... SELECT
  cli.py       command line
  web.py       FastAPI app, background jobs
  static/      the UI, single self-contained file
app/
  DESIGN.md                 the native app's design contract and engine protocol
  Package.swift             SwiftPM target producing the QueryHive executable
  Sources/TrinoExporter/
    App.swift               @main, the engine's Event type, notices
    Support/Theme.swift     the CleanMyMac design system: Tone, Hue, glass, HubButton, the comb
    Support/Engine.swift    launches the bundled engine, decodes its JSON events
    Models/AppModel.swift   connections, object tree, tabs, run orchestration
    Models/QueryTab.swift   one query tab: SQL, destination, format options, log, run state
    Models/SchemaTree.swift one lazily-loaded tree node (connection/catalog/schema/table)
    Models/Connections.swift saved coordinators + Keychain + Trino URL parsing
    Views/RootView.swift    the shell: title strip, tree, workspace, status bar
    Views/SidebarTree.swift the object tree
    Views/Workspace.swift   tab strip, toolbar, format popover, seams
    Views/SQLEditor.swift   NSTextView-backed editor: caret tracking, key interception
    Views/SuggestionPopup.swift  the completion list and its caret-anchored placement
    Models/SQLSuggestions.swift  suggestion model, ranking kinds, the Trino keyword list
    Views/Panels.swift      the Log / Columns / Files panel
    Views/ConnectionsViews.swift  connection picker, colour swatches, editor sheet
  exporter/to_table.py        CTAS / INSERT INTO ... SELECT, executed by the database
  exporter/drivers.py         Trino / PostgreSQL / MySQL: quoting, levels, connect
  engine/queryhive_engine.py  JSON-event CLI the app drives (db_drivers, test, catalogs, schemas, tables, export, to_table)
  build-engine.sh             builds the bundled standalone CPython + pinned packages
  build.sh                    builds app/dist/QueryHive.app
  make-icon.sh                regenerates assets/icon.icns from the app's own drawing code
app.py         launcher used by both run_local.sh and the pywebview .app bundle
```

## Licence

MIT — see [LICENSE](LICENSE). Bundled dependencies carry their own licences (Trino client:
Apache 2.0; psycopg: PostgreSQL License; PyMySQL, openpyxl and XlsxWriter: MIT), all of which are
compatible with redistributing this under MIT.

Two things the licence does **not** cover, both worth knowing before you rebrand or redistribute:

- The Trino, PostgreSQL and MySQL marks in `assets/drivers/` are trademarks of their respective
  projects, used only to say which database a connection talks to. MIT does not grant rights to
  them.
- A DMG built here is ad-hoc signed, so it is notarised nowhere. Nothing in this licence obliges
  anyone to distribute a signed build.
