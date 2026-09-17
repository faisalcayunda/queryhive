# QueryHive app: design contract

Single source of truth for the native macOS app in `app/`. If an implementation detail
conflicts with this file, stop and report the conflict instead of choosing silently.

## Scope

- macOS app, SwiftUI, Apple Silicon (arm64) only, macOS 14+, **CleanMyMac surfaces on a Navicat
  layout** (see `Sources/TrinoExporter/Support/Theme.swift`).
- One job, two destinations. A query either streams into one of nine file formats (txt, csv,
  json, xml, html, sql, xls, xlsx, dbf) without ever holding the result set in memory, or is
  handed to Trino to write into a table.
- Object browser: connection → catalog → schema → table, loaded lazily.
- Built-in engine: a bundled standalone CPython 3.12 with pinned packages. The user never
  configures a Python path.
- Saved connections, Navicat style. The password lives in the macOS Keychain.
- Out of scope: universal or Intel builds, notarization, editing table data, saved query files,
  a result data grid (the export *is* the result).

The browser UI (`exporter/static/index.html`, served by `exporter/web.py`) and the CLI
(`exporter/cli.py`) stay as they are. All three front ends drive the same `exporter` package;
only the app talks to it through the JSON-event engine below.

## Design language

Two layers, and they must not be confused with each other:

- **Navicat supplies the furniture.** An object tree down the left, a tabbed query workspace,
  a toolbar, an editor over a panel with a draggable seam, a status bar.
- **CleanMyMac supplies the surfaces.** Near-black canvas, two-colour module glow, frosted
  glass, rounded type, gradient accents.

This is deliberately *not* the layout of its sibling `scripts/iceberg_importer` (one hero, one
orb, one job per screen). That app runs a single linear flow; this one is a workspace where
several queries are open at once. What the two share is the palette, the glass and the mark.

| Element | Rule |
|---|---|
| Canvas | `Tone.canvas` `#0A0B1E`, near-black, behind everything. |
| Module glow | Two colours per module (`Hue`): the exporter is ice `#4FD8FF` → violet `#7B61FF`, connections are magenta → violet. Success is mint, failure amber → coral. |
| Backdrop | `Backdrop(hue:)` behind the workspace only; the tree and the status bar carry their own material. Radial gradients, never `.blur` (a snapshot capture does not render it). |
| Surfaces | Panels are `.glass(radius)` where they float, flat tinted fills where they are chrome. |
| Type | Rounded for titles (30pt `heroTitle`, 14pt `cardTitle`), `body13` for prose, monospaced for anything the engine produced — paths, counts, query ids, column names. |
| Action | `HubButton`: a gradient capsule with a sheen, a coloured glow and a press-scale. The round orb belongs to a full-screen flow; this app's primary action is a toolbar button. |
| Mark | `HiveMark` (single cell, 15pt, sidebar header) and `HiveHero` (seven-cell comb with the centre cell lit, empty workspace only). The comb is one glyph shared by both, and the lattice divisor must clear its **height** (5.464r), not its width (5r). Every hexagon in the app comes from `hexagonPath`. |

### Shell

```
┌───────────────────────────────────────────────────────────────────────┐
│ (traffic lights)  ⬡ QUERYHIVE                connection chip          │ 46  TitleStrip
├──────────────────┬────────────────────────────────────────────────────┤
│ OBJECTS      + ⟳ │ [Query 1][Query 2][+]                              │ 36  TabStrip
│ [ filter…      ] ├────────────────────────────────────────────────────┤
│ ▾ ⬡ conn        │ [▶ Run] │ conn ▾ │ CSV ▾ │ folder │      name       │ 46  QueryToolbar
│   ▾ cylinder cat ├────────────────────────────────────────────────────┤
│     ▾ folder sch │ SQL editor                                         │    EditorPane
│       ▤ table    │                                                    │
│       ▤ table    ╞═══════════════ draggable seam ════════════════════╡  7  PanelResizer
│   ▸ ⬡ conn2      │ [Log 12][Columns 8][Files 2]                    ⌄  │ 30  BottomPanel
│                  │  the panel's content                               │
├──────────────────┴────────────────────────────────────────────────────┤
│ ● conn · host:port/catalog   Query OK · 12,345 rows · 1 file · 0:42   │ 26  StatusBar
└───────────────────────────────────────────────────────────────────────┘
```

- The title strip carries **nothing on the left**. A mark placed just after the window controls
  sat closer to them than to its own wordmark, so the two read as one object and the lights
  looked crowded. The app's identity lives in the sidebar header instead, which is a panel and
  can give it room; the strip keeps the connection chip on the right and stays draggable
  everywhere else.
