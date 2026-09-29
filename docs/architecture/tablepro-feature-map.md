# TablePro vs QueryHive: feature map, use cases and priorities (PRD input)

Checked 2026-09-29 against the code in both trees. This is read-only work: I edited nothing and ran no git commands.

**Path roots used below.** Table cells are relative to these two roots.
- `TP:` = `/Users/isal/Workspaces/Lab/Experiments/TablePro/TablePro/` (plugins sit under `/Users/isal/Workspaces/Lab/Experiments/TablePro/Plugins/`)
- `QH:` = `/Users/isal/Workspaces/Lab/Experiments/query_hive/`. App files are under `QH:app/Sources/QueryHive/`. The docs still say `TrinoExporter`, but that folder was renamed.

**Legend.** Relevance means relevance to a data or backend engineer on Postgres, MySQL and Trino: H (high), M (medium), L (low). **Δ** marks a row that disagrees with the gap matrix (`tablepro-feature-analysis.md` §5) or with a later "landed" claim. Rows marked *(inferred)* were not checked end to end.

---

## 0. Corrections to the existing docs

Most of these are load-bearing for the PRD.

1. **The analysis §4 overstates the editor.** It says multi-statement scripts, opening and saving `.sql` files, and Format SQL "already exist". Three of those claims are false:
   - **Format SQL:** there is no formatter anywhere in the app. `format` exists only as a label and key binding in `QH:app/Sources/QueryHive/Models/Shortcuts.swift:38,135`, and nothing handles it.
   - **Comment line and Save File:** same situation. `commentLine` and `saveFile` are bound in `Shortcuts.swift` but have no handler. `QueryTab.swift:1023 loadSQLFromFile()` is the only file I/O, so files can be opened but not saved back.
   - **Multi-statement scripts:** the editor has no statement-by-statement runner. `preview` runs exactly one statement (`QH:crates/qh-ffi/src/commands.rs:1347`), and PROGRESS K9 records that PostgreSQL rejects multi-statement input. The "Run Script" menu item is actually `run(tab, from: .all)`, which is the **export** path (`App.swift:99`), and export is also single-statement (`commands.rs:814-845`). A `.sql` runner does exist (`import_data` with `Source::Statements`, `crates/qh-ffi/src/import.rs:180`), but only the CLI and MCP can reach it. The app's ImportSheet offers only `csv`, `tsv` and `xlsx` (`Models/ImportMapping.swift:36-38`).
2. **SSH is described two opposite ways, and both are wrong.** The analysis §4 says "SSH bastion via qh-tunnel". The settings study §6 says "no SSH tunnel at all". In fact the **engine** has it: `SSH_HOST`, agent/key/password auth and known_hosts checking in `crates/qh-ffi/src/tunnel.rs:24-31`, wired in `crates/qh-ffi/src/lib.rs:334-344`. The **app** does not: there are no SSH fields, and Navicat entries that use SSH are reported as "an SSH tunnel, which this app doesn't open" (`Models/AppModel.swift:1248`). For the target persona this is the biggest single gap.
3. **Insert and delete row are still missing from the grid, even after "Gelombang 3 merged".** `CellEdits.insertRow()` and `deleteRow()` exist (`Models/CellEdits.swift:115,126`), but nothing in the app calls them (a grep for `.insertRow(` and `.deleteRow(` finds 0 matches).
4. **ADR-0003 (use NSTableView for the grid) was never carried out.** The grid is still SwiftUI `LazyVStack` over `[[String?]]` (`Views/ResultGrid.swift:287`, `Models/QueryTab.swift:294`). The columnar store `crates/qh-result-store`, built for 500k × 30 rows, has **no dependents** in any `Cargo.toml`. `docs/benchmarks.md:77` lists "Scroll grid 60 fps" as not yet measured. The gap matrix has no row for this at all.
5. **The PostgreSQL tree shows base tables only.** It filters on `table_type = 'BASE TABLE'` (`crates/qh-driver-postgres/src/lib.rs:802-808`), so PostgreSQL views and materialized views never appear. MySQL and Trino use `SHOW TABLES`, which includes views but doesn't label them. No routines appear anywhere.
6. **The engine exposes no column metadata or DDL.** None of the 26 commands (`crates/qh-ffi/src/lib.rs:506-533`) returns columns or DDL, and no crate calls `SHOW CREATE`, `pg_get_*def` or `information_schema.columns`. The MCP `columns` tool works around this with `LIMIT 0` (`crates/qh-ffi/src/mcp.rs:262`). Autocomplete therefore offers only the columns of the **last result in the tab** (`Models/AppModel.swift:1767-1770`).

