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
| Action | Two buttons, not one. `HubButton` is the primary: a gradient capsule with a sheen, a coloured glow and a press-scale. `PillButton` is everything else, and its `role` — `secondary`, `quiet`, `destructive` — is what stops a row of them reading as identical blobs. |
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

### Buttons

Four levels, and a button picks one by what it does rather than by where it sits:

| | look | used by |
|---|---|---|
| primary (`HubButton`) | gradient capsule, glow, press-scale | Run / Save / Done / Continue |
| `PillButton(.secondary)` | tinted fill, hairline border, glyph | Load File…, Test Connection, Copy Name, Reveal in Finder |
| `PillButton(.quiet)` | border only, fills on hover | Clear, Cancel, Back |
| `PillButton(.destructive)` | coral fill and border | Delete |

Two rules that came out of looking at the result rather than the code:

- **A disabled primary loses its colour entirely.** It was `opacity(0.4)` + `saturation(0.2)` on the
  gradient, which produced a muddy grey that read as *almost available*. It is now an empty
  capsule with dim text — a placeholder, which is what it is. Whatever disabled it is named right
  next to it ("Choose a connection…"), so the button does not have to explain itself.
- **Every secondary button carries a glyph.** One grey pill whose only variable was `tint` made
  "Load File…" and "Clear" indistinguishable at a glance; the icon is what makes each readable
  without reading it.

### The export wizard

Choosing Export — from Run's menu or from the grid's footer — opens the destination rather than
running blind. Where an export goes is a decision worth seeing every time, since `Replace` can drop
a table, and the settings belong to the moment of exporting rather than to a menu item of their
own; there is no separate "Export Settings…" entry. The wizard's own button commits and closes it,
so a finished export does not leave a panel over the result it just produced.

One rule for every Export in the app: you see where it is going before it goes.

### One toolbar, one Export button

The toolbar had become a settings panel with a Run button on it: a File/Table switch, a format
menu, a folder chip and a name field, all sitting in a row at the same weight as Run. Every one of
those choices now lives behind the **Export** button's chevron, which opens the destination — File
or Table, format and its options, folder and name, or the target table and its write mode — and
names what pressing the button will do before you press it. Both the Run menu and the grid's footer
open the same wizard.

The object pickers are **gone**. The tree already inserts a qualified, quoted name on a double
click, it is always on screen, and it knows each driver's levels for real. A second way to do the
same thing, empty until a fetch came back, earned its removal rather than a redesign.

The grid's columns now **share out whatever width the panel has**. A result whose columns stop two
thirds of the way across reads as unfinished, and the empty band beside it is the first thing the
eye lands on. The row-number gutter comes out of the space first — without that the columns always
fell exactly that far short.

### The editor's own strip

Navicat's query window stacks three rows: an icon toolbar, a strip of object pickers with Run and
Stop, then the editor. Copied literally that would be a third row of chrome in an app whose
toolbar already carries a connection *and* a destination Navicat's does not have. So the three
things worth having were fitted to the rows that already exist:

| Navicat | here | why |
|---|---|---|
| object pickers on their own strip | at the right of the editor header, which was empty | they are a lookup, not an action, so they take the far side of a row whose actions sit left |
| `Run ⌄` | Run with a chevron menu beside it | the primary action stays one click; the variants stay one more |
| `□ Stop` beside Run | Stop beside Run, disabled when idle | it used to swap into Run's slot, which moved the button out from under the pointer exactly when it was being reached for |
| icon toolbar: EXPLAIN, format, layout toggles | **not copied** | the engine has no EXPLAIN, no formatter, and one grid — a button with nothing behind it is worse than no button |
| "Continue on Error" | **not copied** | one statement runs per run; there is no script to continue |

The pickers follow the connection's driver levels exactly as the tree does — no schema picker for
MySQL, no catalog picker for Postgres — and inserting goes through `qualifiedName`, the same
routine the tree's double-click uses, so the two cannot produce different SQL for the same table.
They are `Menu`s and not the combo boxes the export destination uses: everything here already
exists, so a free-text field would only invite the typo the picker exists to prevent.