- The tree and the panel both resize by dragging their seam; the cursor changes over each.
- The toolbar has **two shapes**, switched by the `File | Table` segmented control:

  ```
  File   [▶ Run]  │ [conn ▾] │ [File|Table] │ [CSV ▾] │ [📁 folder] │ [name]
  Table  [⤓ Save] │ [conn ▾] │ [File|Table] │ [hive.analytics.… ▾] │ [Create ▾]
  ```

  Run writes a file; Save writes a table, because a table run can drop something and the label
  should not hide that behind the same word as writing a CSV.
- The **format button opens a popover**, not a menu: nine writers want a tile grid, and each
  one's own options belong next to the choice, the way Navicat's export wizard puts them.
- The **target button opens a popover** for the same reason, measured rather than assumed: four
  target fields inline needed ~923pt, and at the window's 1120pt minimum that truncated both the
  connection name and the table name. The one value that must never be truncated is the table
  about to be dropped, so the toolbar shows `catalog.schema.table` with middle truncation and the
  popover holds full-width fields plus the three write modes spelled out.

### Spacing

`Metrics` in `Theme.swift` is the only place these numbers exist. Every pane measures its
horizontal padding from `Metrics.gutter`, so the title strip, tab strip, toolbar, editor header,
editor, panel header, panel rows and status bar all start on the same vertical line. Before this
existed the gutters were 6, 8, 9, 10, 12, 13 and 14 in different files, which is exactly what made
the window look assembled rather than drawn.

| Constant | Value | Where |
|---|---|---|
| `gutter` | 12 | every pane's horizontal padding |
| `titleStrip` / `tabStrip` / `toolbar` | 40 / 36 / 46 | window chrome |
| `paneHeader` / `panelTabs` / `statusBar` | 32 / 30 / 26 | the shorter rows |
| `control` | 28 | Run, connection picker, format and folder chips, output-name field |
| `treeRow` / `treeIndent` | 23 / 15 | object tree |

The remaining literal paddings in the views are all *inside* a control — a chip's own label
inset, a pill's text inset — and are deliberately not the gutter. The object tree is the one
place with a second offset: its rows sit in a stack padded by 6 and add `depth × treeIndent + 6`,
so a depth-0 row's content lands on the gutter alongside the "OBJECTS" label.

### Workspace states

- **Empty** (`tabs.isEmpty`): `HiveHero`, "No query open", and a New Query button.
- **Idle**: toolbar enabled, editor focused, panel showing whatever the last run left.
- **Running**: Run becomes Stop; the Log panel is forced open and the status bar shows a live
  row counter. A tab is `stopped`, not `failed`, when the user is the reason it ended.
- **Done / failed**: carried by the tab's dot, the status bar line and the Log entries. There is
  no separate result screen to navigate away from — the panel is always the result.

### Panel contents

| Panel | Contents |
|---|---|
| Log | One line per engine event, translated into a sentence, with a timestamp and a kind tint. Errors from a failed launch land here too. |
| Columns | What the coordinator reported in the `start` event: ordinal, name, type code as a chip. |
| Files | What landed on disk: name, bytes, per-file Reveal, and a total with Reveal in Finder. |

### The SQL editor and its suggestions

The editor is an `NSTextView` (`SQLEditor`), not SwiftUI's `TextEditor`. That is forced:
`TextEditor` exposes no caret, no selection and no key interception, so there is nowhere to hang a
completion list, and `@FocusState` cannot see inside it either — the editor reports focus itself
through `textDidBeginEditing` / `textDidEndEditing`.

`SQLEditor` wraps the text view in a **flipped** container, so the caret rect it publishes can be
handed straight to a SwiftUI `.offset` without the popup landing at the wrong end of the pane.

| Rule | Why |
|---|---|
| Automatic quote/dash/text replacement is **off** | Otherwise `'` becomes a curly quote and `--` an em dash, silently corrupting SQL. |
| Suggestions are the app's own popup, not AppKit's | They wear the module's colours, and Option-Esc is taken so AppKit's word-based completion panel cannot appear behind them. |
| ⌃Space and Option-Esc force the list open; otherwise it appears after 2 characters | Two characters is the floor for typing; the word after a `.` opens at 0 because an object is the only thing that can come next. |
| Nothing is offered inside a `'…'` literal | The user is writing data there, not code. |
| Rows are keyboard-only (↑ ↓ ⏎ ⇥ esc), not clickable | A click would move first responder out of the text view and cost the caret. |
| The caret follows text appended from outside | Double-clicking a table in the tree should leave the caret after what it just wrote. |