---

## 1. Feature map

### 1.1 Connection, auth, SSH and TLS

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| PG / MySQL / Trino drivers | `Plugins/{PostgreSQL,MySQL,Trino}DriverPlugin` | done: `crates/qh-driver-{postgres,mysql,trino}` | H | keep; skip the other 33 engines | |
| SSH tunnel (agent, key, password, known_hosts) | `TP:Core/SSH/SSHTunnelManager.swift`, `LibSSH2Tunnel.swift` | **partial**: engine and CLI only (`crates/qh-tunnel`, `crates/qh-ffi/src/tunnel.rs`). No app UI; Navicat SSH entries dropped (`AppModel.swift:1248`) | **H** | **adopt**: connection form section plus Keychain for the SSH secret. The engine work is done, so this is cheap | **Δ** matrix treats SSH as present |
| Host aliases from `~/.ssh/config`, SSH profiles | `TP:Core/SSH/SSHConfigParser.swift`, `SSHConfigResolver.swift`, `Core/Storage/SSHProfileStorage.swift` | missing | H | adapt: read `~/.ssh/config` Host aliases first; shared profiles later | Δ new row |
| SSH keepalive, connection health check | `TP:Core/SSH/SSHKeepAliveResult.swift`; `connectionHealthCheck` setting | missing. The engine opens one connection per command *(inferred from `open()` in each `commands.rs` fn)*, which lowers the need | M | later; revisit if a tunnel is reused across commands | |
| SOCKS5, tunnel command (kubectl, SSM), Cloudflare, Cloud SQL proxy | `TP:Core/SOCKS/`, `Core/TunnelCommand/`, `Core/Cloudflare/`, `Core/CloudSQL/` | missing | L (on-prem) | skip cloud proxies; tunnel command only if someone asks | |
| TLS modes | `TP:Models/Connection/SSLConfiguration.swift` | done: sslmode for PG/MySQL, verify toggle for Trino (`Models/Connections.swift:95-121`, `Views/ConnectionsViews.swift:471-486`). Custom CA and client-certificate UI not found *(inferred)* | M | adapt: custom CA file path (common with on-prem internal CAs) | |
| Trino auth beyond Basic | JWT (`Plugins/TrinoDriverPlugin/TrinoPlugin.swift:69-85`) | **missing**: Basic only (`crates/qh-driver-trino/src/lib.rs`, PROGRESS K14) | H for enterprise Trino *(inferred)* | adopt JWT; Kerberos/OAuth2 later | **Δ** not in matrix |
| Keychain secrets | yes | done: `crates/qh-credentials` | H | keep | |
| Credential profiles (one login, many connections) | `TP:Core/Storage/` CredentialProfileStorage (settings study §6) | missing | M | later | |
| Groups, colours, tags | `TP:Models/Connection/ConnectionTag.swift`, `Core/Storage/TagStorage.swift` | partial: groups and 6 colours (`Connections.swift:5,441`), no tags | M | adapt: an "environment" tag (prod/staging/dev) that drives the safety cues in §1.13 | Δ |
| Connection import (URL, other apps, cloud) | `TP:Views/Connection/ImportFromApp/`, `ImportFromAWS/`, `Components/ImportFromURLSheet.swift` | done for what matters: Navicat `.ncx` (`Support/NavicatImport.swift`), "Add from URL…" (`Views/SidebarTree.swift:82`) | M | keep; add SSH mapping to the Navicat import once SSH lands | |