"Run Current Statement" is `sqlStatement(in:at:)`: a scanner, not a parser. It splits on a `;`
outside a single-quoted string and outside `--` and `/* */` comments. A semicolon inside a
Postgres dollar-quoted body would fool it, and that is deliberate — such a script is rare, and
refusing to guess beats splitting wrongly and running half a statement.

### Double-click opens a table

The tree's double-click is **Open**: a new tab named after the table holding `SELECT * FROM
"catalog"."schema"."table"`, already run, so the rows are on screen before anything is typed. That
is what a database client does with a table, and it is what the gesture is for. Inserting the name
into whatever editor happens to be in front is the same action as before, and it is still there —
in the context menu, where a deliberate choice belongs.

### The column filter has two shapes

Which one you get is decided by the column's own data, not by a setting. Up to **ten distinct
values** the funnel opens a picker: a `Cari` box that narrows the *list*, then the values the column
actually holds, each a checkbox. Past ten it becomes a search box instead, because a list of fifty
values is worse than typing three characters.

The distinct list comes from the rows the preview fetched, which is the same set the filter narrows
— so the two can never disagree about what is in the data. Values are compared exactly, which is why
`LAKI LAKI` and `LAKI-LAKI` stay separate entries: collapsing them would be the filter inventing a
data-cleaning rule nobody asked for. A NULL is offered as its own entry, rendered `null` in italic,
because "show me the rows with nothing here" is a real question about a column full of them.

Mode is decided once, when the popover opens: choosing values clears any text filter first, rather
than silently combining a selection with a string that does not describe it.

### Explain

**Explain** sits right of Stop, where Navicat puts it: the plan is something reached for while
looking at a query, not a peer of Run. It sends the same statement a Run would, in the same context
(`database(for:)` / `schema(for:)`), because a plan for a different context than the query would run
in is worse than no plan — and it shares Run's blocked reasons for the same reason.

The engine owns the spelling per driver (`EXPLAIN <sql>` on all three today), so the button never
has to know. The reply lands in the **same grid**, because a plan *is* a result set — one text
column on Trino and Postgres, a table on MySQL — so the grid already renders it and there is no
second view to keep in step.

What changes is the footer: `Query plan · 9 lines`, with the LIMIT field and Count all hidden,
because neither means anything for a plan. There is no limit to set on an EXPLAIN and counting the
lines of a plan is not a question anyone has. A Run clears the flag, so the footer goes back to
reporting rows.

### Counting, DBeaver's way

`1.000 rows` was the fetched count wearing a total's clothes. The footer now says what it means —
`First 1000 rows · limit reached` when the cap stopped it — and offers **Count all**, which asks the
server how many rows the statement really returns and then reads `1.000 of 48.320 rows`.

It is a button rather than something the preview does, because it is a **second query over the whole
result**: it can be slow, and the user should choose to pay for it. It is also the one place this
app rewrites SQL, wrapping the statement in `SELECT COUNT(*) FROM ( … ) AS queryhive_count`, and
that is acceptable precisely because it is deliberate and visible. Everything about the wrap is
conservative: only the final `;` is stripped, a `;` inside a string literal or a comment survives,
and a statement that is not a single `SELECT`/`WITH` is refused rather than executed.

The count is of `previewedSQL` — the statement that produced what is on screen, remembered at run
time — not of whatever the editor holds when the button is pressed. A total for a different
statement is a number that looks authoritative and is not.

### Run looks, Export writes

Navicat's split, and the thing that makes this a query editor rather than a one-way pipe. The
toolbar's primary action is **Run**: it fetches the first `rowLimit` rows (default 1000) and paints
them in the Result panel's grid, and it writes nothing. **Export** — or **Save to Table**, when
that is the destination — lives in that grid's footer, because it acts on the result the grid is
showing, and because the toolbar had no room left for a second primary.

That split is also why `runBlockedReason` exists twice. Run needs a connection and a statement and
nothing else: a destination it has not been given yet is none of its business. Export needs the
destination too. Before the split there was one check and one button, and pressing it wrote a file
before you had seen a single row.

The grid is the panel's first tab and the default one after a run, so the panel is now 344pt tall —
232pt showed four rows of the thing the user just asked for. Panel tabs are **Result / Log /
Files**; the old Columns tab is gone, because the grid's own header carries every column name and
its type chip and a second list of them was the same information twice.

The grid itself: a `#` gutter shared by the header and the rows so they cannot drift apart, a
pinned header with per-column type chips tinted by kind, numbers right-aligned and everything else
left, alternating rows, `null` in italic dim rather than an empty cell (which is a different
thing), and a footer that never overstates what is on screen — `First N rows · limit reached` in
amber when the cap stopped it, `N rows` otherwise.