Candidates come from three places, ranked by kind (table → column → schema → catalog → keyword),
then by length, then alphabetically, capped at 8:

1. object names **already loaded** in the tree,
2. the columns the last run reported,
3. `SQLSuggestions.keywords`.

Objects come from the tree, so a fully collapsed tree offers keywords only. Browsing is a network
round trip per level, and blocking a keystroke on one would be worse than a short list — the same
bargain the tree's filter box makes, and the editor header says so.

`model.completion` (`EditorCompletion`) holds the one visible list. It lives on `AppModel` rather
than inside the editor view, so the snapshot tool can open it without typing and so it is
dismissed once on a tab change instead of being rebuilt by every keystroke.

### The app icon

`assets/icon.icns` is **generated**, never hand-drawn:

```bash
./app/make-icon.sh
```

That renders `QueryHiveIcon` (`Support/AppIcon.swift`) through the app's own `--icon` flag, builds
Apple's full iconset and runs `iconutil`. The point is that the icon is drawn from the same
`hiveCells` lattice as the in-app mark, so the Dock and the sidebar cannot drift apart — editing
the artwork means editing that view.

It ships **two** artworks, because one does not survive the whole size range. The seven-cell comb
is legible at 64pt and up; at 32 it turned into a smudge with only the lit centre left, and at 16
it was a dot. So 16 and 32 use a single hexagonal *ring* — the negative space defines the shape at
sizes where a solid fill's vertices anti-alias away — which is also the shape of `HiveMark`. That
mapping lives in `make-icon.sh`, not in the drawing code.

The tile follows Apple's grid: the squircle is 0.805 of the canvas with a 0.225 corner ratio, and
no drop shadow is baked in, because the Dock draws its own.

### Reviewing the design without a human in front of the window

```bash
app/dist/QueryHive.app/Contents/MacOS/QueryHive --snapshot /tmp/qh.png --scene done
```

Renders the shell to a PNG and exits. It draws the app's **own window** with
`cacheDisplay(in:to:)` rather than `ImageRenderer`, and that choice is load-bearing:
`ImageRenderer` cannot rasterize `TextEditor`, `TextField` or `ScrollView` content (it
substitutes a "prohibited" placeholder) and gives `.ultraThinMaterial` nothing to sample, so an
ImageRenderer capture shows empty panels and flat glass. Drawing the real window has neither
problem and needs no screen-recording permission, because a process may always draw its own
views.

Scenes: `done` (default), `files`, `columns`, `empty`, `running`, `connection` (which captures the
editor sheet instead of the main window), and `suggest`, which types into the SQL editor through
the real path — `insertText` fires `textDidChange`, which is what runs the debounce, the candidate
lookup and the popup — so it proves the whole suggestion chain rather than just the popup's
drawing. The fixture is fixed, so two runs of one build produce the same image, and the
connections file and the Keychain are never touched.

## Engine CLI (`app/engine/queryhive_engine.py`)

Commands: `test`, `catalogs`, `schemas`, `tables`, `export`. Every stdout line is one compact
JSON object. Every failure, including a usage error, emits `{"event": "error", "message": ...}`
and exits 1. Tracebacks go to stderr.

Settings come from environment variables only, so a password never appears in a process listing.
The app additionally never puts the password inside `TRINO_URL`, so a URL echoed back in an
error message cannot leak it.

| Key | Used by | Meaning |
|---|---|---|
| `TRINO_URL` | all | Whole connection URL; the parts below override it. |
| `TRINO_HOST`, `TRINO_PORT`, `TRINO_USER`, `TRINO_PASSWORD`, `TRINO_CATALOG`, `TRINO_SCHEMA` | all | Individual parts. A blank password means no BasicAuth. `TRINO_CATALOG`/`TRINO_SCHEMA` also name what `schemas` and `tables` list from. |
| `TRINO_INSECURE` | all | `1` skips TLS verification. |
| `SQL`, `SQL_PATH` | export, to_table | The statement, or a file holding it. One is required. |
| `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE` | to_table | Where the rows are written. Any blank is a usage error. |
| `WRITE_MODE` | to_table | `create` (default), `replace`, `append`. |
| `FORMAT` | export | `txt` `csv` `json` `xml` `html` `sql` `xls` `xlsx` `dbf`. Default `csv`. |
| `OUT_DIR`, `NAME`, `ZIP` | export | Where the files land, their base name, and whether to zip a multi-file result. |
| `BATCH_SIZE`, `ROWS_PER_FILE`, `RETRIES` | export, browse | Rows per `fetchmany`, split threshold (`0`/blank = never), transient-error retries. `RETRIES` is floored at 1: the connector crashes on `max_attempts = 0`. |
| `DELIMITER`, `ENCODING`, `HEADER`, `BOM`, `NULL_TEXT`, `JSONL`, `SQL_TABLE`, `SHEET`, `DBF_CHAR_WIDTH`, `DBF_ENCODING` | export | Per-writer options, exactly the `opts` keys `cli.py` builds. |
| `PROGRESS_MS` | export | Minimum gap between `progress` events (default 250). |