### 1.2 Schema browser and tree

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Lazy tree (catalog → schema → table) | yes | done: `Models/SchemaTree.swift` | H | keep | |
| Views and materialized views in the tree (labelled) | yes | **missing for PG**, unlabelled for MySQL/Trino (`qh-driver-postgres/src/lib.rs:802`) | **H** | **adopt** | **Δ** |
| Columns under a table, table info | `TP:Views/Structure/TableStructureView.swift` | **missing**: no columns command | **H** | **adopt**: a read-only `columns` engine command that also feeds autocomplete and MCP | **Δ** |
| Routines and triggers (read source) | `TP:Views/Sidebar/RoutineRowView.swift`, `Models/Query/RoutineInfo.swift` | missing | M (PG functions, MySQL procs) | adapt, read-only | matrix: defer |
| User-defined types / enums | `TP:Core/Database/EnumLabelEditor.swift`, `Models/Query/UserDefinedTypeInfo.swift` | missing | L | skip | |
| Object listing grid (owner, ACL) | similar | done: `objects` (`commands.rs:789`) | M | keep | |
| Table and database favourites | `TP:Core/Storage/FavoriteTablesStorage.swift`, `FavoriteDatabasesStorage.swift` | partial: saved-query favourites only (migration `0004`) | M | later | |
| System schemas toggle | yes | done: `showAllSchemas` / `showAllDatabases` (`Connections.swift:268`) | L | keep | |

### 1.3 SQL editor

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Syntax colouring | yes | done: `Views/SQLSyntax.swift` | H | keep | |
| Schema-aware autocomplete | `TP:Core/Autocomplete/SQLContextAnalyzer.swift`, `DerivedTableParser.swift` | **partial**: loaded tree nodes, last-result columns and keywords only (`AppModel.swift:1747-1773`) | **H** | adapt: catalog-driven columns, alias resolution | Δ |
| Find/replace with regex | yes | done: `Support/EditorFind.swift`, `Views/SQLFindBar.swift` | H | keep | |
| Code folding | yes | done: `Support/SQLFolding.swift` | M | keep | |
| **SQL formatter** | `TP:Core/Services/Formatting/SQLFormatterService.swift` | **missing** (label only, `Shortcuts.swift:38`) | H | **adopt**; dialect-aware, built our own way | **Δ** |
| Toggle comment | yes | **missing** (label only) | H | **adopt** (trivial) | **Δ** |
| `.sql` open, save back, external-change detection | `TP:Core/Services/Infrastructure/SQLFileService.swift`, `DatabaseFileWatcher.swift` | **partial**: load only (`QueryTab.swift:1023`) | H | **adopt**: save and save-as, file-backed tabs | **Δ** |
| Run a multi-statement script with per-statement outcome | `TP:Core/Utilities/SQL/SQLFileParser.swift`, `Views/Results/ResultSetMenu.swift` | **missing in editor**: one statement per preview; engine runner reachable only via CLI/MCP `import_data` | **H** | **adopt**: expose the engine's statement runner (`import.rs:464`) as "Run Script" with stop/continue policy | **Δ** |
| Run current statement, gutter run buttons | yes | done (`203c433`, remaining-work-plan Batch 5) | H | keep | |
| `:name` parameters | `TP:Views/Editor/QueryParameterPanelView.swift` | done as substitution (`Views/ParameterSheet.swift`, ADR-0028) | M | keep | |
| Keyword case | yes | done: `Models/KeywordCase.swift` | L | keep | |
| Vim mode | `TP:Core/Vim/` (17 files) | missing; deferred by the user | M | later | |

### 1.4 Query execution and results grid

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Statement timeout | `queryTimeoutSeconds` (mechanism not verified) | done and **server-enforced** per driver (ADR-0016, `crates/qh-driver/src/lib.rs` `statement_timeout`) | H | keep; ahead | |
| Stop/cancel | yes | done (`AppModel.swift:2842`; MySQL cancel fixed, PROGRESS K11) | H | keep | |
| Row cap and truncated verdict | row cap 10k default (settings study §4) | done: `rowLimit` 1000 default, cap-at-page-boundary fix (PROGRESS K13) | H | keep | |
| **Pagination / load more / fetch all** | `TP:Core/Coordinators/PaginationCoordinator.swift`, `Views/Components/PaginationControlsView.swift` | **missing** | **H** | **adopt**; needs the virtualised grid first | Δ |
| **Grid virtualisation for 500k × 30** | NSTableView data grid (`TP:Views/Results/KeyHandlingTableView.swift`, `Extensions/DataGridView+Viewport.swift`) | **gap**: SwiftUI LazyVStack over a Swift array; ADR-0003 not carried out; `qh-result-store` unused | **H** | **adopt**: carry out ADR-0003 and ADR-0008 | **Δ** new row |
| Sort (in memory and server-side) | yes | done: `Models/GridSort.swift`, `Models/ServerSort.swift` | H | keep | |
| Filters, presets, cross-column search that escalates to the server | `TP:Core/Coordinators/FilterCoordinator.swift` | done: `Models/FilterPresets.swift`, `Models/GridSearch.swift` | H | keep | |
| Hide / reorder / rename columns | yes | done: `Models/GridColumns.swift` | M | keep | |
| Cell viewer (JSON tree, hex, per-column format) | yes | done: `Views/CellValueViewer.swift`, `Models/JSONTree.swift`, `Support/HexDump.swift`, `Models/ColumnFormat.swift` | M | keep | |
| Whole-row inspector | `TP:Views/RowInspector/RowInspectorView.swift` | partial: per-cell viewer only | M (wide tables) | adopt: row-as-form view for wide rows | |
| Copy as TSV / with headers / INSERT / JSON | `TP:Core/Utilities/SQL/RowValueCopyFormatter.swift` | partial: copy and copy-with-headers (`ResultGrid.swift:521`); copy-as-INSERT/JSON not found *(inferred)* | M | adopt copy-as-INSERT | |
| Multiple result sets, pinning | `ResultSetMenu.swift` | missing | M | adopt together with the script runner | |
| Charts / maps | `TP:Views/Results/ResultChartView.swift` | missing | L | skip (§9) | |
| Trino query id shown | not verified | done: `Views/Panels.swift:225` | M | keep; add a link to the Trino UI | |