### The seams do not shiver

Both drag handles capture the pane's size at the start of a drag and set it from
`value.translation`. That arithmetic was always right; the jitter was in which coordinate space it
was measured in. `DragGesture` defaults to **local** space — the handle's own — and the handle
*moves* as the drag resizes the pane it borders. So the translation was measured against an origin
that the drag itself was shifting: each event alternated between the real delta and roughly zero,
and the seam shivered under the pointer. `.global` measures against the window, which the drag does
not move.

### The default split

The panel starts at 480pt and the window at 1320×880 — a little over half the height for the result
grid, which is what a query editor with a real result set wants and what this app looks like in
use. It was 344pt for a panel that was a message strip, and the strip is gone.

`panelHeight` is still absolute because that is what the resizer sets, but it is **capped at
`AppModel.panelShare` of the workspace** at layout time. A height dragged on a large display must
survive being restored on a small one; without the cap, restoring 480pt onto a 700pt window leaves
the editor 79pt tall. The cap is applied when drawing, never stored, so the user's own value is
never overwritten by the window it happened to open in.

### Why the tree stays fast

The tree did not start slow, it was made slow by three honest mistakes, each visible in the code.

**`AnyView` ate the diff.** The recursion wrapped every child row in `AnyView`, which erases the
row's type — and SwiftUI cannot diff an erased view, so each rebuild drew the whole subtree from
scratch. The docstring even justified it as the only way to recurse; a self-containing `View` is
indeed infinitely sized, but a `@ViewBuilder` *function* can call itself and still return a
concrete `some View`. Same shape, diffable rows.

**Rows listened to the whole app.** Every row carried `@Environment(AppModel.self)` and read the
selection from it, so any observable change — including the editor's keystrokes — re-ran every row.
The selection is now passed in as a plain `Bool`: a row body that reads no observable property is
one SwiftUI can skip when something unrelated changes.

**The breadcrumb walked every node.** `loadedNames` flattened the whole tree with `allNodes()` and
filtered, twice per toolbar redraw. It now walks only the one connection's subtree and stops
descent at a match — 2.4x on a synthetic 30k-node tree, and it no longer scales with every other
connection.

None of the three changed what is drawn, only what is re-drawn.

### The context breadcrumb

The toolbar's picker is a cascade, not a single connection menu: **connection, then that driver's
own levels**. On Trino that is `[connection] [catalog] [schema]`; on Postgres `[connection] [database] [schema]`;
on MySQL `[connection] [database]`, because a schema *is* a database there and offering both would
list the same names twice.

Those levels are `ConnectionKind.contextLevels`, which is deliberately **not** `levels` — the object
tree's contract. They differ in exactly one place: the Postgres tree is schema-first, because a
connection cannot query across databases and browsing one is not a thing you do; but pointing a
query at another database is, so the breadcrumb offers one. That also made Postgres answer
`catalogs` (`SELECT datname FROM pg_database`), which used to be a usage error — its `browse` list
gained the command while `levels` stayed untouched, and the test that pinned the old refusal was
rewritten rather than deleted.

Every level is the **same fixed width**, not sized to its content. Fixed because the names are user
data: a catalog called `aktivitas-produksi-harian` pushed the whole breadcrumb across the bar and
into Run's corner. Equal because they are the same kind of thing — a row of pickers at different
widths reads as though one of them matters more, which is not a claim this bar is making. Middle
truncation is what keeps a long name usable, since the head and the tail are what tell one from
another; the tooltip carries the full value.

That is also what lets Run hold the trailing corner: with the breadcrumb's width independent of its
contents, the button is in the same place on every tab.

It is not decoration. Trino resolves a bare `SELECT * FROM wilayah` against the session's catalog
and schema, so without this the query had to be written fully qualified. Choosing a level sets
`QueryTab.contextDatabase` / `contextSchema`, which every run path reads through
`AppModel.database(for:)` / `schema(for:)` — the tab's pick when it has one, otherwise the
connection's configured default. Blank means "whatever the connection says", so a new tab starts
from the connection and only records a deliberate deviation from it.