Events:

| Event | Fields |
|---|---|
| `step` | `step`: `connect`, `write` |
| `start` | `columns` [{`name`, `type`}], `query_id` |
| `progress` | `rows` (cumulative) |
| `done` | `rows`, `files` [{`path`, `bytes`}], `warnings`, `query_id`, `cancelled` |
| `test` | `ok`, `catalog_count`, `host`, `user` |
| `catalogs`, `schemas`, `tables` | `names` — the identifiers, in the coordinator's own order |
| `to_table` `done` | `rows`, `table`, `mode`, `query_id`, `cancelled`, `warnings`; `progress` also carries `state` |
| `error` | `message` |

- `step connect` is emitted before the network is touched; `step write` and `start` fire
  together, once the cursor is open and the first page exists.
- `progress` is throttled to `PROGRESS_MS` and floored at one event per 1000 rows; a final
  `progress` always precedes `done`.
- `test` reports `catalog_count`, **not** `catalogs`: `catalogs` is also the name of the browse
  *event*, whose payload is a string array. One key cannot be two types, so the count is named.
- The three browse commands quote their identifiers, doubling any embedded `"`. A blank
  `TRINO_CATALOG` (or `TRINO_SCHEMA` for `tables`) is a usage error, never `FROM ""`.
- `SIGTERM`/`SIGINT` set a cancel flag that `run_export`'s `cancel` callback reads. A cancelled
  run still emits a well-formed `done` with `cancelled: true` and the files written so far, then
  exits 0. `Engine.run` redacts every env value whose key looks secret, plus each long `:`-split
  piece of one, out of stderr and out of `error` messages.

### The table destination

`to_table` is the opposite trade to `export`. Nothing is streamed back: the engine opens one
connection, runs `CREATE TABLE … AS <select>` (or `DROP TABLE IF EXISTS` + `CREATE`, or
`INSERT INTO … <select>`) and the coordinator writes the rows itself. A 500M-row CTAS costs the
client one HTTP request and some polling, not 500M rows over the wire.

That means `QueryStream` is unusable here — it exists to pull rows and explicitly rejects a
statement with no result set. `exporter/to_table.py` uses the DBAPI directly and leans on three
things I verified in the installed client rather than assumed:

- `cursor.execute()` blocks until the statement is **FINISHED**, so its return is the completion.
- `cursor.rowcount` carries Trino's `update_count` for CTAS and INSERT, and is **-1** when the
  coordinator reported none. The engine never turns that into a number.
- `stats_callback` fires on every coordinator update, which is the only progress signal available
  for a statement whose rows never arrive. It drives the `progress` events, and it is also where
  cancellation is noticed — the callback calls `cursor.cancel()` once, because a signal handler
  cannot interrupt a blocking socket read reliably.

**Every error path goes through `describe_error`.** trino-python-client 0.339.0 raises, at
`client.py:985`,

```python
TrinoUserError("Query has been cancelled", self.query_id)
```

a bare string where `TrinoQueryError.__init__` expects an error *dict*. Its `__str__` is
`repr(self)` → `self.error_type` → `self._error.get(...)`, so `str(exc)` on the client's own
cancellation error raises `AttributeError: 'str' object has no attribute 'get'`. The naive code
therefore turned every cancel into a nonsense error — and in `main()`, where `str(exc)` formats
the last line of defence, it would have thrown before writing the `error` event at all, leaving
the app an empty stdout to parse. `describe_error` in `exporter/source.py` falls back to the
exception's `args`, which is where the client's message actually still is.

Two checks pin it, and they run in **both** environments on purpose: the local trino stub defines
a well-behaved `TrinoUserError`, so on a checkout without trino installed the real shape is never
exercised — which is exactly how this shipped green.

`replace` deserves its own warning: `DROP TABLE IF EXISTS` runs **before** the query. If the
CREATE then fails, the old table is gone. `TableExportError` carries the warnings collected so
far so the app can say exactly that instead of reporting a bare failure, and the toolbar draws
Replace in coral and asks for confirmation before starting.

The target catalog/schema are deliberately **not** the session's `TRINO_CATALOG`/`TRINO_SCHEMA`:
a query may read from one catalog and write into another.