### 1.5 Data editing and change tracking

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Cell edit → reviewed UPDATE, row count checked inside a transaction | `TP:Core/ChangeTracking/DataChangeManager.swift`, `RowMatchPolicy.swift` | done: `crates/qh-ffi/src/apply.rs`, `Models/MatchPolicy.swift`, `Models/WritePlan.swift` (ADR-0020/0021) | H | keep | |
| **Insert / delete row in grid** | `DataChangeManager.swift`, `BulkDeleteConfirmation.swift` | **partial**: model and engine done, no UI caller | H | **adopt** (UI only) | **Δ** |
| Batch budget and DEFAULT handling | `SQLWriteBatchBudget.swift` | done: `Models/WriteBatchBudget.swift` | M | keep | |
| Undo after commit (Data Rewind) | `TP:Core/DataWrite/Rewind/` | missing | L | skip (licence-gated in TP; pre-commit undo exists) | |

### 1.6 Import and export

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Streaming export (txt, csv, json, xml, html, sql, xls, xlsx, dbf, **parquet**) with part split, retry and zip | `TP:Core/Services/Export/ExportService.swift` plus plugins; Parquet staged via DuckDB (adoption plan §12.2.6) | done: `crates/qh-export/src/lib.rs:119` (10 formats) | H | keep; **ahead** | |
| Export to table on the server (CTAS / replace / append) | not found | done: `commands.rs:1057-1063` | H | keep; **ahead** | |
| Import CSV/TSV/XLSX with mapping and transaction policy | `TP:Core/Services/Export/ImportService.swift`, `Views/Import/RowImportSheet.swift` | done: `Views/ImportSheet.swift`, `crates/qh-import`, ADR-0019 | H | keep | |
| Import `.sql` from the app | yes | partial: engine only | H | adopt (same work as the script runner) | Δ |
| Import JSON | yes | missing | L | later | |
| Data-file window (open CSV/Excel without importing) | `TP:Core/DataFiles/` | missing | L | skip | |
| Copy objects across connections or engines | `TP:Views/ObjectCopy/` | missing (`to_table` works within one connection) | M | later | |
| Backup and restore (pg_dump, mysqldump) | `TP:Views/Backup/`, `Core/Database/PostgresRestoreDiagnostics.swift` | missing | M | later; decided out in the remaining-work-plan | |

### 1.7 History, saved queries and workspace

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| History with full-text search, per connection | `TP:Core/Storage/QueryHistoryStorage.swift` | done: `crates/qh-storage/src/history.rs`, migration `0003`, `Views/Panels.swift` | H | keep | |
| Saved queries and favourites | `TP:Core/Storage/SQLFavoriteStorage.swift` (folders, versions) | partial: flat list plus favourite flag | M | adapt: folders later | |
| History retention (days, auto-cleanup) | `HistorySettings` | missing (remaining-work-plan) | L | later | |
| Session restore | yes | done: `Models/Session.swift`, `session` command | H | keep | |
| Open Quickly | `TP:ViewModels/QuickSwitcherViewModel.swift`, `Core/Utilities/UI/QuickSwitcherFrecencyStore.swift` | done at scope "in-memory tree + saved + history" (`Models/QuickSearch.swift:22-27`) | H | adapt: add frecency | |