Changing the connection clears both, because a schema from the old server does not exist on the new
one. Changing the catalog clears the schema for the same reason.

### Showing every schema

The tree hides Postgres's system schemas — `pg_catalog`, `information_schema`, anything `pg_*` —
because they are not object-tree levels a user browses. Right-clicking a connection offers
**Show System Schemas**, which flips a per-connection flag (`Connection.showAllSchemas`) and re-lists
the connection, this time with `DB_ALL_SCHEMAS=1` so the engine drops its filter. The same switch is
an `InlineCheckbox` labelled **Show All** beside the Schema field in the connection editor — the
editor is where it is set up, the menu is where it is reached while browsing.

MySQL reuses the same switch with the opposite default: its system databases (`information_schema`,
`mysql`, `performance_schema`, `sys`) are **hidden until asked for**, because the server always
returns them and a tree listing them carries four entries nobody browses. That is why its `catalogs`
statement is `SELECT SCHEMA_NAME FROM information_schema.SCHEMATA` rather than `SHOW DATABASES` —
the two list the same thing, and only one of them can carry a `WHERE`. Its checkbox sits beside the
Database field, since MySQL has no schema level.

The toggle is offered on **Postgres and MySQL only**, and the context-menu entry stays Postgres-only
because MySQL's is reachable from the editor. Trino already returns everything its coordinator has,
so a "show all" there would be a control with nothing behind it. The engine nonetheless accepts the
flag on every driver — Trino as a no-op, and every `catalogs_sql` / `schemas_sql` takes the keyword
— so a stray flag can never turn into a `TypeError` the way it would if the parameter existed on
only one driver. That near-miss already happened once, with the Trino schema list.

### Where controls sit

The rule, learned from getting it wrong: **an action belongs next to what it acts on.** The editor
header used to hold `QUERY · 4 lines` at the far left and `Load File…` / `Clear` at the far right
of a 1240pt window — an 800pt journey to reach a button that edits the text directly beneath it,
and a row whose two ends did not look related. They are now one group on the left. The same
mistake put the run spinner at the right end of the panel bar, away from the tabs it describes.

A **button's outcome belongs beside the button**. "Connected · 56 catalogs" used to render up in
the form, above the divider, with a wall between the action and its own result; it now sits
directly right of Test Connection. That cost a layout decision: the footer holds five things, so
the status carries a *negative* `layoutPriority` — it yields width before the button beside it
does, or a greedy status squeezes "Test Connection" down to "Test Conn…". Success truncates at the
tail, a failure in the middle (its cause is at the end, its class prefix at the start, and the
whole message stays in the tooltip). The sheet is 620pt wide for the same reason.

A control that modifies one field sits **with that field**, not in a section of its own. The
schema-level "Show All" checkbox began life as an "Object tree" block below the SSL mode — a
different row from the field it acts on, which invites reading it as a setting about the
connection. It is beside the Schema input now, and `InlineCheckbox` exists for the place: the
`ChipToggle` beside it draws a dark chip, and two boxed controls side by side read as two fields
rather than a field and its modifier.

**Headers group left; footers commit right.** Those are different conventions on purpose. A header
is a label plus its actions, so they sit together at the start. A footer ends a panel or a dialog,
and its action belongs in the corner where the pointer already is — `Reveal in Finder`,
`Copy Name`, and the connection editor's `Cancel` / `Save` stay where they are.

What stays on the right of a header is the control that acts on the *pane* rather than its
contents: the panel's collapse chevron, the window's connection chip. That is a window control,
not an action, and the corner is where you look for it.

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

### Adding a connection

Navicat's flow, and for the same reason: the driver decides the shape of everything after it, so
it is chosen first, from a grid of tiles, and never again. The sheet has three states —
`typePicker` → (`url`) → `form`:

- A **new** connection opens on the grid. Picking a tile sets the driver, its default port and its
  default encryption mode, then moves to the form.
