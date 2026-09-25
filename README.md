# Trino Exporter

Export a Trino query to any of nine file formats, streaming in batches so a big
result set never lands in memory whole.

| | |
|---|---|
| **Formats** | `.txt` `.csv` `.json` `.xml` `.html` `.sql` `.xls` `.xlsx` `.dbf` |
| **Interfaces** | native macOS app (SwiftUI), `queryhive-engine` CLI, macOS `.app` in a DMG |
| **Connection** | any coordinator: URL or host/port/user/password, nothing hardcoded |
| **Engine** | Rust, no interpreter: the app links it across UniFFI, the CLI runs it directly |

## Why batching

The usual "error fetching results" in a Trino export tool comes from calling
`fetchall()`: the client buffers every row, memory climbs, and the request that
was supposed to drain the result set stalls until the coordinator gives up.

This tool never buffers the result. It walks the cursor one batch at a time and
hands each batch straight to a writer that appends to disk, so:

* peak memory stays flat (300k rows × 9 columns ran in 62 MB RSS on the Python
  engine this replaced; the Rust engine's numbers are measured, not assumed —
  `docs/benchmarks.md`);
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

A native SwiftUI app, no browser involved. `app/build.sh` builds the Rust engine
(`app/build-ffi.sh`), regenerates the committed Swift bindings from it, then
builds and ad-hoc signs the bundle. The `.app` is unsigned, so Gatekeeper blocks
the first launch: right-click → Open, or
`xattr -dr com.apple.quarantine app/dist/QueryHive.app`.

The bundle is self-contained: the app links the engine's **static archive**, so
the Rust code is copied into the app binary and `otool -L` on it lists only Apple
frameworks. Nothing in `dist/QueryHive.app` points at `target/`, which is what
made the DMG portable. Two consequences worth knowing:

- The archive cannot record its own dependencies, so the system frameworks Rust's
  own std needs are named by hand in `app/Package.swift`. If a dependency starts
  using another one, the link fails there with an undefined symbol.
- The deployment target is pinned in `.cargo/config.toml` to the same macOS 14.0
  the app declares. Without it the C and assembly objects inside the archive
  (`ring`, `libsqlite3-sys`) would be built for whatever macOS is doing the
  building, and the bundle would quietly require more than its `Info.plist` says.


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
`app/DESIGN.md` is the design contract and the engine protocol; the engine itself is the
`crates/qh-*` workspace, and it reports progress as events so the UI can show a live row
counter, a real cancel, and the columns the coordinator returned.

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

### Appearance

**Settings (⌘,) ▸ Appearance** picks the **mode** — System, Light or Dark — and then the canvas, the
accent, the tone and how hard the backdrop glows. System follows macOS and keeps following it:
switch the system appearance with the window open and the app repaints with it. Light and Dark pin
one instead. The choice is remembered across launches, and each appearance remembers its own canvas
and its own theme, so switching between them does not throw a choice away.