### 1.8 Structure and DDL tools

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| **View DDL (read-only)** | `TP:Views/Structure/DDLTextView.swift`, `Core/MCP/Protocol/Tools/GetTableDdlTool.swift` | **missing** | **H** | **adopt** read-only (`SHOW CREATE TABLE`, `pg_get_*def`, Trino `SHOW CREATE TABLE`) | **Δ** matrix bundled it into "structure editor, defer" |
| Structure editor (alter columns, indexes, FKs) | `TP:Views/Structure/StructureEditingSession.swift` | missing | M | skip for now (clashes with Safe Mode, per remaining-work-plan) | |
| Truncate / drop from the tree under Safe Mode | `TP:Views/Sidebar/TableOperationAlert.swift` | done: `table_op`, ADR-0027, `SidebarTree.swift:469-470` | M | keep | |
| Maintenance (VACUUM, ANALYZE, OPTIMIZE) | `TP:Views/Sidebar/MaintenanceSheet.swift` | missing | M | later | |

### 1.9 ER diagram

| Feature | TablePro | QueryHive | Rel. | Recommendation |
|---|---|---|---|---|
| Interactive ER diagram | `TP:Views/ERDiagram/` | missing | L (warehouses rarely have FKs) | skip (§9) |

### 1.10 Query plan and insights

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| EXPLAIN as tree, diagram and plan diff | `TP:Views/QueryPlan/` (tree, diagram, comparison), `Models/Query/QueryPlanDiff.swift` | **partial**: raw plan text in the grid (`AppModel.swift:2692`, `ResultGrid.swift:221`) | **H** (PG EXPLAIN ANALYZE, Trino distributed plans) | adapt: a tree from PG `FORMAT JSON` and Trino `EXPLAIN (FORMAT JSON)`; skip the diagram | matrix "take if cheap", still open |
| Query insights (most frequent, most expensive, regressing) | `TP:Views/QueryInsights/` | missing; history already stores elapsed and rows | M | later (cheap once history is in place) | |

### 1.11 Compare / diff, server dashboard, users and roles

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Schema and data compare with sync script | `TP:Core/Compare/` (~40 files) | missing | M | skip (§9) | |
| Server dashboard (sessions, metrics, slow queries) | `TP:Core/ServerDashboard/Providers/{PostgreSQL,MySQL}DashboardProvider.swift` | missing | **M-H** (on-prem: "who is holding this lock") | adapt minimal: active sessions plus cancel/kill for PG/MySQL, and Trino running queries. Reconsider the §9 rejection | **Δ** |
| Users and roles editor | `TP:Core/UsersRoles/`, `Views/UsersRoles/` | missing | L | skip | |

### 1.12 AI and MCP

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| MCP server | in-app, about 35 tool files (`TP:Core/MCP/Protocol/Tools/`), pairing, grants | done, read-only: 9 tools plus resources and prompts (`crates/qh-ffi/src/mcp.rs:235-337`); hashed tokens, fail-closed allowlist, forced `read_only`; separate process | H | keep; add a `describe_table`/DDL tool once the columns command exists | |
| MCP token and grant UI | `TP:Views/Settings/Sections/MCPGrantListView.swift` | missing (CLI only) | L | later (§12.3) | |
| AI assistant | `TP:Core/AI/` (Anthropic, Gemini, OpenAI-compatible, **Ollama**: `OllamaDetector.swift`), chat tools | missing | M | deferred on the on-prem policy; if taken up, local model first | |

### 1.13 Safety / Safe Mode

| Feature | TablePro | QueryHive | Rel. | Recommendation | Δ |
|---|---|---|---|---|---|
| Per-connection Safe Mode | 6 levels, app-gated (`TP:Models/Connection/SafeModeLevel.swift:9-14`) | done and **engine-enforced**: full / no_ddl / confirm / read_only plus floor (`crates/qh-sql/src/classify.rs`, ADR-0017/0026/0027) | H | keep; **ahead** | |
| Write confirmation sheet | yes | done: `Views/RunConfirmationSheet.swift` | H | keep | |
| Hash-chained execution log | not found | engine done (migration `0007`); **no app viewer** (grep: 0 matches in app) | M | adopt a read-only viewer | Δ |
| **Production cues** (environment tag, badge on tab and title, tinted editor) | `ConnectionTag.swift`, `SafeModeStatus.swift`, `Core/Menu/SafeModeMenuDelegate.swift` | **missing**: colour dot only; no Safe Mode reference in `Views/Workspace.swift` *(inferred)* | **H** | **adopt** (cheap, high value) | **Δ** |