- An **existing** connection opens straight on the form; "Change Type…" goes back to the grid.
- **"New Connection with URI…"** parses into the *form* rather than saving immediately, so the user
  sees what the URL actually meant — an unexpected port or database is easy to paste without
  reading. This is the only URL parser in the app; the sidebar's "Add from URL…" opens this sheet
  at that state rather than keeping a second implementation.

Each tile, and the connection's own tile everywhere it appears, carries the **driver's real brand
mark** rather than a generic database glyph — the same thing every database client does, and what
makes the grid readable at a glance. They live as `.svg` files under `assets/drivers/` and
`app/make-driver-logos.sh` generates the Swift that draws them, so replacing a logo means
replacing a file. `NSImage` decodes SVG on macOS 14, so they stay sharp at every size with no
rasterising step. See `assets/drivers/README.md` for where each came from and what the licences
do and do not cover.

The form shows only the fields the driver has:

| | Trino | PostgreSQL | MySQL |
|---|---|---|---|
| transport | Scheme (http/https) + Verify TLS | SSL mode | SSL mode |
| `database` field | "Catalog" | "Database · Required" | "Database" |
| `schema` field | yes | yes (`search_path`) | **no** |
| tree | catalog → schema → table | schema → table | database → table |

`ConnectionKind.levels` and the engine's `db_drivers` command are the same contract stated twice,
and they have to agree. A `connections.json` written before QueryHive spoke to more than Trino
still loads: the decoder reads the old `httpScheme` and `catalog` keys and never writes them.

### The object tree's context menu

Navicat's, minus the entries this app has nothing behind — no connection profiles, no groups or
sharing, no server-side "New Database", and no separate "Console" (New Query already is one).
What is left is what the app can actually do:

| level | menu |
|---|---|
| connection | Open Connection · Edit / Duplicate / Delete · New Connection · New Query, Open SQL File… · Color ▸ · Refresh · Reveal connections.json |
| catalog, database, schema | Refresh · Copy Name · New Query · Open SQL File… |
| table | Insert into Query · Copy Qualified Name · Copy Name · Refresh |

Two things are deliberately absent from it. **Keyboard shortcuts**, because this app's ⌘R is Run
and ⌘T is already New Query from the Query menu — printing either beside a different action
misleads, and declaring one in two places can fire it twice. And **colour swatches**, because a
context menu cannot draw them; the Color submenu names the colours and ticks the current one
rather than inventing a row of dots that only works in one menu.

Delete asks before it acts, and the question lives on `AppModel.pendingDeletion` rather than in
the row, so the tree and the editor's Delete button ask the same one.

### Popovers, and seeing them

Two surfaces — the format picker and the target table — live in popovers rather than in the
toolbar. Both were **unreviewable for a long time**, because a popover is a window of its own and
`cacheDisplay` on the main window simply does not contain it. `--scene table-target` captures the
popover window instead (`Snapshot` prefers a sheet, then a popover, then the window), and
flattens it over the canvas colour first: a popover draws its own chrome as a system material,
which `cacheDisplay` cannot sample, so an unflattened capture is white-on-transparent and the
white text in it is invisible.

That blind spot cost a real bug. The target fields offer **everything the object tree has loaded
plus everything fetched on demand** (`AppModel.targetChoices`). They used to offer only the tree's
knowledge, so a user who had never expanded that connection saw a plain text field where a
dropdown belongs — a chevron only appears once there is something behind it. Opening the popover
now fetches the catalogs, and choosing one fetches its schemas; the field shows a spinner while
that is in flight rather than looking empty.

### Colouring the query

The editor colours SQL by what each token *is*: keywords in violet, functions in ice, strings in
mint, numbers in amber, quoted identifiers in gold, `null` / `true` / `false` in coral, comments in
dim italic, punctuation in grey.

One scan, six ordered alternatives, then the words are classified. Ordering rather than state is
what handles the cases a naive splitter gets wrong — a `--` inside a string and a quote inside a
comment are resolved by the alternatives being tried in sequence, the same way a reader resolves
them. A word is a function when a `(` follows it, which is the only thing that distinguishes
`count(` from a column called `count`.

Two things it deliberately does not do. It **stops past 200,000 characters** rather than paying for
a full scan on every keystroke of a pasted dump. And it sets **attributes only**, never the string:
that is what keeps it from looping back through `textDidChange`, and why the binding keeps exactly
what was typed.