Three presets — **Classic** (midnight + ice, the app's own look), **Slate** (neutral charcoal + blue)
and **Flat** (Slate with every gradient off) — sit above seven canvases, five accents and three tones
for anyone who wants to mix their own. Each preset names a canvas for the dark *and* one for the
light, so switching mode stays inside the preset.

**Theme** is the canvas, and a canvas belongs to one appearance: Midnight, Graphite, Nord and Ink in
the dark; Daylight, Cloud and Paper in the light. Settings lists only the half you are in, because
there is no such thing as a light Midnight. None of the light canvases is pure white — a `#FFFFFF`
canvas makes every hairline invisible and turns the frosted panels into grey rectangles.

**Tone** is how the coloured surfaces are painted, independent of which colours they are: *Glow* is
what the app shipped with (gradient fills, a sheen, a coloured glow, a lit backdrop), *Plain* is one
flat colour per surface, and *Soft* is a faint accent wash inside an accent border. Plain and Soft
draw no gradient at all — no ramp on the Run capsule, no sheen, no coloured shadow, flat backdrop.

The accent moves the interactive chrome — the Run capsule, focus rings, the selected tab and tile —
and stops there: the object tree's own colours are how it tells a table from a column, and the app's
mark keeps its own pair.

```bash
QueryHive.app/Contents/MacOS/QueryHive --snapshot /tmp/qh.png --theme graphite --accent blue --tone plain
QueryHive.app/Contents/MacOS/QueryHive --snapshot /tmp/qh-light.png --mode light --theme daylight
```

Renders the shell in an appearance you have not chosen, without saving it — which is how the themes
were reviewed. `--mode system` draws what the system would decide; add `--system-appearance
dark|light` to say what the machine would report, which is how a System-mode render is checked
without changing your Mac's setting.

### Ship it as a DMG

```bash
./app/build-dmg.sh          # -> app/dist/QueryHive-0.1.0-arm64.dmg
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
exceptions the FFI boundary needs (ADR-0014), but it has not been exercised against a real
Developer ID yet, so the first notarized build must be launched and its engine used before it is
published.

**Requirements.** Apple Silicon (arm64) and macOS 14 or later, plus a Rust toolchain to build from
source. Every Apple Silicon Mac can run macOS 14, so architecture is not a limit; an Intel Mac
cannot run it at all.

## Build the engine alone

The engine is a Rust workspace; the app and the golden corpus are two consumers of the same
`qh-ffi` surface.

```bash
cargo build --release --bin queryhive-engine   # target/release/queryhive-engine
cargo build -p qh-ffi                           # the library the app links
./app/build-ffi.sh                              # ...then regenerate app/Generated/ from it
```

`cargo build -p qh-ffi` produces two libraries from one run: `libqh_ffi.dylib`, which the binding
generator reads, and `libqh_ffi.a`, which the app links. Only `build-ffi.sh` stages the archive
where `app/Package.swift` looks for it (`target/ffi/static/<profile>/`), so run it rather than
`cargo build` alone if you are about to build the app.

## Run the engine from the CLI

```bash
export DB_KIND=trino DB_HOST=trino.internal DB_PORT=8443 DB_USER=faisal \
       DB_DATABASE=hive DB_SCHEMA=analytics DB_SCHEME=https
export SQL="SELECT * FROM penerima_manfaat WHERE tahun = 2026" \
       FORMAT=xlsx OUT_DIR=~/Downloads NAME=penerima_2026 \
       BATCH_SIZE=20000 ROWS_PER_FILE=500000
target/release/queryhive-engine export
```

The binary takes exactly one argument — the command — and reads everything else
from the environment, because that is the same contract the app uses over FFI:
there is no second set of option names to drift. One JSON object per stdout line
comes back, which is what the app decodes.

| command | what it does |
|---|---|
| `db_drivers` | the drivers this build has, as JSON |
| `connections` `import_connections` `credential` | the app's saved connections and its password store (no driver opened) |
| `objects` `catalogs` `schemas` `tables` | introspection for the object tree |
| `test` | connect and report, without reading rows |
| `export` | stream a query into any of the nine formats |
| `to_table` | `CREATE TABLE AS` / `DROP + CREATE` / `INSERT INTO … SELECT` |
| `preview` `count` `explain` | the first N rows, a row count, the plan |

Connection settings, `DB_*` (the older `TRINO_*` spelling still works; `DB_*` wins):

| env | note |
|---|---|
| `DB_KIND` | `trino`, `postgres` or `mysql`; otherwise the URL's scheme decides |
| `DB_URL` | `postgresql://user:pass@host:5432/db`, or the split fields below |
| `DB_HOST` `DB_PORT` `DB_USER` `DB_PASSWORD` `DB_DATABASE` `DB_SCHEMA` | override any part of the URL |
| `DB_SCHEME` `DB_SSLMODE` `DB_INSECURE` | `http`/`https` plus the TLS mode; a password implies https |
| `DB_ALL_SCHEMAS` | introspect every schema, not just `DB_SCHEMA` |

Batching and output:

| env | default | note |
|---|---|---|
| `SQL` / `SQL_PATH` | — | the statement, inline or a file; one of the two is required |
| `BATCH_SIZE` | 10000 | rows per fetch from the server |
| `ROWS_PER_FILE` | none | split the output every N rows |
| `RETRIES` | 5 | retries on a transient fetch error |
| `FORMAT` | `csv` | one of the nine |
| `OUT_DIR` `NAME` | cwd, `export` | |
| `ZIP` | off | bundle the result files |
| `LIMIT` | 1000 | `preview`'s row cap |
| `PROGRESS_MS` | engine default | how often a progress event is emitted |
| `TARGET_CATALOG` `TARGET_SCHEMA` `TARGET_TABLE` `WRITE_MODE` | | `to_table`'s destination |

Per-format options: `DELIMITER ENCODING HEADER BOM NULL_TEXT` (txt/csv), `JSONL`,
`SQL_TABLE`, `SHEET`, `DBF_CHAR_WIDTH DBF_ENCODING`.

## Format notes

| format | what to know |
|---|---|
| `txt` | tab-separated with a header row; `DELIMITER` changes it |
| `csv` | RFC 4180 quoting; `BOM` if Excel mangles UTF-8 |
| `json` | array of objects, or `JSONL` for one object per line |
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
cargo test --workspace                          # the engine
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check licenses                       # the licence policy, ADR-0002/0011
cd app && swift build && swift test             # the app's decoders and the engine contract
/usr/bin/python3 tools/golden/live_cases.py     # the frozen golden corpus
```

The golden corpus under `tests/golden/` is a recording: 43 commands, their JSON
exactly as the engine produced it, normalised so the volatile parts (timestamps,
paths, server-assigned OIDs) do not make a correct engine look changed. `_live`
cases are re-run against the dev containers; the rest are compared in-process.
`tests/golden/RECORDED.md` says how it was recorded and what each case covers.

The DBF and XLS writers were additionally verified against `dbfread` and `xlrd` back when this
tree still had a Python engine, and all nine formats were exported end-to-end against a real Trino
coordinator using the `tpch` catalog. Those checks are history, not something the current test
suite re-runs.

## Layout

```
crates/
  qh-core/          errors, values, rendering: the types everything else speaks
  qh-driver/        the driver trait every backend implements
  qh-driver-trino/  the hand-rolled Trino client (ADR-0006)
  qh-driver-postgres/  qh-driver-mysql/    libpq / MySQL protocol clients
  qh-export/        9 streaming writers, part splitting, plan
  qh-sql/           identifier quoting and the SQL a connection's driver needs
  qh-storage/       connections.json; qh-credentials/ the Keychain side
  qh-tunnel/        the SSH bastion a connection can be reached through
  qh-result-store/  the grid's row store; qh-sync/ its prefetch
  qh-rt/            the tokio runtime the FFI owns
  qh-ffi/           the engine entry point: the fourteen commands, the UniFFI surface,
                    and `queryhive-engine`, the CLI the golden harness runs
app/
  DESIGN.md                 the native app's design contract and engine protocol
  Package.swift             SwiftPM targets: the executable, the generated bindings, the C
                            module they call, and the test target
  Sources/TrinoExporter/
    App.swift               @main, the engine's Event type, notices
    Support/DatabaseEngine.swift  the DatabaseEngine/EngineRun contract and the Engine.current seam
    Support/EngineWire.swift  the event wire format
    Support/RustEngine.swift  RustEngine: the contract over UniFFI, and Engine.current
    Support/Theme.swift     the CleanMyMac design system: Tone, Hue, glass, HubButton, the comb
    Support/ThemeStore.swift  the appearance choice: AppTheme, AccentChoice, ThemeStore, presets
    Support/AppIcon.swift   the app icon, drawn in code
    Support/DriverLogos.swift  database brand marks, generated from assets/drivers/*.svg
    Support/NavicatImport.swift  reads connections out of a Navicat .ncx export
    Support/Snapshot.swift  renders the shell to a PNG for design review
    Models/AppModel.swift   connections, object tree, tabs, run orchestration
    Models/QueryTab.swift   one query tab: SQL, destination, format options, log, run state
    Models/SchemaTree.swift one lazily-loaded tree node (connection/catalog/schema/table)
    Models/Connections.swift saved coordinators + Keychain + Trino URL parsing
    Models/Shortcuts.swift  the actions a key can be bound to
    Models/SQLSuggestions.swift  suggestion model, ranking kinds, the Trino keyword list
    Views/RootView.swift    the shell: title strip, tree, workspace, status bar
    Views/SidebarTree.swift the object tree
    Views/Workspace.swift   tab strip, toolbar, format popover, seams
    Views/SQLEditor.swift   NSTextView-backed editor: caret tracking, key interception
    Views/SQLSyntax.swift   colours the editor by what each token is
    Views/SuggestionPopup.swift  the completion list and its caret-anchored placement
    Views/ResultGrid.swift  the rows a Run fetched, as an NSTableView
    Views/ContextCascade.swift  the toolbar's connection and level breadcrumb
    Views/ExportSettings.swift  where an export goes, behind one button
    Views/Panels.swift      the Log / Columns / Files panel
    Views/ConnectionsViews.swift  connection picker, colour swatches, editor sheet
    Views/SettingsView.swift  the Settings window: Appearance and Keyboard
  Tests/TrinoExporterTests/  EngineContractTests, EventDecodingTests, RustEngineTests, plus the
                            MockEngine and EngineContract they share
  Generated/                UniFFI output, committed: the Swift bindings, the C header, the
                            modulemap, and the one translation unit build-ffi.sh writes
  build-ffi.sh                builds libqh_ffi --release and regenerates Generated/
  build.sh                    builds app/dist/QueryHive.app around that library
  build-dmg.sh                wraps that .app in a DMG, with optional Developer ID + notarization
  QueryHive.entitlements      the entitlements the hardened runtime needs (ADR-0014)
  make-icon.sh                regenerates assets/icon.icns from the app's own drawing code
  make-driver-logos.sh        regenerates Support/DriverLogos.swift from assets/drivers/*.svg
tests/golden/                 the frozen corpus, its recordings and RECORDED.md
tools/golden/live_cases.py    runs the live cases against the debug binary
deploy/dev/                   the fixture containers, and the benchmark harness
docs/decisions/               ADRs; docs/architecture/ the blueprint and the folder proposal
```

## Licence

MIT — see [LICENSE](LICENSE). The workspace's Rust dependencies carry their own licences; the
policy is the allow-list in `deny.toml`, enforced by `cargo deny check licenses` (ADR-0002,
ADR-0011), so the list in that file — not this paragraph — is the authority.

Two things the licence does **not** cover, both worth knowing before you rebrand or redistribute:

- The Trino, PostgreSQL and MySQL marks in `assets/drivers/` are trademarks of their respective
  projects, used only to say which database a connection talks to. MIT does not grant rights to
  them.
- A DMG built here is ad-hoc signed, so it is notarised nowhere. Nothing in this licence obliges
  anyone to distribute a signed build.