### 1.14 Sync, settings, keyboard and other

| Feature | TablePro | QueryHive | Rel. | Recommendation |
|---|---|---|---|---|
| iCloud sync, iOS app, licensing | yes | missing (`crates/qh-sync` is shape only) | L | skip (§9) |
| Google sign-in and profiles | no | built, pane hidden until a client ID exists (ADR-0029) | L | keep hidden |
| Settings panes | 12 | 5: General, Appearance, Editor, Data, Keyboard (`Views/SettingsView.swift`) | M | keep; add Notifications only together with the feature |
| Theme editor | yes | fixed theme enum (`Support/ThemeStore.swift`) | L | skip |
| Rebindable shortcuts | yes | 2 fixed schemes (`Models/Shortcuts.swift:84-85`) | M | later |
| Notifications for long operations | `TP:Core/Services/Notifications/` | missing | M (long exports) | later; small version with the script runner |
| Headless CLI with the same commands | TP CLI is a launcher (`TP:CLI/BridgeProxy.swift`) | done: `queryhive-engine` plus golden corpus | H | keep; **ahead** |
| URL scheme, AppleScript, Raycast | yes | missing | L | skip (MCP and the CLI cover it) |
| Plugin ABI | yes | compile-time drivers | L | skip |

---

## 2. Where QueryHive is already ahead (the PRD should protect these)

1. **Streaming export with bounded memory.** 500k × 30 rows from PostgreSQL export in 2,626 ms at 9 MB peak RSS (`QH:docs/benchmarks.md:65`). Part splitting, retry and zip are built in. The PRD should say: never materialise the result for export.
2. **`to_table` on the server.** CTAS, replace and append run on the coordinator, so rows never travel to the laptop (`commands.rs:1057`). TablePro has nothing equivalent.
3. **Native Parquet writer with bounded memory per row group.** No DuckDB staging, validated with pyarrow (ADR-0018). TablePro stages Parquet through DuckDB.
4. **Safe Mode enforced in the engine.** It covers the app, the CLI and MCP alike, adds a monotonic floor and a `confirm` level, and keeps a hash-chained execution log (ADR-0017/0026/0027). TablePro gates in the app only.
5. **Server-enforced statement timeout** with typed errors: PG `statement_timeout`, Trino `query_max_run_time`, MySQL `max_execution_time` (ADR-0016).
6. **MCP runs as a separate process.** It fails closed (an empty allowlist means no connections), forces `read_only`, stores token hashes, and never offers `to_table`. Killing it cannot crash the app.
7. **Import with an explicit transaction policy** (stop, commit or skip), FK handling restored on every exit path, and CSV that truly streams (ADR-0019/0022). TablePro memory-maps the whole file.
8. **Deep Trino client.** Hand-rolled (ADR-0006), sends client capabilities so timestamps keep full precision, and shows the query id.
9. **One engine surface for app, CLI and MCP,** with a golden corpus. Every feature can be tested headless.

---

## 3. Use cases

The persona is a data or backend engineer on Postgres, MySQL and Trino: an on-prem warehouse, large tables, SSH bastions, prod access. Feature references point to §1.

**UC-01. Connect to prod Postgres through a bastion.** Actor: backend engineer. Trigger: a new service DB behind a jump host.
1. Adds a connection (or imports one from Navicat) with host, db and user.
2. Fills the SSH section with a `~/.ssh/config` alias or host/user, using agent or key auth.
3. Confirms an unknown host key.
4. Tests the connection, saves it, tags it `prod`.

Success: it connects without a hand-made `ssh -L`; host key checked against known_hosts; the secret stays in Keychain. Needs: SSH UI (1.1), environment tag (1.1/1.13).

**UC-02. Explore an unfamiliar schema.** Actor: data engineer. Trigger: taking over a pipeline.
1. Expands schema → tables and views.
2. Sees the columns and types under a table.
3. Opens the DDL.
4. Double-clicks to preview rows.