`updateNSView` cannot do the colouring — it returns early when the string already matches, and
re-running the scan on every SwiftUI update would make typing pay for the model's changes. Text
that arrives from outside the editor is coloured at the two points where it can: when the view is
first built, and after a binding-driven replacement.

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

Some states have no route from a fixture: the connection editor's footer only shows a result after
a real test against a real server, and there is none here. `ConnectionEditorTarget.previewTestCount`
exists for exactly that — snapshot scaffolding, named as such — because the alternative is a state
nobody has ever looked at, which is how that footer's layout came to be wrong in the first place.

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

Commands: `db_drivers`, `test`, `catalogs`, `schemas`, `tables`, `export`, `to_table`. Every
stdout line is one compact JSON object. Every failure, including a usage error, emits
`{"event": "error", "message": ...}` and exits 1. Tracebacks go to stderr.

Settings come from environment variables only, so a password never appears in a process listing.
The app additionally never puts the password inside a URL, so a URL echoed back in an error
message cannot leak it.

Three drivers sit behind the one protocol, chosen by `DB_KIND`: `trino` (the default), `postgres`
(`psycopg` 3.x) and `mysql` (`pymysql`). They are described by `exporter/drivers.py`; every DB_*
name has its old `TRINO_*` alias and `DB_*` wins when both are set, so an app that sends only the
parts and an older one that still sends `TRINO_*` both work.

| Key | Used by | Meaning |
|---|---|---|
| `DB_KIND` | all | `trino` (default), `postgres`, `mysql`. |
| `DB_URL` / `TRINO_URL` | all | Whole connection URL; the parts below override it. Optional: the parts alone build a config. |
| `DB_HOST`, `DB_PORT`, `DB_USER`, `DB_PASSWORD` / `TRINO_HOST`, `TRINO_PORT`, `TRINO_USER`, `TRINO_PASSWORD` | all | Individual parts. A blank port means the driver's default (8080 / 5432 / 3306). |
| `DB_DATABASE` / `TRINO_CATALOG` | all | trino: catalog; postgres: `dbname`; mysql: database. Also what the browse commands list from. |
| `DB_SCHEMA` / `TRINO_SCHEMA` | all | trino: schema; postgres: `search_path`; mysql: unused. |
| `DB_SCHEME` | trino | `http` or `https`. A password implies https, as does port 443/8443. |
| `DB_SSLMODE` | postgres, mysql | postgres `disable`\|`prefer`\|`require`\|`verify-ca`\|`verify-full`; mysql `disable`\|`require`. |
| `DB_INSECURE` / `TRINO_INSECURE` | all | `1` skips certificate verification. |
| `SQL`, `SQL_PATH` | export, to_table | The statement, or a file holding it. One is required. |
| `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE` | to_table | Where the rows are written. The driver decides which it uses; a blank one it *does* use is a usage error. Postgres ignores `TARGET_CATALOG`, MySQL ignores `TARGET_SCHEMA`. |
| `WRITE_MODE` | to_table | `create` (default), `replace`, `append`. All three drivers support all three. |
| `FORMAT` | export | `txt` `csv` `json` `xml` `html` `sql` `xls` `xlsx` `dbf`. Default `csv`. |
| `OUT_DIR`, `NAME`, `ZIP` | export | Where the files land, their base name, and whether to zip a multi-file result. |
| `BATCH_SIZE`, `ROWS_PER_FILE`, `RETRIES` | export, browse | Rows per `fetchmany`, split threshold (`0`/blank = never), transient-error retries. `RETRIES` is floored at 1: the trino connector crashes on `max_attempts = 0`. |
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
| `db_drivers` | `drivers` [{`kind`, `label`, `default_port`, `levels`}] |
| `catalogs`, `schemas`, `tables` | `names` — the identifiers, in the server's own order |
| `to_table` `done` | `rows`, `table`, `mode`, `query_id`, `cancelled`, `warnings`; `progress` also carries `state` |
| `error` | `message` |

- `step connect` is emitted before the network is touched; `step write` and `start` fire
  together, once the cursor is open and the first page exists.