## Object tree

- One root per saved connection. `children == nil` means "never fetched"; `children == []` means
  "fetched and empty" — the distinction is what keeps a collapsed node from re-querying the
  coordinator on every redraw.
- Expanding a node runs exactly one command: `.connection` → `catalogs`, `.catalog` → `schemas`,
  `.schema` → `tables`. Tables are leaves.
- Browse runs send `RETRIES=2`: a typo'd catalog should come back quickly, not after five
  backoffs. A failed fetch keeps `children == nil`, so re-expanding retries, and shows the
  error as a child row.
- `rebuildTree()` reuses nodes by id, so a rename, a recolour or a delete never collapses what
  the user had open. "Refresh" (context menu) drops one node's children; the tree header's
  reload button drops the lot.
- Double-click inserts: a table's quoted `"catalog"."schema"."table"` goes into the active
  query, a connection opens its editor, anything else toggles.
- The filter box searches **what has already been loaded** and auto-opens matching levels. It
  cannot search a coordinator, and the UI says so.

## Connections

```
Connection { id: UUID, name, color, host, port, httpScheme, user, catalog, schema, verify }
color: gray | green | amber | red | blue | violet
```

- Stored as a JSON array in `~/Library/Application Support/QueryHive/connections.json`. No
  secrets in that file. A file that fails to decode is moved aside, never overwritten.
- Password: Keychain generic password, service `id.data-ecosystem.queryhive`, account =
  connection id. Deleting a connection deletes its Keychain item. The password is typed in a
  `SecureField` and never logged, displayed or written elsewhere.
- The editor is a **sheet** (Navicat's connection-properties dialog): nothing behind it changes
  until Save, and Save stays disabled until something is dirty.
- "Add from URL…" parses `https://user:secret@host:8443/catalog/schema` (via `TrinoURL`, which
  mirrors the engine's `TrinoConfig.from_url`, including the http default for a bare host) into a
  connection and puts the password straight into the Keychain.
- A port of 443/8443 or a stored password upgrades the scheme to https inside the engine,
  matching `TrinoConfig.__post_init__`. The editor follows the standard port when the scheme is
  switched and the port is still the other scheme's default.
- `Test Connection` runs `test` with the **unsaved** draft values, never the stored ones.
- UserDefaults holds only non-secret preferences: the last output folder and format.

## Build

`app/build.sh` produces `app/dist/QueryHive.app`:

```
Contents/MacOS/QueryHive
Contents/Resources/AppIcon.icns
Contents/Resources/engine/queryhive_engine.py
Contents/Resources/engine/exporter/        (web.py, cli.py and static/ removed)
Contents/Resources/engine/python/          standalone CPython 3.12, arm64
Contents/Resources/engine/site-packages/   pinned: trino, openpyxl, xlwt
```

`app/build-engine.sh` builds `.engine/` and is a no-op while its stamp matches. The engine is
launched as
`engine/python/bin/python3 -s -u engine/queryhive_engine.py <command>` with
`PYTHONPATH=<bundle>/Contents/Resources/engine/site-packages`, `PYTHONNOUSERSITE=1`,
`PYTHONDONTWRITEBYTECODE=1`, every inherited `PYTHON*` variable removed, and the working
directory `~/Library/Application Support/QueryHive/`.

### Distribution

`app/build-dmg.sh` packages the app into `app/dist/QueryHive-<version>-arm64.dmg` and then proves
the artifact: it mounts the image and checks the signature, the architecture, the minimum macOS
from the Mach-O, and that the UI renders from the read-only volume. A DMG that only works after
being copied out fails the build.

**The app is ad-hoc signed.** That signature is internally valid — `codesign --verify --deep
--strict` passes and the app never reports as damaged — but Apple has not vetted it, so a
downloaded copy is quarantined and macOS blocks the first launch until the user allows it once.
Nothing about the app can change that; only notarization can, and notarization needs a paid Apple
Developer Program membership and a **Developer ID Application** certificate.

An "Apple Development" certificate is not a substitute, and this was measured rather than assumed:
signing the bundle with one yields `spctl: rejected, origin=Apple Development: …`, identical to
ad-hoc. It is therefore not used.

`build-dmg.sh --notarize` is the whole remaining path, and `QueryHive.entitlements` carries the
two hardened-runtime exceptions a bundled CPython needs (`disable-library-validation` for the
extension modules dlopen'd out of the bundle, `allow-unsigned-executable-memory` for ctypes).
Those entitlements are **unverified against a real Developer ID** — this machine has none — so the
first notarized build has to be launched and its engine exercised (Test Connection, then an
export) before it is published.