Success: views visible and labelled for PG; columns without running a query; DDL in two clicks or fewer. Needs: tree views (1.2), columns command (1.2), DDL view (1.8).

**UC-03. Write a query with real autocomplete.** Trigger: ad-hoc analysis.
1. Types `SELECT` … `FROM hive.analytics.pen…`.
2. Aliases the table.
3. `a.` suggests that table's columns.
4. Formats and runs.

Success: column suggestions without a prior run; alias resolution; one-key format. Needs: catalog autocomplete (1.3), formatter (1.3).

**UC-04. Browse a 500k+ row table.** Trigger: inspecting a fact table.
1. Opens the table.
2. Scrolls quickly through the first page.
3. Loads more or jumps pages.
4. Sorts on the server by a column.

Success: 60 fps scrolling at 500k × 30 with memory under 800 MB (blueprint §6 target); first rows in under 200 ms; sort matches the server, not just the loaded page. Needs: virtualised grid with `qh-result-store` (1.4), pagination (1.4), server sort (done).

**UC-05. Inspect one row of a very wide table.** Trigger: debugging one record with 150 columns.
1. Filters to the record.
2. Opens the row inspector.
3. Searches field names.
4. Opens a JSON cell as a tree.

Success: every field readable without horizontal scrolling. Needs: row inspector (1.4), cell viewer (done).

**UC-06. Run a long migration or backfill script.** Trigger: a 40-statement `.sql` file.
1. Opens the file in a tab.
2. Runs it as a script with the policy "stop on first error".
3. Watches per-statement progress.
4. On failure, sees the failing statement number and message.
5. Fixes it and saves the file.

Success: per-statement status; Stop works between statements; the file is saved back; a history entry exists. Needs: script runner (1.3), `.sql` save (1.3), multiple results (1.4), notifications (1.14).

**UC-07. Stop a runaway query on prod.** Trigger: a wrong join on a large table.
1. Runs it.
2. The timeout or the Stop button fires.
3. Sees a typed timeout message with the bound.
4. Optionally checks the server's active sessions to confirm it's gone.

Success: the server cancels the query (not only the client); the bound appears in the message. Needs: timeout and cancel (done), minimal session view (1.11).

**UC-08. Keep writes off a prod connection.** Trigger: the analyst and engineer share a prod read replica.
1. Sets the connection to `read_only`, or `confirm` with a floor.
2. Pastes a `DELETE`.
3. The engine refuses before connecting.
4. The same refusal applies via the CLI and MCP.

Success: nothing reaches the server; the tab shows prod and read-only state at all times. Needs: Safe Mode (done), prod cues (1.13).

**UC-09. Fix a few rows safely.** Trigger: a data-quality ticket.
1. Filters to the rows.
2. Edits cells, adds a row, deletes a row.
3. Reviews the generated SQL.
4. Commits in one transaction with row counts verified.

Success: what was reviewed is what runs; a count mismatch rolls back. Needs: insert/delete gestures (1.5), apply_changes (done).

**UC-10. Tune a slow query.** Trigger: an endpoint p95 regressed.
1. Runs EXPLAIN (ANALYZE) on PG.
2. Reads the plan as a tree with cost, rows and time per node.
3. Changes the index or query.
4. Compares against the previous plan.

Success: the hottest node is obvious; comparison works through history. Needs: plan tree (1.10), optionally plan diff (1.10) and insights (1.10).

**UC-11. Export a 10M-row extract for a partner.** Trigger: a data request.
1. Writes the query.
2. Chooses CSV/XLSX/Parquet with part size and zip.
3. Exports.
4. Gets the files and a history entry.

Success: flat memory, retries on a dropped connection, split files, no grid involved. Needs: export (done), notifications (later).

**UC-12. Materialise a result inside Trino.** Trigger: building a staging table.
1. Writes a SELECT.
2. Picks the destination "table", mode create/replace/append.
3. Confirms under Safe Mode.
4. Sees the affected row count.

Success: rows never leave the cluster; replace is refused on a read-only connection. Needs: `to_table` (done).

**UC-13. Load a CSV/XLSX into a staging table.** Trigger: a partner file arrives.
1. Import Data from File.
2. Maps columns and skips some.
3. Picks the transaction policy.
4. Imports and checks the count.

Success: a bad row gets the policy's behaviour (rollback, or skip with the row number); CSV streams. Needs: import (done), `.sql` import in the app (1.6).