- `progress` is throttled to `PROGRESS_MS` and floored at one event per 1000 rows; a final
  `progress` always precedes `done`. psycopg and pymysql have no `stats_callback`, so those runs
  emit only that final line and `done` carries `cursor.rowcount`-or-`-1`.
- `test` reports `catalog_count`, **not** `catalogs`: `catalogs` is also the name of the browse
  *event*, whose payload is a string array. One key cannot be two types, so the count is named.
  It runs whichever top-level probe the driver has, so it never needs a catalog or schema set.
- `db_drivers` reports the object tree each driver has, so the app never hard-codes it: trino
  `["catalog", "schema", "table"]`, postgres `["schema", "table"]` (the database is fixed by the
  connection), mysql `["database", "table"]` (no schema level, and the middle node is built from
  `catalogs`). It reads no settings and opens nothing.
- Each browse command asks the driver for its statement and quotes identifiers the driver's way
  (`"x"` with `""` for trino and postgres, `` `x` `` with ``` `` ``` for mysql). A command the
  driver has no level for is a usage error naming the driver, never `FROM ""`: `catalogs` on
  postgres ("postgres has no catalog level; the database is set on the connection") and `schemas`
  on mysql. The settings a command needs being blank is a usage error too.
- `SIGTERM`/`SIGINT` set a cancel flag that `run_export`'s `cancel` callback reads. A cancelled
  run still emits a well-formed `done` with `cancelled: true` and the files written so far, then
  exits 0. `Engine.run` redacts every env value whose key looks secret, plus each long `:`-split
  piece of one, out of stderr and out of `error` messages.

### The table destination

`to_table` is the opposite trade to `export`. Nothing is streamed back: the engine opens one
connection, runs `CREATE TABLE … AS <select>` (or `DROP TABLE IF EXISTS` + `CREATE`, or
`INSERT INTO … <select>`) and the database writes the rows itself. A 500M-row CTAS costs the
client one request and some polling, not 500M rows over the wire.

That means `QueryStream` is unusable here — it exists to pull rows and explicitly rejects a
statement with no result set. `exporter/to_table.py` uses the driver's DBAPI directly and leans
on three things I verified in the installed client rather than assumed:

- `cursor.execute()` blocks until the statement is **FINISHED**, so its return is the completion.
- `cursor.rowcount` carries Trino's `update_count` for CTAS and INSERT, and is **-1** when the
  server reported none. The engine never turns that into a number.
- `stats_callback` fires on every coordinator update, which is the only progress signal available
  for a statement whose rows never arrive. It drives the `progress` events, and it is also where
  cancellation is noticed — the callback calls `cursor.cancel()` once, because a signal handler
  cannot interrupt a blocking socket read reliably. It is **trino only**: `psycopg` and `pymysql`
  cursors take no such keyword, so `Driver.cursor` passes it for a driver that has progress stats
  and not for one that does not. Those runs emit no intermediate `progress` and no cancel path,
  and `done` carries `rowcount` or `-1`.

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

The target catalog/schema are deliberately **not** the session's `DB_DATABASE`/`DB_SCHEMA`:
a query may read from one database and write into another.

## Object tree

- One root per saved connection. `children == nil` means "never fetched"; `children == []` means
  "fetched and empty" — the distinction is what keeps a collapsed node from re-querying the
  coordinator on every redraw.
- Expanding a node runs exactly one command: `.connection` → `catalogs`, `.catalog`/`.database` →
  `schemas` or `tables`, `.schema` → `tables`. Tables are leaves. Which levels exist comes from
  `db_drivers`, not from a table in the UI: trino has all three, postgres has schemas under the
  connection, mysql has databases under the connection and no schema level at all.
- Browse runs send `RETRIES=2`: a typo'd catalog should come back quickly, not after five
  backoffs. A failed fetch keeps `children == nil`, so re-expanding retries, and shows the
  error as a child row.
- `rebuildTree()` reuses nodes by id, so a rename, a recolour or a delete never collapses what
  the user had open. "Refresh" (context menu) drops one node's children; the tree header's
  reload button drops the lot.
- Double-click inserts: a table's quoted reference goes into the active query (`"catalog"."schema"."table"`
  on trino, `"schema"."table"` on postgres, `` `database`.`table` `` on mysql), a connection opens
  its editor, anything else toggles.
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