**UC-14. Find the query I ran last week.** Trigger: repeating an investigation.
1. Presses Open Quickly or opens History.
2. Types a fragment.
3. Filters by connection.
4. Loads it into the editor, or saves it as a favourite.

Success: full-text hit in under 1 s; survives a restart. Needs: history/FTS/favourites (done), frecency (1.7).

**UC-15. Let an AI agent read the warehouse safely.** Actor: engineer using Claude Code or Cursor. Trigger: asking an agent to explore a schema.
1. Issues a scoped token for two connections.
2. Registers `queryhive-mcp` in the client.
3. The agent lists tables, describes columns, previews rows.
4. The agent tries a write and is refused.

Success: allowlist enforced; no credentials in output; `read_only` forced. Needs: MCP (done), describe/DDL tool (1.12, depends on the columns command).

**UC-16. Resume work after a restart.** Trigger: morning.
1. Opens the app.
2. Tabs, SQL and connections come back.
3. File-backed tabs reopen their files.

Success: no lost SQL; no automatic re-execution. Needs: session restore (done), file-backed tabs (1.3).

**UC-17. Find who blocks a table.** Actor: backend engineer on call. Trigger: a migration hangs.
1. Opens server activity on the PG connection.
2. Sees sessions, waits and locks.
3. Cancels the offending backend (under Safe Mode confirm).

Success: done in under 1 minute without writing a `pg_stat_activity` query. Needs: minimal server dashboard (1.11).

**UC-18. Work against a Trino that needs JWT or a custom CA.** Trigger: enterprise Trino behind an internal CA.
1. Adds the Trino connection with a JWT and a CA file.
2. Tests it.
3. Runs queries.

Success: connects without disabling TLS verification. Needs: Trino JWT (1.1), custom CA (1.1).

---

## 4. Priority proposal

**Must-have, so we don't fall behind (a daily-use blocker for the persona):**
1. SSH tunnel in the app: form, Keychain, host-key prompt, `~/.ssh/config` aliases, Navicat SSH mapping (UC-01). The engine is done.
2. Virtualised grid plus pagination: carry out ADR-0003 and wire `qh-result-store` (UC-04). This is also the only way to back any "large tables" claim.
3. Editor basics that exist only as labels: formatter, toggle comment, save and save-as for `.sql`, file-backed tabs (UC-03, UC-06).
4. Script runner in the editor: expose the engine's `.sql` runner, with per-statement results and a stop/continue policy (UC-06).
5. Read-only metadata: views in the PG tree, a `columns` command, a DDL view; feed autocomplete and MCP from them (UC-02, UC-03, UC-15).
6. Prod safety cues: environment tag, Safe Mode badge on tab and title (UC-08).
7. Insert/delete row gestures in the grid. The engine is done, so this is UI only (UC-09).

**Should-have:**
- EXPLAIN tree for PG and Trino JSON plans (UC-10).
- Trino JWT and custom-CA TLS (UC-18).
- Row inspector for wide tables (UC-05).
- Minimal server activity view with cancel, for PG/MySQL/Trino (UC-17). This means reopening the §9 "server dashboard" rejection in a narrow form.
- `.sql` and JSON import from the app.
- Copy as INSERT.
- Execution-log viewer.

**Later:**
- Query insights from history.
- Notifications for long operations.
- Routines/UDT source view.
- Maintenance (VACUUM/ANALYZE).
- Backup/restore via pg_dump.
- Copy objects across connections.
- Rebindable shortcuts.
- Vim mode.
- Credential/SSH profiles.
- MCP token UI.
- History retention.
- AI assistant, local model first, after the on-prem decision.

**Skip:**
- More engines.
- Plugin ABI.
- iCloud, iOS, licensing.
- Charts and maps.
- ER diagram.
- Schema/data compare and sync.
- Users & roles editor.
- Structure editor (for now).
- Data Rewind.
- Theme editor.
- URL scheme, AppleScript, Raycast.
- Cloud tunnels (Cloudflare, Cloud SQL, AWS IAM).

**Not verified:**
- How TablePro enforces its timeout (client or server).
- Whether TablePro shows Trino query ids.
- The QueryHive TLS custom-CA UI (engine hits only in `qh-driver-mysql/src/tls.rs` and `qh-driver-trino/src/lib.rs`).
- Copy-as-INSERT in QueryHive (grep only).
- Actual grid frame rates (never measured by either side's docs).